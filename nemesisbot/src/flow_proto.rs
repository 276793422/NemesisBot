//! flow 原语层（R8 复评整改）：flow 引擎（plugins-wasm+board+connectors
//! 门控）与 publish 骨架（board+security+connectors 门控，**不含**
//! plugins-wasm）共同消费的**纯原语**——token 渲染 / 外链 scheme 闸 /
//! 比较式拆解求值 / JSON path 子集 / JSON 标量化 / plugin 步出参四态解析。
//! 全部零 WASM 依赖（serde_json 纯函数），模块门控取两消费方的公共交集
//! board+connectors——publish 在无 WASM 裁剪组合下可编译（R8：此前
//! publish.rs 无条件引用 crate::flow_engine（其门控含 plugins-wasm）与
//! nemesis_plugins_wasm，裁剪构建 24 处 E0433）。
//!
//! 单一真相源纪律：flow_engine 经 `pub(crate) use` 再导出，内部调用点与
//! 既有单测引用零改动；publish.rs 直接引用本模块。

// 原语层消费面随 feature 组合浮动（如 ERR_SNIPPET/render_tokens/safe_external_url
// 的部分调用点只在 plugins-wasm/security 分支）——board+connectors 最小交集
// 下部分项暂无消费点是公共交集设计的预期形态，非死代码。
#![allow(dead_code)]

use serde_json::{Value, json};

/// 错误文本摘要长度（畸形出参回灌等场景的截断预算；引擎侧同消费）。
pub(crate) const ERR_SNIPPET: usize = 300;

// ---------------------------------------------------------------------------
// token 渲染 / 外链闸
// ---------------------------------------------------------------------------

/// `{{key}}` 渲染单点：lookup 决定每个键的值，未知 token 原样保留（诚实
/// 不猜）。[`crate::flow_engine::substitute_tokens`] 的实例账面/步骤投影
/// 词表经 lookup 注入；publish.rs 的运行期 token（{{commit}}/{{base_sha}}）
/// 也走这里。
pub(crate) fn render_tokens<F: Fn(&str) -> Option<String>>(tpl: &str, lookup: F) -> String {
    let mut out = String::with_capacity(tpl.len());
    let bytes = tpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && let Some(end) = tpl[i + 2..].find("}}")
        {
            let key = tpl[i + 2..i + 2 + end].trim();
            match lookup(key) {
                Some(v) => out.push_str(&v),
                None => out.push_str(&tpl[i..i + 2 + end + 2]),
            }
            i += 2 + end + 2;
            continue;
        }
        // 按 char 边界推进（非 ASCII 安全）。
        let ch_len = tpl[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&tpl[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// 外链 scheme 闸（同源纪律）：只放行 http(s)。空串 = 无外链（放行）。
pub(crate) fn safe_external_url(raw: &str) -> Option<String> {
    // scheme 大小写不敏感（RFC 3986：HTTPS:// 与 https:// 同义）——只放行
    // http(s)，判前缀用小写比对、回传保原形（与旧外链闸语义对齐）。
    let t = raw.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(t.to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 比较式（when 条件 / 对账匹配同源）
// ---------------------------------------------------------------------------

/// 比较式拆解：最早出现位置命中，同位取最长 op（`>=` 不被 `>` 截断）；
/// 两侧 trim + 成对引号剥离；任一侧为空 = None（畸形，调用方诚实报错）。
pub(crate) fn split_compare_expr(expr: &str) -> Option<(&str, &str, &str)> {
    fn strip_quotes(s: &str) -> &str {
        let t = s.trim();
        if t.len() >= 2
            && ((t.starts_with('"') && t.ends_with('"'))
                || (t.starts_with('\'') && t.ends_with('\'')))
        {
            &t[1..t.len() - 1]
        } else {
            t
        }
    }
    const OPS: [&str; 6] = ["==", "!=", ">=", "<=", ">", "<"];
    let hit = OPS
        .iter()
        .filter_map(|op| expr.find(op).map(|pos| (pos, *op)))
        .min_by_key(|(pos, op)| (*pos, std::cmp::Reverse(op.len()))); // 同位时长 op（>=/>）先中
    let (pos, op) = hit?;
    let left = strip_quotes(&expr[..pos]);
    let right = strip_quotes(&expr[pos + op.len()..]);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, op, right))
}

/// 比较求值：两侧均可解析为数字按数值比，否则按字符串比（`==`/`!=` 全等，
/// `>`/`<` 字典序）。
pub(crate) fn compare_str(op: &str, left: &str, right: &str) -> bool {
    let (ln, rn) = (left.parse::<f64>(), right.parse::<f64>());
    match (ln, rn) {
        (Ok(a), Ok(b)) => match op {
            "==" => a == b,
            "!=" => a != b,
            ">=" => a >= b,
            "<=" => a <= b,
            ">" => a > b,
            _ => a < b,
        },
        _ => match op {
            "==" => left == right,
            "!=" => left != right,
            ">=" => left >= right,
            "<=" => left <= right,
            ">" => left > right,
            _ => left < right,
        },
    }
}

// ---------------------------------------------------------------------------
// JSON 标量化 / JSON path 子集
// ---------------------------------------------------------------------------

/// JSON 标量化（过滤/对账比较的统一取值面）：字符串原样；数字/布尔
/// `to_string`；**对象→"object"、数组→"array"**（不是 "null"——PR 过滤
/// `$.pull_request == null` 必须能拒掉对象形态的在场值）；缺席/Null→"null"。
pub(crate) fn json_scalar(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Object(_)) => "object".to_string(),
        Some(Value::Array(_)) => "array".to_string(),
        Some(Value::Null) | None => "null".to_string(),
    }
}

/// JSON path 子集词法：`$` +（`.` 字段 | `[*]`）*。字段段读到下一个
/// `.`/`[` 为止；未知语法（`$[0]`、裸字段、`..` 空段）= None（诚实：调用方
/// 按空结果处理，不猜）。
pub(crate) fn json_path_segments(path: &str) -> Option<Vec<String>> {
    let mut rest = path.strip_prefix('$')?;
    let mut segs = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            let field = &r[..end];
            if field.is_empty() {
                return None; // `$.` 尾随点 / `..` 空段
            }
            segs.push(field.to_string());
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix("[*]") {
            segs.push("*".to_string());
            rest = r;
        } else {
            return None;
        }
    }
    Some(segs)
}

/// JSON path 求值：字段段 = 对象字段取值（缺席/类型不符丢弃）；`[*]` =
/// 数组展开。返回按序命中值（wildcard 可多个）；路径畸形/中途落空 = 空。
pub(crate) fn json_path_eval<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    let Some(segs) = json_path_segments(path) else {
        return Vec::new();
    };
    let mut cur: Vec<&Value> = vec![v];
    for seg in segs {
        let mut next = Vec::new();
        for node in cur {
            match seg.as_str() {
                "*" => {
                    if let Some(arr) = node.as_array() {
                        next.extend(arr.iter());
                    }
                }
                field => {
                    if let Some(child) = node.get(field) {
                        next.push(child);
                    }
                }
            }
        }
        if next.is_empty() {
            return Vec::new();
        }
        cur = next;
    }
    cur
}

/// JSON path 首个命中值。
pub(crate) fn json_path_first<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    json_path_eval(v, path).into_iter().next()
}

// ---------------------------------------------------------------------------
// plugin 步骤出参四态（guest step 协议解析，纯 JSON 面零 WASM 依赖）
// ---------------------------------------------------------------------------

/// run-step 出参四态（status 词表 done/running/failed/skipped + 自由负载）。
/// flow 引擎 plugin 步与 publish 步 plugin 路由臂共用同一解析单点。
#[derive(Debug, PartialEq)]
pub(crate) enum PluginOutcome {
    /// 同步完成：负载 = artifacts 落盘。
    Done(Value),
    /// 保持 Running，下轮 tick 重投（轮询 = 重跑，幂等由 guest 负责）。
    Running,
    /// guest 自报跳过：Skipped 同款推进，负载 = artifacts。
    Skipped(Value),
    /// guest 自报失败：走 on_fail 既有链。
    Failed(String),
    /// 出参畸形/未知 status：按失败走 on_fail（诚实不静默）。
    Malformed(String),
}

/// run-step 出参解析：`{"status": <词表>,...自由负载}`。畸形（非 JSON /
/// 缺 status / 未知 status）= Malformed；failed 的错误文本取负载 `error`
/// 字段（缺省 = status 原文）。负载 = 去 status 键后的对象（done/skipped
/// 落 artifacts；failed 不落——错误文本进 step.error 走 ERR_SNIPPET 截断）。
pub(crate) fn plugin_step_outcome(out_json: &str) -> PluginOutcome {
    let Ok(mut v) = serde_json::from_str::<Value>(out_json.trim()) else {
        return PluginOutcome::Malformed(format!("run-step 出参非 JSON: {}", snippet_of(out_json)));
    };
    let Some(status) = v.get("status").and_then(Value::as_str).map(str::to_string) else {
        return PluginOutcome::Malformed("run-step 出参缺 status 字段".to_string());
    };
    let payload = match v.as_object_mut() {
        Some(o) => {
            o.remove("status");
            Value::Object(o.clone())
        }
        None => json!({}),
    };
    match status.as_str() {
        "done" => PluginOutcome::Done(payload),
        "running" => PluginOutcome::Running,
        "failed" => {
            let err = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("guest 自报 failed（无 error 明细）");
            PluginOutcome::Failed(err.to_string())
        }
        "skipped" => PluginOutcome::Skipped(payload),
        other => PluginOutcome::Malformed(format!("run-step 出参未知 status: {other}")),
    }
}

/// 错误文本摘要（ERR_SNIPPET 同长度，畸形出参回灌用）。
fn snippet_of(s: &str) -> String {
    s.trim().chars().take(ERR_SNIPPET).collect()
}

#[cfg(test)]
mod tests;

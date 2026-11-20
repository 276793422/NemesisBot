//! 敏感读名单（2026-11-20 四体验与安全问题修复批 4）。
//!
//! 读族工具（read_file / read_image / grep / git 等）恒 FileRead=LOW 放行
//! ——工作区指令文件（AGENT.md / CLAUDE.md 等人格/行为定义）被任意路径
//! 读取零审核。本模块提供 basename 精确匹配名单：命中 → pipeline 钉
//! `force_approval`（ABAC 升格 RequireApproval + 跳过「总是允许」规则表，
//! 审批卡必到人——审批放行即误报出路，非硬拦）。
//!
//! 边界（诚实声明）：exec 族（`cat AGENT.md`）不进 FileRead 管线，天然
//! 管不到（用户明示豁免——「管不了脚本，但至少管自家工具的读操作」）；
//! 系统内部 ContextBuilder 读工作区指令文件不走工具管线，不受影响。
//!
//! 热语义（跟随热生效批次 C5 立场）：名单=安全规则，构造期唯一生效
//! （reload 不重建），与 ssrf_allowed_hosts 同款。改名单重启 Agent 生效。

/// 内置默认名单：指令/人格文件族（大小写不敏感 basename 精确匹配）。
/// 覆盖本仓库与业界常见的 agent 指令文件名。
pub const DEFAULT_SENSITIVE_READ_NAMES: &[&str] = &[
    "AGENT.md",
    "AGENTS.md",
    "CLAUDE.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "BOOT.md",
];

/// 名单条目：`lower` = 判定键（小写），`display` = 审批理由/审计展示原文。
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    lower: String,
    display: String,
}

/// 敏感读名单匹配器。挂在 SecurityPlugin 上，构造期随
/// `sensitive_read_names` 扩展名单装配（内置名单恒在，无开关——本功能
/// 即用户诉求本身）。basename 大小写不敏感精确匹配：子目录中的同名
/// 文件同样命中（`docs/AGENT.md` 与 `AGENT.md` 同权），带扩展名变体
/// 不命中（精确语义，`AGENT.md.bak` 不是指令文件）。
pub struct SensitiveReadMatcher {
    entries: Vec<Entry>,
}

impl SensitiveReadMatcher {
    /// 构造：内置默认名单 ∪ 扩展名单（trim、去重、大小写不敏感）。
    pub fn new(extra: &[String]) -> Self {
        let mut entries: Vec<Entry> = DEFAULT_SENSITIVE_READ_NAMES
            .iter()
            .map(|s| Entry {
                lower: s.to_ascii_lowercase(),
                display: (*s).to_string(),
            })
            .collect();
        for raw in extra {
            let display = raw.trim().to_string();
            if display.is_empty() {
                continue;
            }
            let lower = display.to_ascii_lowercase();
            if entries.iter().any(|e| e.lower == lower) {
                continue;
            }
            entries.push(Entry { lower, display });
        }
        Self { entries }
    }

    /// target 命中判定：取 basename（`/` 与 `\` 都剥——Windows 路径形态
    /// 兜底）小写后与名单精确匹配。返回命中条目的展示原文（审批理由用）。
    pub fn hit(&self, target: &str) -> Option<String> {
        let t = target.trim();
        if t.is_empty() {
            return None;
        }
        let base = t.rsplit(['/', '\\']).next().unwrap_or(t);
        let lower = base.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| e.lower == lower)
            .map(|e| e.display.clone())
    }

    /// 结构化 args 深扫兜底（2026-11-20 第二轮横扫，批 4 漏网）：
    /// FileRead 档的动态注册工具（MCP 桥 / WASM 插件 / connector 镜像）
    /// 名字由服务器侧任意给定——extract_target 按名穷举永远追不上，target
    /// 恒空，敏感读臂对整个动态面失效；自由文本 target（git args
    /// `-p -- AGENTS.md` 多 token）的 basename 匹配也会 miss。本方法递归
    /// 收集 args JSON 全部字符串值，逐值按空白 token 拆分后做 basename
    /// 命中判定，任一命中即返回（配合管线臂 `hit(target) ∪ hit_in_args`
    /// 恒双跑，不只在 target 为空时——覆盖面优先）。误伤面 = 某字符串值
    /// 的 token basename 恰与名单同名（如 grep 的 pattern 文本提到
    /// AGENT.md）——保守方向（多弹一张审批卡），可接受。深度上限 4 防
    /// 畸形深嵌套载荷；FileRead 调用频率低（每轮 LLM 往返一次），无热路径
    /// 顾虑。
    pub fn hit_in_args(&self, args: &serde_json::Value) -> Option<String> {
        let mut found = None;
        self.scan_value(args, 0, &mut found);
        found
    }

    /// 深扫递归体：先到先得（命中即短路），`depth` 超限静默放行。
    fn scan_value(&self, v: &serde_json::Value, depth: usize, found: &mut Option<String>) {
        if found.is_some() || depth > 4 {
            return;
        }
        match v {
            serde_json::Value::String(s) => {
                for tok in s.split_whitespace() {
                    // 逐 token 命中；rev:path 形态（git show
                    // HEAD:AGENTS.md，单 token 无空白）再按 ':' 分段兜底
                    // （rsplit 含 token 自身，无 ':' 时与直接 hit 等价）。
                    for seg in tok.rsplit(':') {
                        if let Some(name) = self.hit(seg) {
                            *found = Some(name);
                            return;
                        }
                    }
                }
            }
            serde_json::Value::Array(a) => {
                for item in a {
                    self.scan_value(item, depth + 1, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
            serde_json::Value::Object(o) => {
                for (_k, val) in o {
                    self.scan_value(val, depth + 1, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;

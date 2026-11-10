//! YAML 嵌套深度词法估计（BUG-20261110-044 单一真相源）。
//!
//! serde_yaml 0.9 反序列化器按嵌套深度递归下降，深嵌套 flow 集合
//! （`[[[[…]]]]`）或块式缩进炸弹会栈溢出，而栈溢出是 abort 不可捕获
//! （gateway 进程级 DoS）。不可信 YAML（远端安装的 SKILL.md frontmatter、
//! 模型生成的工作流草稿等）喂给 serde_yaml 之前必须过词法上界闸。
//!
//! 度量件语义（上界估计器，不求精确）：flow 集合按括号字符计（同引号
//! 闭合内的 `[`/`{` 是字面量，serde 不会为其递归）；块式嵌套按缩进宽度
//! 计（2 字符一级，tab 按单字符保守计入）。取二者最大值。
//!
//! **引号状态逐行重置**（BUG-20261110-044 核心修复）：plain scalar 内的
//! 撇号（`it's`/`don't`，真实 SKILL.md 描述里常见）会把幻影单引号跨行
//! 永久开启——若引号状态跨行保留，后续所有行的 flow 括号被当字符串
//! 字面量跳过计深，深度守卫被绕过（炸弹 0 深放行 → serde_yaml 栈溢出
//! abort）。逐行重置后跨行引号标量的内容行按外层计深只会多计不会漏计
//!（上界方向安全；极端多行引号串被保守拒绝可接受）。

/// 嵌套深度上限（64）：真实 schema 深度 ≤ 10，64 已是荒谬余量
/// （与 pipeline.rs 的 serde_json 128 层递归闸同族语义）。
pub const YAML_MAX_NESTING_DEPTH: usize = 64;

/// 词法估计 YAML 文本的嵌套深度上界。
pub fn yaml_nesting_depth(s: &str) -> usize {
    let mut flow_cur = 0usize;
    let mut flow_max = 0usize;
    let mut indent_max = 0usize;
    for line in s.lines() {
        // 引号状态逐行重置（BUG-20261110-044，见模块文档）。
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let indent = line.len() - line.trim_start().len();
        indent_max = indent_max.max(indent);
        for ch in line.chars() {
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && q == '"' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                continue;
            }
            match ch {
                '"' | '\'' => quote = Some(ch),
                '[' | '{' => {
                    flow_cur += 1;
                    flow_max = flow_max.max(flow_cur);
                }
                ']' | '}' => flow_cur = flow_cur.saturating_sub(1),
                _ => {}
            }
        }
    }
    flow_max.max(indent_max / 2)
}

/// 深度闸便捷判定：超 [`YAML_MAX_NESTING_DEPTH`] 返回 Err（诚实拒绝，
/// 喂 serde_yaml 之前调用）。
pub fn check_yaml_nesting_depth(s: &str) -> Result<(), String> {
    let depth = yaml_nesting_depth(s);
    if depth > YAML_MAX_NESTING_DEPTH {
        return Err(format!(
            "nesting depth {depth} > max {YAML_MAX_NESTING_DEPTH} (untrusted-content stack overflow guard)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

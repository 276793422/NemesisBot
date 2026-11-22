//! env 布尔旗标宽容解析（项目单一真相源，BUG-20261122-027）。
//!
//! 值集 = Go `strconv.ParseBool` 全集（true/false/t/f/1/0）+ 项目宽容惯例
//! 扩展（yes/no/on/off，同 config env 覆盖层 `parse_env_bool` 与
//! `normalize_stream_value` 的出发点），大小写不敏感、两端 trim。其余值 =
//! `None`——调用方按各自语境保守缺省并在有日志面时 warn。
//!
//! 消费面：config env 覆盖（nemesis-config sections/io 委托）、v4 验签
//! strict 旗标（nemesis-verify revocation）、desktop child headless 旗标。
//! **程序内部协议标记不走本函数**（parent→child env 注入的 `"1"` 字面
//! 契约，如 NEMESISBOT_EXECUTOR_SANDBOX / NEMESISBOT_SELFTEST_* /
//! NEMESISBOT_CONSOLE 族——精确比对是写方定义的契约，不是用户输入解析）。

/// 宽容布尔解析。真：`true/t/1/yes/on`；假：`false/f/0/no/off`；
/// 其余 `None`（调用方 warn + 保守缺省）。
pub fn parse_env_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "on" => Some(true),
        "false" | "f" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

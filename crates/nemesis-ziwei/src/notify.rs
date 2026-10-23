//! notify —— 报警条件 DSL 运行时求值（T2.3）。
//!
//! 语法与 [`nemesis_ziwei_profile::model::validate_notify_expr`]（入库校验）
//! 同源：`字段 操作符 字面量`，操作符 `== != >= <= > <`，字面量 = 标识符
//! 或数。这里做**运行时**解析 + 对已解码结构快照（serde_json object）求值：
//!
//! - 标识符字面量：与快照同名字段的**字符串值**比较（如 `message==MONITOR_ALERT`）；
//! - 数字字面量：与字段数值比较（`value>90`）；
//! - 字段缺失 / 类型不合 = 不匹配（诚实无异常，不猜）。
//!
//! 自写解析不求值任意表达式——profile 是数据不是脚本（总控 §2.2）。

/// 比较操作符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Ge,
    Le,
    Gt,
    Lt,
}

/// 已解析条件。
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub field: String,
    pub op: Op,
    pub value: Literal,
}

/// 字面量（标识符或数；与入库校验的字法定义一致）。
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Ident(String),
    Num(f64),
}

impl Condition {
    /// 解析条件表达式（语法与 profile 入库校验一致；入库已验，此处兜底）。
    pub fn parse(expr: &str) -> Result<Condition, String> {
        let expr = expr.trim();
        // 与 validate_notify_expr 同序：长操作符优先（== 先于 =的找不到问题
        // 不存在，但 >= 必须先于 > 探测，否则 "a>=1" 会在 ">=1" 上匹配 ">"）。
        let ops = ["==", "!=", ">=", "<=", ">", "<"];
        let mut found: Option<(usize, &str)> = None;
        for op in ops {
            if let Some(pos) = expr.find(op) {
                found = Some((pos, op));
                break;
            }
        }
        let (pos, op) = found.ok_or_else(|| format!("notify 条件 {expr:?} 缺比较符"))?;
        let field = expr[..pos].trim();
        let value = expr[pos + op.len()..].trim();
        let ident = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .next()
                    .map(|c| c.is_ascii_alphabetic() || c == b'_')
                    .unwrap_or(false)
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        };
        if !ident(field) {
            return Err(format!("notify 条件 {expr:?} 字段名 {field:?} 非法"));
        }
        let literal = if ident(value) {
            Literal::Ident(value.to_string())
        } else {
            let n: f64 = value.parse().map_err(|_| {
                format!("notify 条件 {expr:?} 比较值 {value:?} 非法（须数或标识符）")
            })?;
            Literal::Num(n)
        };
        let op = match op {
            "==" => Op::Eq,
            "!=" => Op::Ne,
            ">=" => Op::Ge,
            "<=" => Op::Le,
            ">" => Op::Gt,
            _ => Op::Lt,
        };
        Ok(Condition {
            field: field.to_string(),
            op,
            value: literal,
        })
    }

    /// 对结构快照求值。字段缺失/类型不合 = false（不匹配，不报错）。
    pub fn evaluate(&self, snapshot: &serde_json::Value) -> bool {
        let Some(field) = snapshot.get(&self.field) else {
            return false;
        };
        match &self.value {
            Literal::Ident(want) => {
                let Some(got) = field.as_str() else {
                    return false;
                };
                match self.op {
                    Op::Eq => got == want,
                    Op::Ne => got != want,
                    // 字符串无序——序比较恒 false（诚实不猜）
                    _ => false,
                }
            }
            Literal::Num(want) => {
                let Some(got) = field.as_f64() else {
                    return false;
                };
                match self.op {
                    Op::Eq => got == *want,
                    Op::Ne => got != *want,
                    Op::Ge => got >= *want,
                    Op::Le => got <= *want,
                    Op::Gt => got > *want,
                    Op::Lt => got < *want,
                }
            }
        }
    }
}

/// 解析 + 求值一步（调度器主用口）。
pub fn matches(expr: &str, snapshot: &serde_json::Value) -> bool {
    Condition::parse(expr)
        .map(|c| c.evaluate(snapshot))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;

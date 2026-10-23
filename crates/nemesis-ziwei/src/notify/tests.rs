//! notify DSL 测试（外置：`#[cfg(test)] mod tests;` 约定）。

use super::*;

#[test]
fn parse_all_ops() {
    let c = Condition::parse("state==ALERT").unwrap();
    assert_eq!(c.field, "state");
    assert_eq!(c.op, Op::Eq);
    assert_eq!(c.value, Literal::Ident("ALERT".into()));

    assert_eq!(Condition::parse("value>90").unwrap().op, Op::Gt);
    assert_eq!(Condition::parse("value>=90").unwrap().op, Op::Ge);
    assert_eq!(Condition::parse("value<90").unwrap().op, Op::Lt);
    assert_eq!(Condition::parse("value<=90").unwrap().op, Op::Le);
    assert_eq!(Condition::parse("state!=ALERT").unwrap().op, Op::Ne);
    assert_eq!(
        Condition::parse("value >= 90").unwrap().value,
        Literal::Num(90.0)
    );
    // 负数 / 小数
    assert_eq!(
        Condition::parse("delta<-0.5").unwrap().value,
        Literal::Num(-0.5)
    );
}

#[test]
fn parse_rejects_garbage() {
    assert!(Condition::parse("no-op-here").is_err());
    assert!(Condition::parse("9value==1").is_err(), "字段名须标识符");
    assert!(Condition::parse("a==b c").is_err(), "值须数或标识符");
    assert!(Condition::parse("a==").is_err());
}

#[test]
fn eval_ident_string() {
    let snap = serde_json::json!({ "message": "MONITOR_ALERT", "status": 2 });
    assert!(matches("message==MONITOR_ALERT", &snap));
    assert!(!matches("message==MONITOR_OK", &snap));
    assert!(matches("message!=MONITOR_OK", &snap));
    // 序比较对字符串诚实不猜
    assert!(!matches("message>MONITOR_OK", &snap));
    // 字段缺失 = false
    assert!(!matches("ghost==MONITOR_ALERT", &snap));
}

#[test]
fn eval_numeric() {
    let snap = serde_json::json!({ "value": 1005, "ratio": 0.5 });
    assert!(matches("value>1000", &snap));
    assert!(matches("value>=1005", &snap));
    assert!(!matches("value>1005", &snap));
    assert!(matches("value==1005", &snap));
    assert!(matches("value!=1004", &snap));
    assert!(matches("ratio<0.75", &snap));
    // 类型不合 = false
    assert!(!matches("message>1", &snap));
    // 标识符字面量遇数字字段：类型不合 = false
    assert!(!matches("value>abc", &snap));
}

#[test]
fn malformed_expr_never_panics() {
    // 调度器主入口对坏表达式诚实 false
    assert!(!matches("???", &serde_json::json!({})));
}

use super::*;

fn tmpws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

const TS: &str = "2026-10-14T08:00:00+00:00";

#[test]
fn default_ledger_fresh_workspace() {
    let ws = tmpws();
    let l = load_ledger(ws.path()).expect("缺文件 = 全新模拟账本");
    assert_eq!(l.cash, DEFAULT_CASH);
    assert!(l.positions.is_empty());
    assert!(l.orders.is_empty());
}

#[test]
fn buy_updates_cash_positions_and_orders() {
    let ws = tmpws();
    let mut l = load_ledger(ws.path()).expect("load");
    let o = l.buy("AAPL", 10.0, 150.0, TS).expect("buy");
    assert_eq!(o.order_id, "NB-ORD-1");
    assert_eq!(o.side, OrderSide::Buy);
    assert_eq!(o.status, OrderStatus::Filled);
    assert_eq!(l.cash, r2(DEFAULT_CASH - 1500.0));
    let pos = l.positions.get("AAPL").expect("position");
    assert_eq!((pos.qty, pos.avg_cost), (10.0, 150.0));

    // 加仓 → 加权均价
    l.buy("AAPL", 10.0, 170.0, TS).expect("buy2");
    let pos = l.positions.get("AAPL").expect("position");
    assert_eq!(pos.qty, 20.0);
    assert_eq!(pos.avg_cost, 160.0);
    assert_eq!(l.orders.len(), 2);
    assert_eq!(l.orders[1].order_id, "NB-ORD-2");

    // 资金不足
    let err = l.buy("AAPL", 1.0e9, 150.0, TS).expect_err("insufficient");
    assert!(err.contains("模拟资金不足"), "{err}");
}

#[test]
fn sell_reduces_position_and_removes_on_close() {
    let ws = tmpws();
    let mut l = load_ledger(ws.path()).expect("load");
    l.buy("AAPL", 10.0, 150.0, TS).expect("buy");

    // 无持仓标的
    let err = l.sell("MSFT", 1.0, 100.0, TS).expect_err("no position");
    assert!(err.contains("无 MSFT 持仓"), "{err}");

    // 超量
    let err = l.sell("AAPL", 11.0, 100.0, TS).expect_err("over sell");
    assert!(err.contains("持仓不足"), "{err}");

    // 部分卖出
    l.sell("AAPL", 4.0, 200.0, TS).expect("partial sell");
    let pos = l.positions.get("AAPL").expect("still holding");
    assert_eq!(pos.qty, 6.0);
    assert_eq!(pos.avg_cost, 150.0, "均价不受卖出影响");
    assert_eq!(l.cash, r2(DEFAULT_CASH - 1500.0 + 800.0));

    // 清仓 → 移除持仓
    l.sell("AAPL", 6.0, 160.0, TS).expect("close");
    assert!(!l.positions.contains_key("AAPL"), "清仓后持仓移除");
}

#[test]
fn cancel_is_honest_error_v1() {
    let ws = tmpws();
    let mut l = load_ledger(ws.path()).expect("load");
    let o = l.buy("AAPL", 1.0, 10.0, TS).expect("buy");

    let err = l.cancel(&o.order_id).expect_err("v1 无挂单");
    assert!(err.contains("已即时成交"), "{err}");
    assert!(
        err.contains("反向单") || err.contains("撤销"),
        "文案要指出路: {err}"
    );

    let err = l.cancel("NB-ORD-404").expect_err("not found");
    assert!(err.contains("不存在"), "{err}");
}

#[test]
fn save_load_roundtrip_preserves_all() {
    let ws = tmpws();
    let mut l = load_ledger(ws.path()).expect("load");
    l.buy("AAPL", 10.0, 150.5, TS).expect("buy");
    l.sell("AAPL", 3.0, 160.0, TS).expect("sell");
    save_ledger(ws.path(), &l).expect("save");

    let back = load_ledger(ws.path()).expect("reload");
    assert_eq!(back.cash, l.cash);
    assert_eq!(back.positions.len(), 1);
    assert_eq!(back.orders.len(), 2);
    assert_eq!(back.orders[0].order_id, "NB-ORD-1");
    assert_eq!(back.orders[1].side, OrderSide::Sell);
}

#[test]
fn corrupt_ledger_is_honest_error() {
    let ws = tmpws();
    let path = ledger_path(ws.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, "{broken").expect("write corrupt");
    let err = load_ledger(ws.path()).expect_err("corrupt");
    assert!(err.contains("拒绝当空处理"), "{err}");
    assert!(
        err.contains(path.to_string_lossy().as_ref()),
        "错误要带路径: {err}"
    );
}

#[test]
fn order_id_continues_after_manual_deletion() {
    let mut l = BrokerLedger::default();
    l.buy("A", 1.0, 1.0, TS).expect("buy1");
    l.buy("A", 1.0, 1.0, TS).expect("buy2");
    l.buy("A", 1.0, 1.0, TS).expect("buy3");
    assert_eq!(l.next_order_id(), "NB-ORD-4");
    // 手工删掉中间订单，下一单不重号
    l.orders.remove(1);
    assert_eq!(l.next_order_id(), "NB-ORD-4");
}

#[test]
fn buy_rejects_invalid_input() {
    let mut l = BrokerLedger::default();
    assert!(l.buy("A", 0.0, 10.0, TS).is_err(), "qty=0");
    assert!(l.buy("A", -1.0, 10.0, TS).is_err(), "qty<0");
    assert!(l.buy("A", 1.0, -1.0, TS).is_err(), "price<0");
}

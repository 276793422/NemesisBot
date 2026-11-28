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

/// sell 与 buy 同款价格/数量闸——负价卖出缺闸时
/// proceeds 为负直接扣现金（账本可被推成负余额）；NaN 价也拒。
#[test]
fn sell_rejects_invalid_input() {
    let mut l = BrokerLedger::default();
    l.buy("A", 1.0, 10.0, TS).expect("buy");
    let cash_before = l.cash;

    assert!(l.sell("A", 0.0, 10.0, TS).is_err(), "qty=0");
    assert!(l.sell("A", -1.0, 10.0, TS).is_err(), "qty<0");
    assert!(l.sell("A", 1.0, -1.0, TS).is_err(), "price<0");
    assert!(l.sell("A", 1.0, f64::NAN, TS).is_err(), "price=NaN");

    // 全部拒绝后账本原封不动（无现金变动、持仓不丢）
    assert_eq!(l.cash, cash_before, "非法单不得动账");
    assert!(l.positions.contains_key("A"), "非法单不得动持仓");
}

// BUG-20261121-030：非有限价格与 r2 溢出闸——+inf 价漏过旧
// `price < 0.0 || price.is_nan()` 闸；qty=1、price=1e307 的**全有限** JSON
// 输入经 r2 的 ×100 放大溢出 proceeds=inf，cash 记入 inf 后资金闸
// `cash < cost` 对一切有限 cost 永假（inf < inf == false），账本资金检查
// 永久失效。
#[test]
fn buy_rejects_non_finite_and_overflowing_cost() {
    let mut l = BrokerLedger::default();
    let cash_before = l.cash;

    assert!(l.buy("A", 1.0, f64::INFINITY, TS).is_err(), "price=+inf");
    assert!(
        l.buy("A", 1.0, f64::NEG_INFINITY, TS).is_err(),
        "price=-inf"
    );
    assert!(l.buy("A", f64::INFINITY, 1.0, TS).is_err(), "qty=+inf");
    // 全有限输入 × r2 放大溢出：拒单不动账。
    let err = l
        .buy("A", 1.0, 1.0e307, TS)
        .expect_err("qty×price ×100 溢出必须拒单");
    assert!(err.contains("溢出"), "{err}");

    assert_eq!(l.cash, cash_before, "被拒订单不得动现金");
    assert!(l.positions.is_empty(), "被拒订单不得建持仓");
    // 账本仍可用（资金闸未被 inf 破坏——修复前 cash 已成 inf）。
    l.buy("A", 1.0, 10.0, TS).expect("正常单照常成交");
}

#[test]
fn sell_rejects_non_finite_and_overflowing_proceeds() {
    let mut l = BrokerLedger::default();
    l.buy("A", 1.0, 10.0, TS).expect("建仓");
    let cash_before = l.cash;

    assert!(l.sell("A", 1.0, f64::INFINITY, TS).is_err(), "price=+inf");
    let err = l
        .sell("A", 1.0, 1.0e307, TS)
        .expect_err("proceeds 溢出必须拒单");
    assert!(err.contains("溢出"), "{err}");

    // 现金分文未动、持仓未丢、后续正常卖出照常。
    assert_eq!(l.cash, cash_before, "被拒订单不得动现金");
    assert!(l.positions.contains_key("A"), "被拒订单不得动持仓");
    l.sell("A", 1.0, 10.0, TS).expect("正常卖出照常");
}

// BUG-20261120-035：账本写锁必须是进程级单例——写者不止一个工具实例
// （主 loop 与项目 loop 各持一份 ConnectorInvokeTool，治理资产锚同一主
// workspace），锁若退化为 per-instance 实例字段，跨实例交错 RMW = 后
// save 者整文件覆盖（丢单/资金凭空重复）。两次独立获取的数据地址必须
// 同一（静态 Mutex 内联存储数据，地址恒定）。guard 收在具名局部变量里
// 函数尾释放——勿写 `let p = &*ledger_write_lock();` 形态：let 初始化式
// 借用触发临时 lifetime extension，guard 活到测试体结束，同线程二次取
// 锁即死锁（parking_lot 非重入，首轮实录挂死）。
#[test]
fn ledger_write_lock_is_single_process_global() {
    fn held_addr() -> *const () {
        let guard = ledger_write_lock();
        std::ptr::from_ref::<()>(&*guard)
    }
    let p1 = held_addr();
    let p2 = held_addr();
    assert!(
        std::ptr::eq(p1, p2),
        "写锁必须钉在进程级静态单例上，不得随调用方实例化"
    );
}

// 同锁并发 RMW 守恒：两「实例」（线程）各 25 笔 buy 全程走
// ledger_write_lock 串行化——订单数与资金必须精确守恒。无锁（或
// per-instance 锁）时交错 load→save 后 save 整文件覆盖，必然丢单。
#[test]
fn concurrent_rmw_through_write_lock_loses_no_orders() {
    let ws = tmpws();
    let root = ws.path().to_path_buf();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let root = root.clone();
            std::thread::spawn(move || {
                for _ in 0..25 {
                    let _serial = ledger_write_lock();
                    let mut l = load_ledger(&root).expect("load");
                    l.buy("AAPL", 10.0, 150.0, TS).expect("buy");
                    save_ledger(&root, &l).expect("save");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
    let l = load_ledger(&root).expect("reload");
    assert_eq!(l.orders.len(), 50, "50 笔订单一笔不丢");
    assert_eq!(l.cash, r2(DEFAULT_CASH - 1500.0 * 50.0), "资金精确守恒");
    // 订单号全唯一（next_order_id 按现存最大序号 +1，锁内无竞争重号）
    let mut ids: Vec<_> = l.orders.iter().map(|o| o.order_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 50, "订单号不得重号");
}

// BUG-20261128-044：二阶 r2 溢出——现金**累计** r2(cash+proceeds) 的 ×100
// 放大可越过 f64 上限（单笔 proceeds 一阶闸拦不住：两笔各 1.7e306 的卖出
// 相加 3.4e306 → ×100 = 3.4e308 = inf）。inf 入账 = 资金闸 `cash < cost`
// 永假 + serde_json 把非有限 f64 序列化为 null = 账本文件砖化（下次 load
// 诚实报损坏）。拒单必须不动账、文件照常可落盘回读。
#[test]
fn sell_cash_accumulation_overflow_rejected_ledger_intact() {
    let mut l = BrokerLedger::default();
    l.buy("QQQ", 2.0, 1.0, TS).expect("建仓 2 股");
    // 第一笔近界卖出过闸：proceeds = r2(1.7e306) = 1.7e306 有限，
    // 累计 cash ≈ 1.7e306（999998 在该数量级被 float 吸收）。
    l.sell("QQQ", 1.0, 1.7e306, TS)
        .expect("第一笔近界卖出应正常成交");
    let cash_after_first = l.cash;
    assert!(
        cash_after_first.is_finite() && cash_after_first > 1.0e306,
        "第一笔后现金 ≈ 1.7e306，实际 {cash_after_first}"
    );

    // 第二笔同价卖出：proceeds 一阶有限（1.7e306，旧一阶闸放行），
    // 但累计 r2(3.4e306) = 3.4e308 = inf——必须拒单不动账。
    let err = l
        .sell("QQQ", 1.0, 1.7e306, TS)
        .expect_err("现金累计溢出必须拒单");
    assert!(err.contains("现金累计溢出"), "{err}");
    assert!(l.cash.is_finite(), "拒单后现金必须仍有限");
    assert_eq!(l.cash, cash_after_first, "被拒订单不得动现金");
    let pos = l.positions.get("QQQ").expect("被拒订单不得动持仓");
    assert_eq!(pos.qty, 1.0, "被拒订单不得动持仓数量");
    assert_eq!(l.orders.len(), 2, "拒单不得追加订单记录");

    // 关键反砖化断言：账本照常落盘且可回读（修复前 cash=inf 序列化为
    // null，load 直接报损坏）。
    let ws = tmpws();
    save_ledger(ws.path(), &l).expect("有限账本必须可落盘");
    let reloaded = load_ledger(ws.path()).expect("落盘后必须可回读");
    assert_eq!(reloaded.cash, l.cash, "落盘回读现金一致");
}

// BUG-20261128-044 同族（buy 均价臂）：avg_cost 累计 r2(总成本/数量) 的
// ×100 放大同样可溢出。现金/持仓受 sell 闸约束后正常交易路径到不了界，
// 但 load_ledger 接受任意有限 f64 的手工账本——用 avg_cost=1e308 的存量
// 持仓加仓 1 股验证防线：r2((1e308+1)/2) = r2(5e307) → 5e309 = inf → 拒单
// 不动账（修复前 inf 均价入账，save 序列化 null 砖化账本）。
#[test]
fn buy_avg_cost_overflow_rejected_ledger_intact() {
    let ws = tmpws();
    let path = ws.path().join("data/paper_broker.json");
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(
        &path,
        r#"{"cash": 1000000.0, "positions": {"XX": {"symbol": "XX", "qty": 1.0, "avg_cost": 1e308}}, "orders": []}"#,
    )
    .expect("write");
    let mut l = load_ledger(ws.path()).expect("手工账本可加载");
    let err = l.buy("XX", 1.0, 1.0, TS).expect_err("均价累计溢出必须拒单");
    assert!(err.contains("累计值溢出"), "{err}");
    let pos = &l.positions["XX"];
    assert!(pos.avg_cost.is_finite(), "拒单后均价必须仍有限");
    assert_eq!(pos.avg_cost, 1e308, "被拒订单不得动持仓均价");
    assert_eq!(l.cash, 1000000.0, "被拒订单不得动现金");
    assert!(l.orders.is_empty(), "拒单不得追加订单记录");
    save_ledger(ws.path(), &l).expect("拒单后账本照常可落盘");
}

// BUG-20261128-044 同族（buy 现金臂）：存量文件现金 1.8e307（合法有限
// JSON，一阶闸全放行）扣 1 元 → r2(1.8e307-1) = 1.8e309 = inf → 拒单，
// 堵住「加载即信任」路径。
#[test]
fn buy_cash_deduction_overflow_rejected() {
    let ws = tmpws();
    let path = ws.path().join("data/paper_broker.json");
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(&path, r#"{"cash": 1.8e307, "positions": {}, "orders": []}"#).expect("write");
    let mut l = load_ledger(ws.path()).expect("手工账本可加载");
    let err = l.buy("YY", 1.0, 1.0, TS).expect_err("现金扣减溢出必须拒单");
    assert!(err.contains("累计值溢出"), "{err}");
    assert_eq!(l.cash, 1.8e307, "被拒订单不得动现金");
    assert!(l.orders.is_empty(), "拒单不得追加订单记录");
}

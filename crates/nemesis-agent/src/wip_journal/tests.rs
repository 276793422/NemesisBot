//! wip 侧车单测（独立测试文件——仓内内联测试纪律）。
//! 覆盖：注册/追加/折叠全生命周期、未注册静默、消费即删+pending 一次
//! 性、跨实例（模拟重启）消费、滚动上限、per-session 隔离、空 key 全
//! no-op、事件 serde round-trip、注入节渲染契约。

use super::*;

#[test]
fn test_register_append_fold_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    let sk = "sess-a";

    j.register(root, sk);
    assert_eq!(j.live_count(), 1);
    let path = root.join("logs").join("wip").join("sess-a.jsonl");
    assert!(path.parent().unwrap().exists());

    j.append(
        sk,
        WipEvent::ToolStart {
            ts: WipEvent::now_ms(),
            call_id: "c1".into(),
            tool: "exec".into(),
            args: "ls".into(),
        },
    );
    j.append(
        sk,
        WipEvent::ToolEnd {
            ts: WipEvent::now_ms(),
            call_id: "c1".into(),
            ok: true,
            dur_ms: 42,
            result: "ok".into(),
        },
    );
    j.append(
        sk,
        WipEvent::Text {
            ts: WipEvent::now_ms(),
            text: "正在查看目录".into(),
        },
    );

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 3, "三个事件 = 三行 JSONL");

    // 折叠：文件删、注册表清。
    assert!(j.fold(sk));
    assert_eq!(j.live_count(), 0);
    assert!(!path.exists());
    // 幂等：二次 fold 无操作。
    assert!(!j.fold(sk));
}

#[test]
fn test_append_without_register_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    // 未注册 key（detached 子代理形态）：追加零写、无 panic。
    j.append(
        "detached-key",
        WipEvent::ToolStart {
            ts: 0,
            call_id: "c".into(),
            tool: "exec".into(),
            args: "x".into(),
        },
    );
    assert!(!root.join("logs").join("wip").exists());
    assert_eq!(j.live_count(), 0);
    // fold 未注册 key 同样无操作。
    assert!(!j.fold("detached-key"));
}

#[test]
fn test_consume_stages_pending_and_register_truncates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    j.register(root, "s1");
    j.append(
        "s1",
        WipEvent::ToolStart {
            ts: 1_700_000_000_000,
            call_id: "c1".into(),
            tool: "web_fetch".into(),
            args: "https://example.com".into(),
        },
    );

    // 消费只暂存不删盘（崩溃韧性：连续硬杀现场不回退）。
    assert!(j.consume_previous(root, "s1"));
    assert!(
        root.join("logs").join("wip").join("s1.jsonl").exists(),
        "消费不删文件——截断收口在 register"
    );
    let rendered = j.pending_for("s1").expect("pending 已挂表");
    assert!(rendered.contains("web_fetch"), "渲染含工具名原文");
    assert!(rendered.contains("https://example.com"), "渲染含参数原文");
    // peek 语义：同一 turn 多轮构建拿到同一文本（字节稳定）。
    assert_eq!(j.pending_for("s1").as_deref(), Some(rendered.as_str()));

    // register 截断：旧现场删除，本轮从空文件写自己的事件。
    j.register(root, "s1");
    j.append(
        "s1",
        WipEvent::Text {
            ts: 1_700_000_000_100,
            text: "本轮自己的事件".into(),
        },
    );
    let raw = std::fs::read_to_string(root.join("logs").join("wip").join("s1.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 1, "register 截断后从空文件起写");
    assert!(raw.contains("本轮自己的事件"));
    assert!(!raw.contains("web_fetch"), "旧现场不混入本轮文件");
    // fold 清 pending + 删文件。
    assert!(j.fold("s1"));
    assert!(j.pending_for("s1").is_none());
}

#[test]
fn test_consume_previous_cross_instance_simulates_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // 实例 1（崩溃前进程）：写完不折叠即丢弃（模拟硬杀）。
    let j1 = WipJournal::default();
    j1.register(root, "crash-sess");
    j1.append(
        "crash-sess",
        WipEvent::ToolEnd {
            ts: 1_700_000_000_000,
            call_id: "c9".into(),
            ok: false,
            dur_ms: 5,
            result: "被中断".into(),
        },
    );
    drop(j1);

    // 实例 2（重启后进程）：全新 WipJournal 消费盘上遗留现场。
    let j2 = WipJournal::default();
    assert!(
        j2.consume_previous(root, "crash-sess"),
        "跨实例从磁盘检出遗留现场"
    );
    let rendered = j2.pending_for("crash-sess").unwrap();
    assert!(rendered.contains("失败/中断"));

    // 无遗留文件的正常轮：消费静默 false 且清 stale pending。
    j2.consume_previous(root, "no-such-sess");
    assert!(j2.pending_for("no-such-sess").is_none());
    // 垃圾行（非 JSON）：解析失败被滤掉，全垃圾 = false 且不挂 pending。
    std::fs::create_dir_all(root.join("logs").join("wip")).unwrap();
    std::fs::write(
        root.join("logs").join("wip").join("garbage.jsonl"),
        "not json\n{broken\n",
    )
    .unwrap();
    assert!(!j2.consume_previous(root, "garbage"));
    assert!(j2.pending_for("garbage").is_none());
}

#[test]
fn test_consecutive_crash_scene_advances_not_regresses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // turn N-1：崩溃前写了现场 A。
    let j = WipJournal::default();
    j.register(root, "cc");
    j.append(
        "cc",
        WipEvent::Text {
            ts: 1,
            text: "场景A".into(),
        },
    );
    drop(j);
    // turn N（重启后）：消费场景 A → 注册截断 → 写自己的事件 → 又崩。
    let j2 = WipJournal::default();
    assert!(j2.consume_previous(root, "cc"));
    assert!(j2.pending_for("cc").unwrap().contains("场景A"));
    j2.register(root, "cc");
    j2.append(
        "cc",
        WipEvent::Text {
            ts: 2,
            text: "场景B续跑工作".into(),
        },
    );
    drop(j2);
    // turn N+1（再次重启）：消费到的是**最新**现场 B（含续跑工作），
    // 信息不回退到 A。
    let j3 = WipJournal::default();
    assert!(j3.consume_previous(root, "cc"));
    let scene = j3.pending_for("cc").unwrap();
    assert!(scene.contains("场景B续跑工作"), "最新现场在场");
    assert!(!scene.contains("场景A"), "旧场景已被截断不回放");
}

#[test]
fn test_fold_clears_pending_too() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    j.register(root, "pf");
    j.append(
        "pf",
        WipEvent::Text {
            ts: 0,
            text: "中间叙述".into(),
        },
    );
    assert!(j.consume_previous(root, "pf"));
    // 同一轮内异常收尾：折叠连 pending 一起清（防跨轮幽灵注入）。
    assert!(j.fold("pf"));
    assert!(j.pending_for("pf").is_none());
}

#[test]
fn test_rotation_cap_keeps_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    j.register(root, "rot");
    for i in 0..(MAX_EVENTS + 50) {
        j.append(
            "rot",
            WipEvent::Text {
                ts: i as u64,
                text: format!("事件{i}"),
            },
        );
    }
    let raw = std::fs::read_to_string(root.join("logs").join("wip").join("rot.jsonl")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert!(
        lines.len() <= MAX_EVENTS,
        "滚动上限：{} 行 > {}",
        lines.len(),
        MAX_EVENTS
    );
    // 保尾：最后一行是最晚事件、最早事件已被丢。
    assert!(
        lines
            .last()
            .unwrap()
            .contains(&format!("事件{}", MAX_EVENTS + 49)),
        "最后一行应为最新事件"
    );
    assert!(!lines[0].contains("事件0"), "最旧事件应已滚出");
}

#[test]
fn test_per_session_isolation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    j.register(root, "sess/one:with-special");
    j.register(root, "sess-two");
    j.append(
        "sess/one:with-special",
        WipEvent::Text {
            ts: 0,
            text: "a".into(),
        },
    );
    j.append(
        "sess-two",
        WipEvent::Text {
            ts: 0,
            text: "b".into(),
        },
    );
    let wip_dir = root.join("logs").join("wip");
    let entries: Vec<_> = std::fs::read_dir(&wip_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 2, "两会话两文件：{:?}", entries);
    // 特殊字符 key 被 sanitize 成安全文件名。
    assert!(entries.iter().all(|n| n.ends_with(".jsonl")));

    // 折叠其一不影响另一。
    assert!(j.fold("sess-two"));
    assert_eq!(j.live_count(), 1);
    let rest: Vec<_> = std::fs::read_dir(&wip_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(rest.len(), 1, "折叠后剩一文件");
}

#[test]
fn test_dotted_session_key_keeps_full_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    // 点号 key（email chat_id / IP 形态）：sanitize 白名单保留点号，文件名
    // 必须字面拼接扩展名——`with_extension` 会把最后一个点之后整段当扩展名
    // 替换（`chan_email_a.name_x.com` → `chan_email_a.jsonl`），同前缀点号
    // key 撞同一文件 = 现场跨会话串台。
    j.register(root, "chan:email:a.name@x.com");
    j.register(root, "chan:email:a.name@y.org");
    j.append(
        "chan:email:a.name@x.com",
        WipEvent::Text {
            ts: 0,
            text: "来自x".into(),
        },
    );
    j.append(
        "chan:email:a.name@y.org",
        WipEvent::Text {
            ts: 0,
            text: "来自y".into(),
        },
    );
    let wip_dir = root.join("logs").join("wip");
    let entries: Vec<_> = std::fs::read_dir(&wip_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 2, "两个点号 key 必须两个文件：{:?}", entries);
    let fx = wip_dir.join("chan_email_a.name_x.com.jsonl");
    let fy = wip_dir.join("chan_email_a.name_y.org.jsonl");
    assert!(fx.exists(), "点号 key 全名保留：{:?}", entries);
    assert!(fy.exists(), "点号 key 全名保留：{:?}", entries);
    let raw_x = std::fs::read_to_string(&fx).unwrap();
    assert!(raw_x.contains("来自x"));
    assert!(!raw_x.contains("来自y"), "x/y 现场不得串台");
    assert!(j.fold("chan:email:a.name@x.com"));
    assert!(!fx.exists(), "fold 删全名文件");
    assert!(fy.exists(), "fold 其一不影响另一");
}

#[test]
fn test_empty_session_key_full_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    assert!(!j.consume_previous(root, ""));
    j.register(root, "");
    j.append(
        "",
        WipEvent::Text {
            ts: 0,
            text: "x".into(),
        },
    );
    assert!(!j.fold(""));
    assert!(j.pending_for("").is_none());
    assert_eq!(j.live_count(), 0);
    assert!(!root.join("logs").exists(), "空 key 不应创建任何目录");
}

#[test]
fn test_wip_event_serde_round_trip() {
    let events = vec![
        WipEvent::ToolStart {
            ts: 1,
            call_id: "c".into(),
            tool: "exec".into(),
            args: "ls -la".into(),
        },
        WipEvent::ToolEnd {
            ts: 2,
            call_id: "c".into(),
            ok: false,
            dur_ms: 7,
            result: "timeout".into(),
        },
        WipEvent::Text {
            ts: 3,
            text: "叙述".into(),
        },
    ];
    for e in events {
        let line = serde_json::to_string(&e).unwrap();
        assert!(line.contains(r#""t":"#), "判别字段 t 在场：{line}");
        let back: WipEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(back.ts_ms(), e.ts_ms());
    }
    // 判别字面钉住（磁盘格式契约）。
    let s = serde_json::to_string(&WipEvent::Text {
        ts: 0,
        text: String::new(),
    })
    .unwrap();
    assert!(s.contains(r#""t":"text""#));
}

#[test]
fn test_render_injection_section_contract() {
    let text = render_injection_section("[12:00:00] 调用工具 exec，参数：ls");
    assert!(text.contains("中断现场恢复"));
    assert!(text.contains("Interrupted Turn Scene"));
    assert!(text.contains("调用工具 exec"), "事件原文在场");
    assert!(text.contains("一次性注入"), "一次性语义向模型声明");
    assert!(text.contains("不是行动指令"), "事实记录非指令防注入放大");
}

/// BUG-20261123-015：撕裂尾不毒化恢复现场——wip jsonl 尾行多字节截半
/// （断电残留）时，旧读法 read_to_string 整文件 Err → consume_previous
/// 返回 false → 上一轮恢复材料全部静默丢弃。修复后坏尾行 parse 失败
/// 跳过，前缀事件照常进 pending 注入表。
#[test]
fn test_consume_previous_torn_tail_preserves_prefix() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let j = WipJournal::default();
    j.register(root, "s-torn");
    j.append(
        "s-torn",
        WipEvent::ToolStart {
            ts: 1,
            call_id: "c1".into(),
            tool: "exec".into(),
            args: "ls".into(),
        },
    );
    j.append(
        "s-torn",
        WipEvent::Text {
            ts: 2,
            text: "正在查看".into(),
        },
    );
    // 模拟断电撕裂：「看」(E7 9C 8B) 截半只留首字节，无换行。
    let path = root.join("logs").join("wip").join("s-torn.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(&[0xE7]).unwrap();
    drop(f);

    assert!(
        j.consume_previous(root, "s-torn"),
        "撕裂尾不得丢弃前缀现场（修复前整文件 Err → false）"
    );
    let rendered = j.pending_for("s-torn").expect("pending 已挂表");
    assert!(rendered.contains("exec"), "前缀工具事件保全");
    assert!(rendered.contains("正在查看"), "前缀文本事件保全");
}

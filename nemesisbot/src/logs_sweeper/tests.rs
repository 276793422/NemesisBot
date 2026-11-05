//! M7/M8 清扫器测试：TTL 纯函数 / 账本末行判定 / 各族夹具删留 /
//! 全 0 不扫 / meta 配对同删。
//!
//! 纪律对齐：断言**内容**（read_dir 枚举产物）而非目录存在性（Windows
//! 删除挂起幻影：名字仍可枚举的窗口内存在性断言会假红）；read_dir 有序
//! 消费在实现侧已显式排序，这里断言集合不含顺序。

use std::time::{Duration, SystemTime};

use super::{LogsTtl, is_expired, ledger_tail_is_append, sweep_workspace_logs};

fn now() -> SystemTime {
    SystemTime::now()
}

/// 过期时钟：now + 40 天（拨**前**）。
///
/// 方向说明：夹具 mtime = 真实当下、不可回拨（std 无稳定 API，不引
/// filetime），所以把 sweep 的 `now` 参数推进到未来——夹具相对它就是
/// 「40 天前」。拨反（now - 40d）会让 `duration_since(mtime)` 出 Err
/// 恒不超龄，全夹具零删除（首跑实录）。
fn old_now() -> SystemTime {
    SystemTime::now() + Duration::from_secs(40 * 86400)
}

/// 目录现存文件名集合（断言内容不断言存在性——Windows 幻影容忍）。
fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect(),
        Err(_) => Vec::new(), // 目录已不存在 = 空集（内容口径）
    };
    v.sort();
    v
}

// —— TTL 纯函数 ——

#[test]
fn ttl_is_expired_boundaries() {
    let t = now();
    assert!(!is_expired(t, 30, t), "零年龄不超龄");
    assert!(
        is_expired(t - Duration::from_secs(30 * 86400), 30, t),
        "恰好 30 天 = 超龄（>= 语义，边界不含糊）"
    );
    assert!(
        !is_expired(t - Duration::from_secs(30 * 86400 - 1), 30, t),
        "差一秒不满 30 天 = 保留"
    );
    assert!(
        !is_expired(t - Duration::from_secs(86400 * 365 * 10), 0, t),
        "ttl=0 = 族关，再老也不删"
    );
    // 时钟回拨形态（mtime 在未来）：duration_since 出错 = 不超龄。
    assert!(!is_expired(t + Duration::from_secs(3600), 7, t));
}

// —— 账本末行判定 ——

#[test]
fn ledger_tail_op_judgement() {
    assert!(ledger_tail_is_append(
        br#"{"op":"append","seq":1,"rows":[]}"#
    ));
    // raw byte string 不收非 ASCII，含中文载荷走 str.as_bytes()。
    assert!(!ledger_tail_is_append(
        r#"{"op":"truncate","seq":2,"rows":[{"role":"user","content_sha256":"ab","content":"全文旁证"}]}"#.as_bytes(),
    ));
    assert!(!ledger_tail_is_append(
        br#"{"op":"fork","seq":3,"rows":[]}"#
    ));
    assert!(!ledger_tail_is_append(b"not json at all"), "不可解析=保留");
    assert!(!ledger_tail_is_append(b""), "空文件=保留");
    assert!(!ledger_tail_is_append(b"\n\n"), "全空行=保留");
    // 末空行容忍 + 真实 LedgerEntry 形态（body flatten：op 在顶层，
    // 末行才算——前面的 truncate 行不遮蔽末行 append）。
    let multi = concat!(
        "{\"op\":\"truncate\",\"seq\":1,\"ts\":\"t\",\"actor\":\"a\",\"rows\":[],",
        "\"prev_hash\":\"h\",\"hash\":\"x\"}\n",
        "{\"op\":\"append\",\"seq\":2,\"ts\":\"t\",\"actor\":\"a\",\"rows\":[],",
        "\"prev_hash\":\"x\",\"hash\":\"y\"}\n",
    );
    assert!(ledger_tail_is_append(multi.as_bytes()), "以末行裁决");
}

// —— 夹具删留：平铺文件族（session_logs / boundary / terminal） ——

#[test]
fn flat_family_sweep_deletes_expired_keeps_fresh() {
    let ws = tempfile::tempdir().unwrap();
    let sl = ws.path().join("logs").join("session_logs");
    std::fs::create_dir_all(&sl).unwrap();
    std::fs::write(sl.join("old.jsonl"), "old body").unwrap();
    std::fs::write(sl.join("fresh.jsonl"), "fresh body").unwrap();
    std::fs::write(sl.join("old.meta.json"), "{}").unwrap();

    // 时钟拨老 40 天：三件夹具按 mtime 全部超 30 天 TTL。排序迭代下
    // fresh.jsonl → old.jsonl（连带配对同删 old.meta.json）→ old.meta.json
    // 已消失跳过；配对件计数与正文同权。
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl {
            session_logs: 30,
            boundary: 30,
            request_logs: 14,
            terminal: 7,
            event_ledger: 30,
            trash: 90,
            gateway: 14,
            cluster_logs: 14,
            eval: 30,
            capture: 30,
            dreaming: 30,
            calibration: 30,
            deliveries: 30,
            heartbeat: 30,
            ziwei: 30,
            exec_checkpoints: 1,
        },
        old_now(),
    );
    let session = stats
        .iter()
        .find(|(f, _)| f.name() == "session_logs")
        .unwrap()
        .1;
    assert_eq!(
        session.deleted, 3,
        "两正文 + 一配对 sidecar（old.meta.json 经 old.jsonl 配对）: {stats:?}"
    );
    assert_eq!(
        names(&sl),
        Vec::<String>::new(),
        "目录清空（内容口径，Windows 幻影容忍）"
    );
}

#[test]
fn half_expired_sweep_keeps_fresh_deletes_old() {
    let ws = tempfile::tempdir().unwrap();
    let sl = ws.path().join("logs").join("session_logs");
    std::fs::create_dir_all(&sl).unwrap();
    // 两个文件写入时间天然都是「现在」；时钟只拨 8 天 → terminal(7d)
    // 超龄、session_logs(30d) 未超龄 → session 一件不删、terminal 全删。
    let term = ws.path().join("logs").join("terminal");
    std::fs::create_dir_all(&term).unwrap();
    std::fs::write(term.join("pty_1.log"), "io").unwrap();
    std::fs::write(sl.join("live.jsonl"), "live").unwrap();

    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(8 * 86400),
    );
    let session = stats
        .iter()
        .find(|(f, _)| f.name() == "session_logs")
        .unwrap()
        .1;
    let terminal = stats
        .iter()
        .find(|(f, _)| f.name() == "terminal")
        .unwrap()
        .1;
    assert_eq!(session.deleted, 0, "8 天 < 30d：session_logs 一件不删");
    assert_eq!(terminal.deleted, 1, "8 天 >= 7d：terminal 删");
    assert_eq!(names(&sl), vec!["live.jsonl"], "未过期正文保留（内容口径）");
}

#[test]
fn session_meta_sidecar_pairs_with_body_deletion() {
    let ws = tempfile::tempdir().unwrap();
    let sl = ws.path().join("logs").join("session_logs");
    std::fs::create_dir_all(&sl).unwrap();
    // 正文旧、sidecar「新」在 mtime 口径下不可造（不引 filetime）——
    // 用 mtime 全旧的正文 + 限量窗口验证配对：正文删 → sidecar 无论
    // 自身年龄都跟走。这里直接验「配对同删」语义本身。
    std::fs::write(sl.join("gone.jsonl"), "body").unwrap();
    std::fs::write(sl.join("gone.meta.json"), "{}").unwrap();
    std::fs::write(sl.join("orphan.meta.json"), "{}").unwrap(); // 无正文孤儿：按自身年龄走常规判定

    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let session = stats
        .iter()
        .find(|(f, _)| f.name() == "session_logs")
        .unwrap()
        .1;
    assert_eq!(
        session.deleted, 3,
        "正文 + 配对 sidecar + 孤儿 sidecar 全删: {stats:?}"
    );
    assert_eq!(
        names(&sl),
        Vec::<String>::new(),
        "目录清空（内容口径，幻影容忍）"
    );
}

// —— request_logs：会话目录形态 ——

#[test]
fn request_logs_sweeps_session_dirs_whole() {
    let ws = tempfile::tempdir().unwrap();
    let rl = ws.path().join("logs").join("request_logs");
    // 目录族验证点 = 整目录删 + 内部文件计字节；「新鲜目录」形态无法与
    // 过期目录在单拍共存（时钟拨老对全夹具生效，mtime 不可回拨）——
    // 未过期保留语义由 half_expired 测试在 session/terminal 族覆盖。
    let a = rl.join("1729000000_aaaa");
    let b = rl.join("1729000001_bbbb");
    std::fs::create_dir_all(a.join("nested")).unwrap();
    std::fs::write(a.join("00.request.md"), "req").unwrap();
    std::fs::write(a.join("nested").join("01.AI.Response.raw.json"), "{}").unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("00.request.md"), "req").unwrap();

    // 时钟拨 20 天：request_logs(14d) 两目录都删；boundary(30d) 不动。
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(20 * 86400),
    );
    let req = stats
        .iter()
        .find(|(f, _)| f.name() == "request_logs")
        .unwrap()
        .1;
    assert_eq!(req.deleted, 2, "会话目录整删（含 nested）: {stats:?}");
    assert!(req.bytes_freed > 0, "释放字节按内部文件累计");
    assert_eq!(names(&rl), Vec::<String>::new(), "内容口径：目录族清空");
}

// —— cluster_logs：双层目录（{device}/{ts_ms}_{task}/，2026-11-06 补族） ——

#[test]
fn cluster_logs_two_level_sweeps_task_dirs_and_empty_devices() {
    let ws = tempfile::tempdir().unwrap();
    let cl = ws.path().join("logs").join("cluster_logs");
    let dev_a = cl.join("node-a");
    let dev_b = cl.join("node-b");
    let ta = dev_a.join("1729000000_100");
    let tb = dev_b.join("1729000001_101");
    std::fs::create_dir_all(&ta).unwrap();
    std::fs::create_dir_all(&tb).unwrap();
    std::fs::write(ta.join("00.request.md"), "req").unwrap();
    std::fs::write(tb.join("00.request.md"), "req").unwrap();
    // 意外平铺文件：保守不删。
    std::fs::write(dev_a.join("stray.txt"), "keep").unwrap();

    // 时钟拨 20 天：两任务目录(14d)全删；node-b 清空成空壳被摘除；
    // node-a 因 stray.txt 非空保留（意外平铺文件保守不动）。
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(20 * 86400),
    );
    let cl_stats = stats
        .iter()
        .find(|(f, _)| f.name() == "cluster_logs")
        .unwrap()
        .1;
    assert_eq!(cl_stats.deleted, 2, "两任务目录整删: {stats:?}");
    assert!(cl_stats.bytes_freed > 0, "释放字节按任务目录内容累计");
    assert_eq!(names(&dev_a), vec!["stray.txt"], "意外平铺文件保守保留");
    assert_eq!(
        names(&cl),
        vec!["node-a"],
        "node-b 空壳摘除；node-a 留（非空）"
    );

    // 二拍：清掉 stray 后 node-a 也成空壳被摘（零删除零 failed——空壳
    // 只是名字壳，摘除失败也不计 failed）。
    std::fs::remove_file(dev_a.join("stray.txt")).unwrap();
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let cl_stats = stats
        .iter()
        .find(|(f, _)| f.name() == "cluster_logs")
        .unwrap()
        .1;
    assert_eq!(cl_stats.deleted, 0);
    assert_eq!(cl_stats.failed, 0, "空壳摘除失败不产生 failed 计数");
    assert_eq!(
        names(&cl),
        Vec::<String>::new(),
        "空壳设备目录摘除（内容口径）"
    );
}

// —— gateway / eval：2026-11-06 横扫补族 ——

#[test]
fn gateway_and_eval_families_sweep() {
    let ws = tempfile::tempdir().unwrap();
    let gw = ws.path().join("logs").join("gateway");
    std::fs::create_dir_all(&gw).unwrap();
    // 真实命名形态：tracing-appender {prefix}.YYYY-MM-DD（无 .log 后缀，
    // common.rs 滚动前缀 = 文件 stem）。
    std::fs::write(gw.join("nemesisbot.2026-01-01"), "{}").unwrap();
    let ev = ws.path().join("logs").join("eval");
    let report = ev.join("1729000000_demo-skill");
    std::fs::create_dir_all(&report).unwrap();
    std::fs::write(report.join("assessment.json"), "{}").unwrap();

    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let gateway = stats.iter().find(|(f, _)| f.name() == "gateway").unwrap().1;
    let eval = stats.iter().find(|(f, _)| f.name() == "eval").unwrap().1;
    assert_eq!(
        gateway.deleted, 1,
        "滚动日志文件删（40d >= 14d）: {stats:?}"
    );
    assert_eq!(eval.deleted, 1, "评估报告目录整删（40d >= 30d）: {stats:?}");
    assert_eq!(names(&gw), Vec::<String>::new());
    assert_eq!(names(&ev), Vec::<String>::new());
}

// —— capture：双层目录（{session}/{ts}_{signal}/，2026-11-06 二轮补族） ——

#[test]
fn capture_two_level_sweeps_signal_dirs_and_empty_sessions() {
    let ws = tempfile::tempdir().unwrap();
    let cap = ws.path().join("logs").join("capture");
    let sess_a = cap.join("agent_main_session_aaaa");
    let sess_b = cap.join("agent_main_session_bbbb");
    let sig_a = sess_a.join("1729000000_tool_end");
    let sig_b = sess_b.join("1729000001_exec_fail");
    std::fs::create_dir_all(&sig_a).unwrap();
    std::fs::create_dir_all(&sig_b).unwrap();
    std::fs::write(sig_a.join("frame.json"), "{}").unwrap();
    std::fs::write(sig_b.join("frame.json"), "{}").unwrap();
    // 意外平铺文件：保守不删。
    std::fs::write(sess_a.join("stray.txt"), "keep").unwrap();

    // 时钟拨 40 天：两信号目录(30d)全删；sess_b 清空成空壳被摘除；
    // sess_a 因 stray.txt 非空保留（意外平铺文件保守不动）。
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let cap_stats = stats.iter().find(|(f, _)| f.name() == "capture").unwrap().1;
    assert_eq!(cap_stats.deleted, 2, "两信号目录整删: {stats:?}");
    assert_eq!(names(&sess_a), vec!["stray.txt"], "意外平铺文件保守保留");
    assert_eq!(
        names(&cap),
        vec!["agent_main_session_aaaa"],
        "sess_b 空壳摘除；sess_a 留（非空）"
    );

    // 二拍：清掉 stray 后 sess_a 也成空壳被摘（零删除零 failed——空壳
    // 只是名字壳，摘除失败也不计 failed）。
    std::fs::remove_file(sess_a.join("stray.txt")).unwrap();
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let cap_stats = stats.iter().find(|(f, _)| f.name() == "capture").unwrap().1;
    assert_eq!(cap_stats.deleted, 0);
    assert_eq!(cap_stats.failed, 0, "空壳摘除失败不产生 failed 计数");
    assert_eq!(
        names(&cap),
        Vec::<String>::new(),
        "空壳 session 目录摘除（内容口径）"
    );
}

// —— dreaming / calibration / deliveries：2026-11-06 二轮补族 ——

#[test]
fn dreaming_calibration_deliveries_families_sweep() {
    let ws = tempfile::tempdir().unwrap();
    let dr = ws.path().join("logs").join("dreaming");
    std::fs::create_dir_all(&dr).unwrap();
    std::fs::write(dr.join("sweep_1729000000.json"), "{}").unwrap();
    let cal = ws.path().join("logs").join("calibration");
    let report = cal.join("1729000000");
    std::fs::create_dir_all(&report).unwrap();
    std::fs::write(report.join("report.json"), "{}").unwrap();
    std::fs::write(report.join("report.md"), "# report").unwrap();
    let dv = ws.path().join("logs").join("deliveries");
    std::fs::create_dir_all(&dv).unwrap();
    std::fs::write(dv.join("task-42.patch"), "diff").unwrap();

    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let dreaming = stats
        .iter()
        .find(|(f, _)| f.name() == "dreaming")
        .unwrap()
        .1;
    let calibration = stats
        .iter()
        .find(|(f, _)| f.name() == "calibration")
        .unwrap()
        .1;
    let deliveries = stats
        .iter()
        .find(|(f, _)| f.name() == "deliveries")
        .unwrap()
        .1;
    assert_eq!(
        dreaming.deleted, 1,
        "dreaming 平铺报告文件删（40d >= 30d）: {stats:?}"
    );
    assert_eq!(calibration.deleted, 1, "校准报告目录整删: {stats:?}");
    assert!(calibration.bytes_freed > 0, "释放字节按目录内文件累计");
    assert_eq!(deliveries.deleted, 1, "patch 留底文件删: {stats:?}");
    assert_eq!(names(&dr), Vec::<String>::new());
    assert_eq!(names(&cal), Vec::<String>::new());
    assert_eq!(names(&dv), Vec::<String>::new());
}

// —— cluster_logs 根级 legacy 每日事件流：三轮补族 ——

#[test]
fn cluster_logs_legacy_daily_files_sweep_root_flat() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path().join("logs").join("cluster_logs");
    // legacy 每日事件流（cluster_log.rs 按日轮换形态）+ 未知平铺文件 +
    // 双层正常条目同根共存。
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("cluster_2026-10-01.log"), "legacy events").unwrap();
    std::fs::write(root.join("stray.bin"), "unknown form").unwrap();
    let device = root.join("Device-A").join("1729000000_task-1");
    std::fs::create_dir_all(&device).unwrap();
    std::fs::write(device.join("00.request.md"), "req").unwrap();

    // 拍 1（时钟拨老 40 天）：legacy 旧日文件删、未知平铺保守保留、
    // 双层任务目录整删。
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let cluster = stats
        .iter()
        .find(|(f, _)| f.name() == "cluster_logs")
        .unwrap()
        .1;
    assert_eq!(
        cluster.deleted, 2,
        "legacy 旧日文件 + 任务目录各计 1 件: {stats:?}"
    );
    assert_eq!(
        names(&root),
        vec!["stray.bin"],
        "根级只留未知形态（保守不动）"
    );

    // 拍 2：重建当日 legacy 文件（mtime = 现下）+ 时钟拨 8 天（< 14d
    // TTL）→ 保留零删（当日文件永不被误删——写方持久句柄按日轮换，
    // 旧日文件句柄已 drop，删除安全）。
    std::fs::write(root.join("cluster_2026-11-06.log"), "today").unwrap();
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(8 * 86400),
    );
    let cluster = stats
        .iter()
        .find(|(f, _)| f.name() == "cluster_logs")
        .unwrap()
        .1;
    assert_eq!(cluster.deleted, 0, "当日 legacy 文件新鲜保留: {stats:?}");
    assert_eq!(names(&root), vec!["cluster_2026-11-06.log", "stray.bin"]);
}

// —— heartbeat / ziwei / exec_checkpoints：三轮补族 ——

#[test]
fn heartbeat_ziwei_exec_checkpoints_families_sweep() {
    let ws = tempfile::tempdir().unwrap();
    let logs = ws.path().join("logs");
    // heartbeat 单文件 + ziwei 三根（notify.jsonl + reports 两棵树）+
    // exec_checkpoints 崩溃孤儿目录 + reports 用户点名输出（域外对照）。
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("heartbeat.log"), "beat").unwrap();
    std::fs::create_dir_all(logs.join("ziwei")).unwrap();
    std::fs::write(logs.join("ziwei").join("notify.jsonl"), "{\"t\":1}").unwrap();
    let verify = ws
        .path()
        .join("reports")
        .join("ziwei-verify")
        .join("prof-1");
    std::fs::create_dir_all(&verify).unwrap();
    std::fs::write(verify.join("verify_ab12cd34.json"), "{}").unwrap();
    let recover = ws
        .path()
        .join("reports")
        .join("ziwei-recover")
        .join("prof-1");
    std::fs::create_dir_all(&recover).unwrap();
    std::fs::write(recover.join("recover_1729000000.md"), "# recover").unwrap();
    std::fs::write(
        ws.path().join("reports").join("custom_export.jsonl"),
        "user-named deliverable",
    )
    .unwrap();
    let ecp = logs.join("exec_checkpoints").join("NB-101.git");
    std::fs::create_dir_all(ecp.join("objects")).unwrap();
    std::fs::write(ecp.join("objects").join("pack-1"), "git bytes").unwrap();

    // 拍 1（时钟拨老 40 天）：heartbeat 删 1、ziwei 删 3（jsonl + 两棵
    // profile 树）、exec_checkpoints 删 1（40d >= 1d 短闸）；用户点名
    // 输出 custom_export.jsonl 不在射程。
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let heartbeat = stats
        .iter()
        .find(|(f, _)| f.name() == "heartbeat")
        .unwrap()
        .1;
    let ziwei = stats.iter().find(|(f, _)| f.name() == "ziwei").unwrap().1;
    let exec_ck = stats
        .iter()
        .find(|(f, _)| f.name() == "exec_checkpoints")
        .unwrap()
        .1;
    assert_eq!(heartbeat.deleted, 1, "heartbeat.log 单文件删: {stats:?}");
    assert_eq!(
        ziwei.deleted, 3,
        "notify.jsonl + 两棵 profile 报告树各 1: {stats:?}"
    );
    assert_eq!(exec_ck.deleted, 1, "崩溃孤儿 .git 目录整删: {stats:?}");
    assert_eq!(heartbeat.failed + ziwei.failed + exec_ck.failed, 0);
    assert_eq!(
        names(&ws.path().join("reports")),
        vec!["custom_export.jsonl", "ziwei-recover", "ziwei-verify"],
        "profile 树清空、根空壳保留（与目录族同构）；{{dest}} 用户点名输出在"
    );
    assert_eq!(
        names(&verify),
        Vec::<String>::new(),
        "ziwei-verify 内容清空"
    );
    assert_eq!(
        names(&recover),
        Vec::<String>::new(),
        "ziwei-recover 内容清空"
    );
    assert_eq!(names(&logs.join("exec_checkpoints")), Vec::<String>::new());

    // 拍 2：重建 heartbeat.log + notify.jsonl（mtime = 现下）+ 时钟拨
    // 8 天（< 30d）→ 单文件/混合根零删保留。exec_checkpoints TTL=1
    // 短闸不重建（8d 时钟下新鲜夹具也超龄 = 短闸快清的设计语义，拍 1
    // 已验删面）。
    std::fs::write(logs.join("heartbeat.log"), "beat").unwrap();
    std::fs::write(logs.join("ziwei").join("notify.jsonl"), "{\"t\":2}").unwrap();
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(8 * 86400),
    );
    let heartbeat = stats
        .iter()
        .find(|(f, _)| f.name() == "heartbeat")
        .unwrap()
        .1;
    let ziwei = stats.iter().find(|(f, _)| f.name() == "ziwei").unwrap().1;
    assert_eq!(heartbeat.deleted, 0, "新鲜 heartbeat.log 保留: {stats:?}");
    assert_eq!(ziwei.deleted, 0, "新鲜 notify.jsonl 保留: {stats:?}");
    assert!(
        logs.join("heartbeat.log").is_file(),
        "heartbeat.log 原样在（拍 2 从未删过，无幻影窗口）"
    );
    assert_eq!(names(&logs.join("ziwei")), vec!["notify.jsonl"]);
}

// —— event_ledger：超龄 且 末行 append ——

#[test]
fn event_ledger_deletes_only_append_tails() {
    let ws = tempfile::tempdir().unwrap();
    let el = ws.path().join("logs").join("event_ledger");
    std::fs::create_dir_all(&el).unwrap();
    std::fs::write(
        el.join("append_tail.jsonl"),
        "{\"op\":\"append\",\"seq\":1,\"ts\":\"t\",\"actor\":\"a\",\"rows\":[],\"prev_hash\":\"h\",\"hash\":\"x\"}\n",
    )
    .unwrap();
    std::fs::write(
        el.join("truncate_tail.jsonl"),
        "{\"op\":\"truncate\",\"seq\":2,\"ts\":\"t\",\"actor\":\"a\",\"rows\":[{\"role\":\"user\",\"content_sha256\":\"ab\",\"content\":\"唯一旁证\"}],\"prev_hash\":\"x\",\"hash\":\"y\"}\n",
    )
    .unwrap();
    std::fs::write(el.join("garbage.jsonl"), "不是账本").unwrap();

    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let ledger = stats
        .iter()
        .find(|(f, _)| f.name() == "event_ledger")
        .unwrap()
        .1;
    assert_eq!(ledger.deleted, 1, "只有末行 append 的账本删: {stats:?}");
    assert_eq!(
        names(&el),
        vec!["garbage.jsonl", "truncate_tail.jsonl"],
        "truncate 旁证与不可解析账本保留（内容口径）"
    );
}

// —— 全 0 = 完全不扫 ——

#[test]
fn all_zero_ttl_sweeps_nothing() {
    let ws = tempfile::tempdir().unwrap();
    let sl = ws.path().join("logs").join("session_logs");
    std::fs::create_dir_all(&sl).unwrap();
    std::fs::write(sl.join("ancient.jsonl"), "body").unwrap();

    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::all_disabled_shape(), old_now());
    assert!(
        stats.iter().all(|(_, s)| s.deleted == 0 && s.failed == 0),
        "十六族全 0 = 零触碰: {stats:?}"
    );
    assert_eq!(names(&sl), vec!["ancient.jsonl"], "夹具原样保留");
}

// —— trash：回收站条目目录（拍板① 90d） ——

#[test]
fn trash_family_sweeps_entry_dirs_and_outlives_other_families() {
    let ws = tempfile::tempdir().unwrap();
    // 条目形态 = chat_trash 真实布局：logs/trash/sessions/{ms}_{safe}/
    // 内 chat.jsonl + trash.json 清单。
    let tr = ws.path().join("logs").join("trash").join("sessions");
    let entry = tr.join("1729000000_agent_main_session_aaaa");
    std::fs::create_dir_all(&entry).unwrap();
    std::fs::write(
        entry.join("chat.jsonl"),
        "{\"role\":\"user\",\"content\":\"x\"}\n",
    )
    .unwrap();
    std::fs::write(entry.join("trash.json"), "{}").unwrap();

    // 时钟拨 40 天：其余族全部超龄（默认 7~30d），trash(90d) 未超龄——
    // 「回收站保留期长于所有日志族」的拍板语义在此被单独钉住。
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(40 * 86400),
    );
    let trash = stats.iter().find(|(f, _)| f.name() == "trash").unwrap().1;
    assert_eq!(trash.deleted, 0, "40 天 < 90d：回收站一件不删: {stats:?}");
    assert_eq!(
        names(&tr),
        vec!["1729000000_agent_main_session_aaaa"],
        "条目保留（内容口径）"
    );

    // 时钟拨 100 天：trash 超龄 → 整目录删（清单 + 三件套副本同走）。
    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(100 * 86400),
    );
    let trash = stats.iter().find(|(f, _)| f.name() == "trash").unwrap().1;
    assert_eq!(trash.deleted, 1, "100 天 >= 90d：条目整目录删: {stats:?}");
    assert!(trash.bytes_freed > 0, "释放字节按条目内文件累计");
    assert_eq!(names(&tr), Vec::<String>::new(), "内容口径：回收站清空");
}

/// trash 族 TTL 删除必须走 trash_purge_in 单点：带清单条目删除时把
/// sessions/ 下 live todo 孤儿一并清掉（修复前入站的旧形态条目无 todo
/// 副本，孤儿只能靠 purge 收敛）；无清单条目 = 原键不可信寻址，孤儿
/// 诚实保留（purge 同语义）。孤儿寻址用 nemesis-path 真相源落位。
#[test]
fn trash_sweep_purges_live_todo_orphan_with_entry() {
    let ws = tempfile::tempdir().unwrap();
    let tr = ws.path().join("logs").join("trash").join("sessions");
    let sessions = nemesis_path::resolve_sessions_dir_in_workspace(ws.path());

    // 条目 A：带清单（旧形态——无 todo 副本在条目内）+ sessions/ 下
    // live 孤儿。清单指向的键是唯一寻址线索。
    let entry_a = tr.join("1729000000_agent_main_session_legacy");
    std::fs::create_dir_all(&entry_a).unwrap();
    std::fs::write(entry_a.join("chat.jsonl"), "x").unwrap();
    std::fs::write(
        entry_a.join("trash.json"),
        r#"{"original_session_key":"agent:main:session:legacy","deleted_at_ms":1729000000}"#,
    )
    .unwrap();
    let orphan_a =
        nemesis_path::session_todo_file_in_workspace(ws.path(), "agent:main:session:legacy");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(&orphan_a, "[]").unwrap();

    // 条目 B：无清单（孤儿目录形态）。任何 live 孤儿都无法经它寻址。
    let entry_b = tr.join("1729000001_agent_main_session_ghost");
    std::fs::create_dir_all(&entry_b).unwrap();
    std::fs::write(entry_b.join("chat.jsonl"), "x").unwrap();
    let orphan_b =
        nemesis_path::session_todo_file_in_workspace(ws.path(), "agent:main:session:ghost");
    std::fs::write(&orphan_b, "[]").unwrap();

    let stats = sweep_workspace_logs(
        ws.path(),
        &LogsTtl::default(),
        SystemTime::now() + Duration::from_secs(100 * 86400),
    );
    let trash = stats.iter().find(|(f, _)| f.name() == "trash").unwrap().1;
    assert_eq!(trash.deleted, 2, "两条目都超 90d：全删: {stats:?}");
    assert_eq!(names(&tr), Vec::<String>::new(), "回收站清空（内容口径）");
    // A 的孤儿随条目收敛消失；B 无清单不可寻址，孤儿诚实保留。
    assert!(!orphan_a.exists(), "带清单条目的 live todo 孤儿随删收敛");
    assert!(
        orphan_b.exists(),
        "无清单条目不可寻址：孤儿保留（purge 同语义）"
    );
}

// —— 限量：单族单拍不超过 200 件 ——

#[test]
fn per_sweep_cap_bounds_deletions() {
    let ws = tempfile::tempdir().unwrap();
    let term = ws.path().join("logs").join("terminal");
    std::fs::create_dir_all(&term).unwrap();
    for i in 0..230 {
        std::fs::write(term.join(format!("pty_{i:04}.log")), "io").unwrap();
    }
    let stats = sweep_workspace_logs(ws.path(), &LogsTtl::default(), old_now());
    let terminal = stats
        .iter()
        .find(|(f, _)| f.name() == "terminal")
        .unwrap()
        .1;
    assert_eq!(terminal.deleted, 200, "单拍限量 200");
    assert_eq!(names(&term).len(), 30, "余量交下拍（内容口径）");
}

impl LogsTtl {
    /// 测试助记：十六族全 0 形态。
    fn all_disabled_shape() -> Self {
        Self {
            session_logs: 0,
            boundary: 0,
            request_logs: 0,
            terminal: 0,
            event_ledger: 0,
            trash: 0,
            gateway: 0,
            cluster_logs: 0,
            eval: 0,
            capture: 0,
            dreaming: 0,
            calibration: 0,
            deliveries: 0,
            heartbeat: 0,
            ziwei: 0,
            exec_checkpoints: 0,
        }
    }
}

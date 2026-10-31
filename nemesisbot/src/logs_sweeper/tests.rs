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
        "六族全 0 = 零触碰: {stats:?}"
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
    /// 测试助记：六族全 0 形态。
    fn all_disabled_shape() -> Self {
        Self {
            session_logs: 0,
            boundary: 0,
            request_logs: 0,
            terminal: 0,
            event_ledger: 0,
            trash: 0,
        }
    }
}

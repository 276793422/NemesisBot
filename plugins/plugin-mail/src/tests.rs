//! 插件内测试：store 生命周期 / 地址闸 / SMTP e2e（两级判定 + 拒中继 +
//! 超限 552 + STARTTLS 负例 + 反垃圾闸 + idle 超时）/ POP3 e2e（USER 映射 +
//! 拒绝 fail-open 锁定 + 并集 UIDL 前缀 + RETR 字节一致 + DELE+QUIT 真删 +
//! idle 超时）/ 启动准入闸四态 / TLS 装配 fail-closed。
//!
//! 诚实边界：TLS 握手正例（POP3S / STARTTLS 升级）不做进程内 e2e——需要
//! 证书链夹具（rcgen 非依赖），真机 UAT 用 openssl 两级链覆盖；协议编码函数
//! 已由 nemesis-channels 434 套件（email 通道同源路径）覆盖；此处只覆盖
//! store/服务端/闸面。

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::client::ClientSettings;
use crate::pop3_server::{Pop3Ctx, Pop3Settings};
use crate::smtp_server::{SmtpCtx, SmtpSettings};
use crate::store::{MailQuery, MailStore, validate_address};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn temp_store() -> (tempfile::TempDir, Arc<MailStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(MailStore::new(dir.path().to_path_buf()));
    (dir, store)
}

fn null_host() -> Arc<crate::MailHost> {
    Arc::new(crate::MailHost {
        audit_fn: None,
        notify_in_fn: None,
        notify_sent_fn: None,
    })
}

/// 测试本机域（每次调用生成 Vec<String>——addr_add 签名是 &[String]）。
fn domains() -> Vec<String> {
    vec!["corp.example".to_string()]
}

// ---------------------------------------------------------------------------
// store 单测
// ---------------------------------------------------------------------------

#[test]
fn address_validation_gate() {
    assert!(validate_address("zoo@corp.example").is_ok());
    assert!(validate_address("a.b+c_d@x-y.io").is_ok());
    assert!(validate_address("").is_err());
    assert!(validate_address("no-at-sign").is_err());
    assert!(validate_address("@corp.example").is_err());
    assert!(validate_address("local@").is_err());
    assert!(validate_address("a..b@corp.example").is_err()); // 路径穿越面
    assert!(validate_address("a/b@corp.example").is_err()); // 路径分隔符
    assert!(validate_address("a\\b@corp.example").is_err());
    assert!(validate_address("a:b@corp.example").is_err()); // Windows 文件名禁
    assert!(validate_address("a b@corp.example").is_err()); // 空白
    let long = format!("{}@corp.example", "x".repeat(250));
    assert!(validate_address(&long).is_err()); // >254
}

#[test]
fn addr_registration_domain_gate_and_idempotency() {
    let (_dir, store) = temp_store();
    // 域不在 local_domains = 拒。
    assert!(store.addr_add("zoo@other.com", &domains()).is_err());
    assert!(store.addr_add("zoo@corp.example", &domains()).is_ok());
    // 幂等重注册。
    assert!(store.addr_add("zoo@corp.example", &domains()).is_ok());
    let addrs = store.list_addresses().unwrap();
    assert_eq!(addrs.len(), 1);
    // 默认主库 = 首个注册地址。
    assert_eq!(store.default_mailbox().unwrap(), "zoo@corp.example");
    // 未注册地址注销 = 诚实 Err。
    assert!(store.addr_remove("ghost@corp.example").is_err());
}

#[test]
fn addr_remove_stops_delivery_but_keeps_history() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    store
        .deliver(
            "zoo@corp.example",
            "From: a@b.c\r\nSubject: t\r\n\r\nhi\r\n",
        )
        .unwrap();
    // 注销 = 摘注册表，库目录与历史信保留。
    store.addr_remove("zoo@corp.example").unwrap();
    assert!(store.list_addresses().unwrap().is_empty());
    let dir = store.root().join("zoo@corp.example").join("new");
    assert!(
        dir.read_dir().map(|d| d.count()).unwrap_or(0) >= 1,
        "历史信必须保留"
    );
    // 重新注册自动归位（信还在库里）。
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let q = MailQuery::default();
    assert_eq!(store.list(&q).unwrap().len(), 1);
}

#[test]
fn deliver_list_and_unread_filter() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    store
        .deliver(
            "zoo@corp.example",
            "From: a@x.com\r\nSubject: first\r\n\r\n1\r\n",
        )
        .unwrap();
    store
        .deliver(
            "zoo@corp.example",
            "From: b@x.com\r\nSubject: second\r\n\r\n2\r\n",
        )
        .unwrap();
    let q = MailQuery::default();
    let items = store.list(&q).unwrap();
    assert_eq!(items.len(), 2);
    // 新信在前（id 降序）。
    assert_eq!(items[0].subject, "second");
    assert!(!items[0].seen);
    // 只看未读。
    let q_unread = MailQuery {
        unread_only: true,
        ..Default::default()
    };
    assert_eq!(store.list(&q_unread).unwrap().len(), 2);
    // 已读平移后 unread_only 过滤生效。
    let id = items[0].id.clone();
    store.mark_read(&id).unwrap();
    assert_eq!(store.list(&q_unread).unwrap().len(), 1);
    // seen 旗标翻转。
    let items = store.list(&q).unwrap();
    let read_item = items.iter().find(|i| i.id == id).unwrap();
    assert!(read_item.seen);
}

#[test]
fn read_and_delete_roundtrip() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let id = store
        .deliver(
            "zoo@corp.example",
            "From: a@x.com\r\nSubject: body-check\r\n\r\npayload\r\n",
        )
        .unwrap();
    let (meta, body) = store.read(&id).unwrap();
    assert_eq!(meta.subject, "body-check");
    assert_eq!(meta.mailbox, "zoo@corp.example");
    assert!(body.contains("payload"), "正文不含头块：{body:?}");
    store.delete(&id).unwrap();
    assert!(store.read(&id).is_err());
}

#[test]
fn path_traversal_id_rejected() {
    let (_dir, store) = temp_store();
    assert!(store.read("../escape").is_err());
    assert!(store.delete("a/b").is_err());
}

#[test]
fn list_rejects_unregistered_mailbox() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let q = MailQuery {
        mailbox: Some("ghost@corp.example".into()),
        ..Default::default()
    };
    assert!(store.list(&q).is_err(), "只读已注册库");
}

#[test]
fn record_dedup_by_message_id() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let meta = serde_json::json!({
        "mailbox": "zoo@corp.example",
        "from": "ext@other.com",
        "to": "zoo@corp.example",
        "subject": "外部纳管",
        "message_id": "<mid-1@other.com>",
        "body": "hello",
    });
    let first = store.record(&meta).unwrap();
    assert_eq!(first.get("deduped"), Some(&serde_json::json!(false)));
    let second = store.record(&meta).unwrap();
    assert_eq!(
        second.get("deduped"),
        Some(&serde_json::json!(true)),
        "message_id 去重"
    );
    let q = MailQuery::default();
    assert_eq!(store.list(&q).unwrap().len(), 1, "去重后不产生第二封");
}

#[test]
fn record_strips_crlf_header_injection() {
    // BUG-20261120-039 横向同族回归锁：meta 经 C-ABI mail_record 直达，
    // from/subject/message_id 混入 CR/LF 不得在本地 .eml 头区断行注入头
    //（头区每行必须是已知字段；正文区不受影响）。
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let meta = serde_json::json!({
        "mailbox": "zoo@corp.example",
        "from": "ext@other.com\r\nX-Inject: pwned-from",
        "to": "zoo@corp.example",
        "subject": "注入\r\nX-Inject: pwned-subject",
        "message_id": "<mid-x@other.com\r\nX-Inject: pwned-mid>",
        "body": "plain body",
    });
    let out = store.record(&meta).unwrap();
    assert_eq!(out.get("ok"), Some(&serde_json::json!(true)));
    let id = out.get("id").and_then(|v| v.as_str()).expect("id");
    // 直读原始 .eml（read() 返回的是 body 侧，头区断言需原始文件）：
    // <root>/<mailbox>/new/<id>.eml
    let eml_path = _dir
        .path()
        .join("zoo@corp.example")
        .join("new")
        .join(format!("{id}.eml"));
    let eml = std::fs::read_to_string(&eml_path).expect("eml file");
    let header_zone = eml.split("\r\n\r\n").next().unwrap_or("");
    let known = ["From: ", "To: ", "Subject: ", "Date: ", "Message-ID: "];
    for line in header_zone.split("\r\n") {
        assert!(
            known.iter().any(|p| line.starts_with(p)),
            "unexpected header line (injection?): {line:?}"
        );
    }
    assert!(eml.contains("plain body"), "body intact");
}

// ---------------------------------------------------------------------------
// SMTP e2e
// ---------------------------------------------------------------------------

async fn smtp_session(port: u16) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
    let s = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let (r, w) = s.into_split();
    (BufReader::new(r), w)
}

async fn recv_line(r: &mut BufReader<OwnedReadHalf>) -> String {
    let mut line = String::new();
    r.read_line(&mut line).await.expect("read_line");
    line.trim_end().to_string()
}

async fn send_line(w: &mut OwnedWriteHalf, line: &str) {
    w.write_all(format!("{line}\r\n").as_bytes())
        .await
        .expect("write");
}

async fn spawn_smtp_full(
    store: Arc<MailStore>,
    settings: SmtpSettings,
) -> (u16, tokio::sync::watch::Sender<bool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let ctx = Arc::new(SmtpCtx {
        store,
        host: null_host(),
    });
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(crate::smtp_server::run(
        listener,
        ctx,
        Arc::new(settings),
        None, // 无 acceptor = 明文面（TLS 正例走真机 UAT，负例见 502 测试）
        rx,
    ));
    // sender 必须由调用方持有（活到测试结束）——夹具局部绑定会在返回时
    // drop，run() 的 changed() 收 Err 立即退出 → listener 关闭 → RST。
    (port, tx)
}

async fn spawn_smtp(
    store: Arc<MailStore>,
    max_bytes: u64,
) -> (u16, tokio::sync::watch::Sender<bool>) {
    spawn_smtp_full(
        store,
        SmtpSettings {
            enabled: true,
            local_domains: domains(),
            max_message_bytes: max_bytes,
            ..Default::default()
        },
    )
    .await
}

#[tokio::test]
async fn smtp_registered_delivery_and_two_level_gate() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let (port, _tx) = spawn_smtp(store.clone(), 10 * 1024 * 1024).await;

    let (mut r, mut w) = smtp_session(port).await;
    assert!(recv_line(&mut r).await.starts_with("220"));

    send_line(&mut w, "EHLO tester").await;
    // EHLO 多行应答：末行 250（无连字符）——两行都消费。
    assert!(recv_line(&mut r).await.starts_with("250-"));
    assert!(recv_line(&mut r).await.starts_with("250 "));

    send_line(&mut w, "MAIL FROM:<ext@other.com>").await;
    assert!(recv_line(&mut r).await.starts_with("250"));

    // 第一级（域不在 = 拒中继）。
    send_line(&mut w, "RCPT TO:<someone@else.net>").await;
    let resp = recv_line(&mut r).await;
    assert!(
        resp.starts_with("550") && resp.contains("relay access denied"),
        "拒中继：{resp}"
    );

    // 第二级（域在内但地址未注册 = 拒）。
    send_line(&mut w, "RCPT TO:<unknown@corp.example>").await;
    let resp = recv_line(&mut r).await;
    assert!(
        resp.starts_with("550") && resp.contains("not registered"),
        "注册闸：{resp}"
    );

    // 双关都过 = 收。
    send_line(&mut w, "RCPT TO:<zoo@corp.example>").await;
    assert!(recv_line(&mut r).await.starts_with("250"));

    send_line(&mut w, "DATA").await;
    assert!(recv_line(&mut r).await.starts_with("354"));
    // 载荷带行首点（客户端侧反点填充——线上形态行首点双写）。
    w.write_all(b"From: ext@other.com\r\nTo: zoo@corp.example\r\nSubject: gate-test\r\n\r\n..leading dot line\r\n..plain\r\n")
        .await
        .expect("payload");
    send_line(&mut w, ".").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("250"), "DATA 应答：{resp}");

    // 信进了对应库 + 反点还原（正文以单个点开头）。
    let q = MailQuery {
        mailbox: Some("zoo@corp.example".into()),
        ..Default::default()
    };
    let items = store.list(&q).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].subject, "gate-test");
    let (_meta, body) = store.read(&items[0].id).unwrap();
    // 反点还原成立 = 客户端双写 `..leading` 已剥成单点（正文首行形态）+
    // 全文无双点残留（`..` 若在场说明服务端没剥）。
    assert!(
        body.starts_with(".leading dot line\r\n"),
        "反点未还原：{body:?}"
    );
    assert!(!body.contains(".."), "反点未剥离（双点残留）：{body:?}");

    // 小写动词形态（strip_ci 路径）。
    send_line(&mut w, "mail from:<ext@other.com>").await;
    assert!(recv_line(&mut r).await.starts_with("250"));
    send_line(&mut w, "rcpt to:<zoo@corp.example>").await;
    assert!(recv_line(&mut r).await.starts_with("250"), "小写动词须等价");

    send_line(&mut w, "QUIT").await;
    assert!(recv_line(&mut r).await.starts_with("221"));
}

#[tokio::test]
async fn smtp_oversize_rejected_552() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    // 64 字节上限。
    let (port, _tx) = spawn_smtp(store, 64).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await; // 220
    send_line(&mut w, "EHLO t").await;
    let _ = recv_line(&mut r).await; // 250-
    let _ = recv_line(&mut r).await; // 250 末行
    send_line(&mut w, "MAIL FROM:<a@b.c>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "RCPT TO:<zoo@corp.example>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "DATA").await;
    let _ = recv_line(&mut r).await; // 354

    // 超限载荷（两行即破 64 字节闸）。
    w.write_all(b"From: a@b.c\r\nSubject: big\r\n\r\n")
        .await
        .expect("payload head");
    w.write_all([b'x'; 200].as_slice())
        .await
        .expect("payload body");
    send_line(&mut w, ".").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("552"), "超限应 552：{resp}");
}

#[tokio::test]
async fn smtp_multi_rcpt_delivers_to_each_mailbox() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    store.addr_add("bot@corp.example", &domains()).unwrap();
    let (port, _tx) = spawn_smtp(store.clone(), 10 * 1024 * 1024).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "EHLO t").await;
    let _ = recv_line(&mut r).await; // 250-
    let _ = recv_line(&mut r).await; // 250 末行
    send_line(&mut w, "MAIL FROM:<a@b.c>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "RCPT TO:<zoo@corp.example>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "RCPT TO:<bot@corp.example>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "DATA").await;
    let _ = recv_line(&mut r).await; // 354
    w.write_all(b"From: a@b.c\r\nSubject: fanout\r\n\r\nx\r\n")
        .await
        .expect("payload");
    send_line(&mut w, ".").await;
    assert!(recv_line(&mut r).await.starts_with("250"));

    for mailbox in ["zoo@corp.example", "bot@corp.example"] {
        let q = MailQuery {
            mailbox: Some(mailbox.into()),
            ..Default::default()
        };
        let items = store.list(&q).unwrap();
        assert_eq!(items.len(), 1, "{mailbox} 应各收一封");
        assert_eq!(items[0].subject, "fanout");
    }
}

#[tokio::test]
async fn smtp_starttls_unavailable_without_cert() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let (port, _tx) = spawn_smtp(store, 10 * 1024 * 1024).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await; // 220
    send_line(&mut w, "EHLO t").await;
    // 无 acceptor：EHLO 两行、不含 STARTTLS 广告（opportunistic 不虚报）。
    let l1 = recv_line(&mut r).await;
    let l2 = recv_line(&mut r).await;
    assert!(
        !l1.contains("STARTTLS") && !l2.contains("STARTTLS"),
        "无证书不得广告 STARTTLS：{l1} / {l2}"
    );
    // 客户端强发 STARTTLS = 502（能力不存在）。
    send_line(&mut w, "STARTTLS").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("502"), "无证书 STARTTLS 应 502：{resp}");
}

#[tokio::test]
async fn smtp_max_messages_gate_closes_connection() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let settings = SmtpSettings {
        enabled: true,
        local_domains: domains(),
        max_messages_per_connection: 1,
        ..Default::default()
    };
    let (port, _tx) = spawn_smtp_full(store, settings).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await; // 220
    send_line(&mut w, "EHLO t").await;
    let _ = recv_line(&mut r).await;
    let _ = recv_line(&mut r).await;
    // 第一封：正常收（闸限 = 1）。
    send_line(&mut w, "MAIL FROM:<a@b.c>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "RCPT TO:<zoo@corp.example>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "DATA").await;
    let _ = recv_line(&mut r).await; // 354
    w.write_all(b"Subject: one\r\n\r\nx\r\n")
        .await
        .expect("payload");
    send_line(&mut w, ".").await;
    assert!(recv_line(&mut r).await.starts_with("250"), "第一封应正常收");

    // 第二封：闸到限 → DATA 452 + 服务端主动断开（刷信灌库面收口）。
    send_line(&mut w, "MAIL FROM:<a@b.c>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "RCPT TO:<zoo@corp.example>").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "DATA").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("452"), "超限应 452：{resp}");
    let mut line = String::new();
    let n = r.read_line(&mut line).await.unwrap_or(0);
    assert_eq!(n, 0, "闸断开后应 EOF");
}

#[tokio::test]
async fn smtp_rcpt_failures_gate_closes_connection() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let settings = SmtpSettings {
        enabled: true,
        local_domains: domains(),
        max_rcpt_failures: 2,
        ..Default::default()
    };
    let (port, _tx) = spawn_smtp_full(store, settings).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "EHLO t").await;
    let _ = recv_line(&mut r).await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "MAIL FROM:<a@b.c>").await;
    let _ = recv_line(&mut r).await;

    // 前两次失败 = 550（正常拒收语义）；第三次 = 421 + 断开（地址枚举面收口）。
    send_line(&mut w, "RCPT TO:<x@else.net>").await;
    assert!(recv_line(&mut r).await.starts_with("550"));
    send_line(&mut w, "RCPT TO:<x@else.net>").await;
    assert!(recv_line(&mut r).await.starts_with("550"));
    send_line(&mut w, "RCPT TO:<x@else.net>").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("421"), "第三次失败应 421 断开：{resp}");
    let mut line = String::new();
    let n = r.read_line(&mut line).await.unwrap_or(0);
    assert_eq!(n, 0, "421 后应 EOF");
}

#[tokio::test]
async fn smtp_idle_timeout_closes_session() {
    let (_dir, store) = temp_store();
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    let settings = SmtpSettings {
        enabled: true,
        local_domains: domains(),
        idle_timeout_secs: 1,
        ..Default::default()
    };
    let (port, _tx) = spawn_smtp_full(store, settings).await;

    let (mut r, mut w) = smtp_session(port).await;
    let _ = recv_line(&mut r).await; // 220
    // 静默 1.5s（> 1s 闸）——服务端 idle 超时收口；后续写可能因 RST 失败、
    // 读必 EOF——两种形态都算连接已收口。
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let write_ok = w.write_all(b"NOOP\r\n").await;
    let mut line = String::new();
    let n = r.read_line(&mut line).await.unwrap_or(0);
    assert!(
        write_ok.is_err() || n == 0,
        "idle 超时后连接应收口（write_ok={write_ok:?} n={n}）"
    );
}

// ---------------------------------------------------------------------------
// POP3 e2e
// ---------------------------------------------------------------------------

async fn spawn_pop3(
    store: Arc<MailStore>,
    password: &str,
) -> (u16, tokio::sync::watch::Sender<bool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let ctx = Arc::new(Pop3Ctx {
        store,
        host: null_host(),
    });
    let settings = Arc::new(Pop3Settings {
        enabled: true,
        password: password.to_string(),
        ..Default::default()
    });
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(crate::pop3_server::run(
        listener, ctx, settings,
        None, // 无 acceptor = 明文面（回环测试形态；闸逻辑见 admission 单测）
        rx,
    ));
    // sender 归调用方持有（理由同 spawn_smtp）。
    (port, tx)
}

async fn pop3_session(port: u16) -> (BufReader<OwnedReadHalf>, OwnedWriteHalf) {
    let s = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let (r, w) = s.into_split();
    (BufReader::new(r), w)
}

/// 读多行应答体（终止于单独 `.` 行；含反点还原）。
async fn read_multiline(r: &mut BufReader<OwnedReadHalf>) -> String {
    let mut out = String::new();
    loop {
        let line = recv_line(r).await;
        if line == "." {
            break;
        }
        if let Some(stripped) = line.strip_prefix("..") {
            out.push('.');
            out.push_str(stripped);
        } else {
            out.push_str(&line);
        }
        out.push('\n');
    }
    out
}

fn seed_two_mailboxes(store: &MailStore) {
    store.addr_add("zoo@corp.example", &domains()).unwrap();
    store.addr_add("bot@corp.example", &domains()).unwrap();
    store
        .deliver(
            "zoo@corp.example",
            "From: a@x.com\r\nSubject: for-zoo\r\n\r\nzoo-body\r\n",
        )
        .unwrap();
    store
        .deliver(
            "bot@corp.example",
            "From: b@x.com\r\nSubject: for-bot\r\n\r\nbot-body\r\n",
        )
        .unwrap();
}

#[tokio::test]
async fn pop3_user_maps_to_own_mailbox() {
    let (_dir, store) = temp_store();
    seed_two_mailboxes(&store);
    let (port, _tx) = spawn_pop3(store.clone(), "").await;

    let (mut r, mut w) = pop3_session(port).await;
    assert!(recv_line(&mut r).await.starts_with("+OK"));

    send_line(&mut w, "USER zoo@corp.example").await;
    assert!(recv_line(&mut r).await.starts_with("+OK"));
    send_line(&mut w, "STAT").await;
    let resp = recv_line(&mut r).await;
    assert!(
        resp.starts_with("+OK 1 "),
        "USER 后只见本库（1 封）：{resp}"
    );

    // 指名未注册地址 = -ERR，且后续命令被锁（fail-open 防线：不许静默回落并集）。
    send_line(&mut w, "USER ghost@corp.example").await;
    assert!(recv_line(&mut r).await.starts_with("-ERR"));
    send_line(&mut w, "STAT").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("-ERR"), "拒绝后不得回落并集视图：{resp}");

    // 空 USER 回到并集形态。
    send_line(&mut w, "USER").await;
    assert!(recv_line(&mut r).await.starts_with("+OK"));
    send_line(&mut w, "STAT").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("+OK 2 "), "并集 = 两库合计：{resp}");
}

#[tokio::test]
async fn pop3_uidl_carries_mailbox_prefix_and_retr_bytes_match() {
    let (_dir, store) = temp_store();
    seed_two_mailboxes(&store);
    let (port, _tx) = spawn_pop3(store.clone(), "").await;

    let (mut r, mut w) = pop3_session(port).await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "UIDL").await;
    let _ = recv_line(&mut r).await; // +OK
    let body = read_multiline(&mut r).await;
    assert!(
        body.contains("zoo@corp.example/"),
        "UIDL 带库前缀：{body:?}"
    );
    assert!(
        body.contains("bot@corp.example/"),
        "UIDL 带库前缀：{body:?}"
    );

    // RETR 1（升序第一封 = zoo 的）逐字节核对。
    send_line(&mut w, "RETR 1").await;
    let head = recv_line(&mut r).await;
    assert!(head.starts_with("+OK"), "RETR 应答：{head}");
    let payload = read_multiline(&mut r).await;
    assert!(
        payload.contains("Subject: for-zoo"),
        "RETR 载荷：{payload:?}"
    );
    assert!(payload.contains("zoo-body"), "RETR 正文：{payload:?}");
    // 信封来源与盘上 .eml 同源。
    assert!(payload.contains("From: a@x.com"));
}

#[tokio::test]
async fn pop3_dele_quit_really_deletes() {
    let (_dir, store) = temp_store();
    seed_two_mailboxes(&store);
    let (port, _tx) = spawn_pop3(store.clone(), "").await;

    let (mut r, mut w) = pop3_session(port).await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "USER").await; // 并集
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "STAT").await;
    let _ = recv_line(&mut r).await; // +OK 2 ...
    send_line(&mut w, "DELE 1").await;
    assert!(recv_line(&mut r).await.starts_with("+OK"));
    // RSET 撤标记后再 STAT 仍 2。
    send_line(&mut w, "RSET").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "STAT").await;
    assert!(recv_line(&mut r).await.starts_with("+OK 2 "));
    // DELE + QUIT = 真删。
    send_line(&mut w, "DELE 1").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "QUIT").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("+OK"), "QUIT：{resp}");

    // 会话结束后盘上：DELE 1 删的是升序第一封（时间戳早 = zoo 的信），
    // 剩余信在 bot 库（list 默认主库查 zoo = 空，须点名查 bot）。
    let q_zoo = MailQuery {
        mailbox: Some("zoo@corp.example".into()),
        ..Default::default()
    };
    let q_bot = MailQuery {
        mailbox: Some("bot@corp.example".into()),
        ..Default::default()
    };
    assert_eq!(store.list(&q_zoo).unwrap().len(), 0, "zoo 库已删空");
    assert_eq!(store.list(&q_bot).unwrap().len(), 1, "bot 库历史信保留");
}

#[tokio::test]
async fn pop3_password_gate() {
    let (_dir, store) = temp_store();
    seed_two_mailboxes(&store);
    let (port, _tx) = spawn_pop3(store, "secret").await;

    let (mut r, mut w) = pop3_session(port).await;
    let _ = recv_line(&mut r).await;
    // 未认证直接 STAT = -ERR。
    send_line(&mut w, "STAT").await;
    assert!(
        recv_line(&mut r).await.starts_with("-ERR"),
        "未认证不得进入 TRANSACTION"
    );
    // 错密码 = -ERR。
    send_line(&mut w, "USER zoo@corp.example").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "PASS wrong").await;
    assert!(recv_line(&mut r).await.starts_with("-ERR"));
    send_line(&mut w, "STAT").await;
    assert!(recv_line(&mut r).await.starts_with("-ERR"));
    // 对密码后放行。
    send_line(&mut w, "USER zoo@corp.example").await;
    let _ = recv_line(&mut r).await;
    send_line(&mut w, "PASS secret").await;
    assert!(recv_line(&mut r).await.starts_with("+OK"));
    send_line(&mut w, "STAT").await;
    assert!(
        recv_line(&mut r).await.starts_with("+OK 1 "),
        "认证后只见本库"
    );
}

// ---------------------------------------------------------------------------
// 启动准入闸 + TLS 装配（产品层 fail-closed 面）
// ---------------------------------------------------------------------------

#[test]
fn pop3_admission_gate_states() {
    // 回环 + 免认证 = 放行（本机调试合法形态）。
    let cfg = Pop3Settings {
        enabled: true,
        password: String::new(),
        ..Default::default()
    };
    assert!(crate::pop3_server::admission(&cfg).is_ok());
    // 回环 + 密码 = 放行。
    let cfg = Pop3Settings {
        enabled: true,
        password: "pw".into(),
        ..Default::default()
    };
    assert!(crate::pop3_server::admission(&cfg).is_ok());
    // 非回环 + 免认证 = 拒绝（不可豁免——allow_plaintext_auth 也压不住）。
    let cfg = Pop3Settings {
        enabled: true,
        bind: "0.0.0.0".into(),
        password: String::new(),
        allow_plaintext_auth: true,
        ..Default::default()
    };
    assert!(
        crate::pop3_server::admission(&cfg).is_err(),
        "免认证非回环必须拒绝启动"
    );
    // 非回环 + 明文密码 + 未显式放行 = 拒绝（默认 fail-closed）。
    let cfg = Pop3Settings {
        enabled: true,
        bind: "0.0.0.0".into(),
        password: "pw".into(),
        ..Default::default()
    };
    assert!(
        crate::pop3_server::admission(&cfg).is_err(),
        "非回环明文默认必须拒绝启动"
    );
    // 非回环 + 明文密码 + allow_plaintext_auth = 放行（显式逃生阀，LAN 自担）。
    let cfg = Pop3Settings {
        enabled: true,
        bind: "0.0.0.0".into(),
        password: "pw".into(),
        allow_plaintext_auth: true,
        ..Default::default()
    };
    assert!(crate::pop3_server::admission(&cfg).is_ok());
}

#[test]
fn build_face_tls_fail_closed() {
    // disabled = None（明文面）。
    assert!(
        crate::build_face_tls(&crate::TlsServerSettings::default(), "POP3")
            .unwrap()
            .is_none()
    );
    // enabled + 路径空 = Err（拒绝启动对应面）。
    let tls = crate::TlsServerSettings {
        enabled: true,
        ..Default::default()
    };
    assert!(crate::build_face_tls(&tls, "POP3").is_err());
    // enabled + 证书文件不存在 = Err。
    let tls = crate::TlsServerSettings {
        enabled: true,
        cert_path: "Z:/definitely/not/here-cert.pem".into(),
        key_path: "Z:/definitely/not/here-key.pem".into(),
    };
    assert!(crate::build_face_tls(&tls, "POP3").is_err());
}

#[tokio::test]
async fn pop3_idle_timeout_closes_session() {
    let (_dir, store) = temp_store();
    seed_two_mailboxes(&store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let ctx = Arc::new(Pop3Ctx {
        store,
        host: null_host(),
    });
    let settings = Arc::new(Pop3Settings {
        enabled: true,
        idle_timeout_secs: 1,
        ..Default::default()
    });
    let (_tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(crate::pop3_server::run(listener, ctx, settings, None, rx));

    let (mut r, mut w) = pop3_session(port).await;
    let _ = recv_line(&mut r).await; // +OK ready
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let write_ok = w.write_all(b"NOOP\r\n").await;
    let mut line = String::new();
    let n = r.read_line(&mut line).await.unwrap_or(0);
    assert!(
        write_ok.is_err() || n == 0,
        "POP3 idle 超时后连接应收口（write_ok={write_ok:?} n={n}）"
    );
}

// ---------------------------------------------------------------------------
// client 配置面（无网络）
// ---------------------------------------------------------------------------

#[test]
fn client_settings_defaults_implicit_tls_465() {
    let c = ClientSettings::default();
    assert_eq!(c.smtp_port, 465);
    assert!(c.smtp_host.is_empty());
    assert!(c.ca_cert_path.is_empty(), "缺省走 webpki 公共根");
}

// ---------------------------------------------------------------------------
// BUG-20261122-008 回归锁：远程行触发的 str 切片 panic
// ---------------------------------------------------------------------------
//
// strip_ci 旧形态 `s[..prefix.len()]` 只有字节长度守卫——CJK 开头的网络行
// 字节 prefix.len()（"MAIL FROM:"=10）落在多字节字符中间 → str 切片 panic
// （会话任务崩 = 连接无应答直接断，远程一行即触发）。extract_envelope_addr
// 旧形态 find('<')/rfind('>') 无序守卫："abc>def<" → 逆序 range panic。
// 修复后两类行都走正常协议应答路径，会话存活。

#[tokio::test]
async fn smtp_multibyte_command_line_does_not_kill_session() {
    let (_dir, store) = temp_store();
    let (port, _tx) = spawn_smtp(store.clone(), 10 * 1024 * 1024).await;

    let (mut r, mut w) = smtp_session(port).await;
    assert!(recv_line(&mut r).await.starts_with("220"));

    send_line(&mut w, "EHLO tester").await;
    // EHLO 多行应答：末行 250（无连字符）——两行都消费。
    assert!(recv_line(&mut r).await.starts_with("250-"));
    assert!(recv_line(&mut r).await.starts_with("250 "));

    // CJK 行：字节 10 落在第 4 个汉字（3 字节/字）中间——修复前此处 panic
    // = 连接无 500 应答直接 EOF（recv_line expect 崩测试即假红信号）。
    send_line(&mut w, "邮件服务器测试行").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("500"), "多字节未知命令应答 500：{resp}");

    // 会话仍存活：后续正常命令有应答（修复前到不了这里）。
    send_line(&mut w, "NOOP").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("250"), "会话存活：{resp}");
}

#[tokio::test]
async fn smtp_inverted_bracket_envelope_does_not_kill_session() {
    let (_dir, store) = temp_store();
    let (port, _tx) = spawn_smtp(store.clone(), 10 * 1024 * 1024).await;

    let (mut r, mut w) = smtp_session(port).await;
    assert!(recv_line(&mut r).await.starts_with("220"));

    send_line(&mut w, "EHLO tester").await;
    assert!(recv_line(&mut r).await.starts_with("250-"));
    assert!(recv_line(&mut r).await.starts_with("250 "));

    // 括号次序倒置（'<' 在 '>' 之后）——修复前 find/rfind 逆序区间
    // `&trimmed[8..3]` panic。修复后按无括号形态走裸地址回落，250 继续会话。
    send_line(&mut w, "MAIL FROM:abc>def<").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("250"), "倒置括号按裸地址回落：{resp}");

    send_line(&mut w, "NOOP").await;
    let resp = recv_line(&mut r).await;
    assert!(resp.starts_with("250"), "会话存活：{resp}");
}

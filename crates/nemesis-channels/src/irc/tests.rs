use super::*;

/// 构造测试用总线发送端（broadcast::channel 是同步构造，不依赖 runtime）。
fn test_sender() -> tokio::sync::broadcast::Sender<InboundMessage> {
    tokio::sync::broadcast::channel(16).0
}

#[tokio::test]
async fn test_irc_channel_new_validates_server() {
    let config = IRCConfig {
        server: String::new(),
        use_tls: true,
        nick: "bot".to_string(),
        password: None,
        channel: "#test".to_string(),
        allow_from: Vec::new(),
        ..Default::default()
    };
    assert!(IRCChannel::new(config, test_sender()).is_err());
}

#[tokio::test]
async fn test_irc_channel_lifecycle() {
    let config = IRCConfig {
        server: "irc.libera.chat:6697".to_string(),
        use_tls: true,
        nick: "NemesisBot".to_string(),
        password: None,
        channel: "#nemesisbot".to_string(),
        allow_from: Vec::new(),
        ..Default::default()
    };
    let ch = IRCChannel::new(config, test_sender()).unwrap();
    assert_eq!(ch.name(), "irc");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_irc_reconnect_backoff_floors_zero() {
    // 0 值退避会让 sleep(0) 退化成紧循环 hammer 服务器（对齐 mqtt
    // reconnect_delay 下限 1 惯例）；装配层负值钳 0 后也落到此兜底。
    let config = IRCConfig {
        server: "irc.libera.chat:6697".to_string(),
        nick: "NemesisBot".to_string(),
        channel: "#nemesisbot".to_string(),
        reconnect_backoff_secs: 0,
        max_reconnect_backoff_secs: 0,
        ..Default::default()
    };
    let ch = IRCChannel::new(config, test_sender()).unwrap();
    assert_eq!(
        ch.config.reconnect_backoff_secs, 1,
        "0 值退避必须抬到下限 1s"
    );
    assert_eq!(ch.config.max_reconnect_backoff_secs, 1);
}

#[test]
fn test_ensure_hash_prefix() {
    assert_eq!(ensure_hash_prefix("test"), "#test");
    assert_eq!(ensure_hash_prefix("#test"), "#test");
    assert_eq!(ensure_hash_prefix(""), "");
}

#[test]
fn test_split_message_short() {
    let lines = IRCChannel::split_message("hello", 400);
    assert_eq!(lines, vec!["hello"]);
}

#[test]
fn test_split_message_long() {
    let long = "a ".repeat(300);
    let lines = IRCChannel::split_message(&long, 400);
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(line.len() <= 400);
    }
}

#[test]
fn test_extract_nick() {
    assert_eq!(
        IRCChannel::extract_nick_from_prefix("nick!user@host"),
        "nick"
    );
    assert_eq!(IRCChannel::extract_nick_from_prefix("nick"), "nick");
}

#[test]
fn test_build_registration() {
    let config = IRCConfig {
        server: "irc.libera.chat:6697".to_string(),
        use_tls: true,
        nick: "TestBot".to_string(),
        password: Some("secret".to_string()),
        channel: "#test".to_string(),
        allow_from: Vec::new(),
        ..Default::default()
    };
    let ch = IRCChannel::new(config, test_sender()).unwrap();
    let cmds = ch.build_registration();
    assert_eq!(cmds[0], "PASS secret");
    assert_eq!(cmds[1], "NICK TestBot");
    assert_eq!(cmds[2], "USER TestBot 0 * :NemesisBot");
}

#[test]
fn test_parse_irc_line() {
    let (prefix, command, params) =
        IRCChannel::parse_irc_line(":nick!user@host PRIVMSG #channel :hello");
    assert_eq!(prefix, Some("nick!user@host"));
    assert_eq!(command, "PRIVMSG");
    assert_eq!(params, "#channel :hello");
}

#[test]
fn test_parse_irc_line_no_prefix() {
    let (prefix, command, params) = IRCChannel::parse_irc_line("PING :12345");
    assert!(prefix.is_none());
    assert_eq!(command, "PING");
    assert_eq!(params, ":12345");
}

#[test]
fn test_handle_ping() {
    let pong = IRCChannel::handle_ping("PING :12345").unwrap();
    assert_eq!(pong, "PONG :12345");
}

#[test]
fn test_handle_ping_not_ping() {
    assert!(IRCChannel::handle_ping("PRIVMSG #test :hello").is_none());
}

#[test]
fn test_parse_privmsg() {
    let (target, content) = IRCChannel::parse_privmsg("#channel :hello world").unwrap();
    assert_eq!(target, "#channel");
    assert_eq!(content, "hello world");
}

#[test]
fn test_parse_privmsg_no_content() {
    assert!(IRCChannel::parse_privmsg("#channel").is_none());
}

// ---- Additional coverage tests for 95%+ ----

#[test]
fn test_parse_irc_line_prefix_only() {
    // Just a prefix with no command
    let (prefix, command, params) = IRCChannel::parse_irc_line(":nick!user@host");
    assert!(prefix.is_none());
    assert_eq!(command, "");
    assert_eq!(params, "");
}

#[test]
fn test_parse_irc_line_no_params() {
    let (prefix, command, params) = IRCChannel::parse_irc_line("QUIT");
    assert!(prefix.is_none());
    assert_eq!(command, "QUIT");
    assert_eq!(params, "");
}

#[test]
fn test_parse_irc_line_with_prefix_no_params() {
    let (prefix, command, params) = IRCChannel::parse_irc_line(":server.example.com 001");
    assert_eq!(prefix, Some("server.example.com"));
    assert_eq!(command, "001");
    assert_eq!(params, "");
}

#[test]
fn test_parse_privmsg_with_colons() {
    let (target, content) = IRCChannel::parse_privmsg("#channel :hello: world").unwrap();
    assert_eq!(target, "#channel");
    assert_eq!(content, "hello: world");
}

#[test]
fn test_extract_nick_no_bang() {
    assert_eq!(IRCChannel::extract_nick_from_prefix("justnick"), "justnick");
}

#[test]
fn test_build_registration_no_password() {
    let config = IRCConfig {
        server: "irc.libera.chat:6697".to_string(),
        use_tls: true,
        nick: "TestBot".to_string(),
        password: None,
        channel: "#test".to_string(),
        allow_from: Vec::new(),
        ..Default::default()
    };
    let ch = IRCChannel::new(config, test_sender()).unwrap();
    let cmds = ch.build_registration();
    assert_eq!(cmds[0], "NICK TestBot");
    assert_eq!(cmds[1], "USER TestBot 0 * :NemesisBot");
}

#[test]
fn test_ensure_hash_prefix_various() {
    assert_eq!(ensure_hash_prefix("channel"), "#channel");
    assert_eq!(ensure_hash_prefix("#channel"), "#channel");
    assert_eq!(ensure_hash_prefix("##double"), "##double");
}

#[test]
fn test_split_message_exact_limit() {
    let msg = "a".repeat(400);
    let lines = IRCChannel::split_message(&msg, 400);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 400);
}

#[test]
fn test_split_message_multiline() {
    // split_message chunks by IRC line-length limit, not by newline. A short
    // multiline message (under max_len) is returned as a single whole chunk.
    let msg = "line1\nline2\nline3";
    let lines = IRCChannel::split_message(msg, 400);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "line1\nline2\nline3");
}

#[test]
fn test_irc_config_default() {
    let cfg = IRCConfig::default();
    assert!(cfg.server.is_empty());
    assert!(cfg.use_tls);
    assert!(cfg.nick.is_empty());
    assert!(cfg.password.is_none());
    assert!(cfg.channel.is_empty());
    assert!(cfg.allow_from.is_empty());
}

#[test]
fn test_handle_ping_empty() {
    let result = IRCChannel::handle_ping("PING ");
    assert_eq!(result, Some("PONG ".to_string()));
}

// ---- BUG-20261031-247：收件黑洞（读循环无 PRIVMSG 分支，通道从未接线总线） ----

/// 起一个只推行的假 IRC 服务器（单连接）：accept 后先发 RPL_WELCOME greeting
/// （触发客户端 JOIN），再把 script 行一次写尽（客户端读循环按行处理，无
/// 请求-响应依赖），随后静读直到客户端断开（stop() 发 QUIT）。
async fn spawn_fake_irc(script: &[String]) -> std::net::SocketAddr {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let script = script.to_vec();
    tokio::spawn(async move {
        let (stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let (rd, mut wr) = stream.into_split();
        let mut greeting = String::from(":srv.test 001 nb :Welcome to test\r\n");
        for line in &script {
            greeting.push_str(line);
        }
        if wr.write_all(greeting.as_bytes()).await.is_err() {
            return;
        }
        // 静读客户端行（JOIN/QUIT 等），直到对端关闭——保持连接活着让
        // 客户端读循环有机会处理 script 行。
        let mut rd = BufReader::new(rd);
        let mut line = String::new();
        loop {
            line.clear();
            if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
        }
    });
    addr
}

fn irc_config_for(addr: std::net::SocketAddr, allow_from: Vec<String>) -> IRCConfig {
    IRCConfig {
        server: addr.to_string(),
        use_tls: false,
        nick: "nb".to_string(),
        password: None,
        channel: "#test".to_string(),
        allow_from,
        reconnect_backoff_secs: 1,
        max_reconnect_backoff_secs: 2,
    }
}

/// 修复前红证形态：旧读循环的 match 没有 PRIVMSG 分支——消息落 `_ => {}`
/// 静默丢弃；构造器只收 1 个参数，收件发布面在修复前结构性不存在（本测试
/// 在旧源码下连编译都过不了）。修复后：频道 PRIVMSG 必须以 InboundMessage
/// 发布到总线，chat_id=频道名、sender_id=发送者昵称、session_key 带 irc: 前缀。
#[tokio::test]
async fn test_privmsg_publishes_inbound_to_bus() {
    let script = vec![":alice!u@h PRIVMSG #test :hello bot\r\n".to_string()];
    let addr = spawn_fake_irc(&script).await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let ch = IRCChannel::new(irc_config_for(addr, Vec::new()), tx).unwrap();
    ch.start().await.unwrap();

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("connection loop must publish the PRIVMSG within 5s")
        .unwrap();
    assert_eq!(inbound.channel, "irc");
    assert_eq!(inbound.sender_id, "alice");
    assert_eq!(inbound.chat_id, "#test");
    assert_eq!(inbound.content, "hello bot");
    assert_eq!(inbound.session_key, "irc:#test");

    ch.stop().await.unwrap();
}

/// 私聊判别：target 是本 bot 当前昵称的 PRIVMSG 是私聊（query），chat_id
/// 必须取发送者昵称而非 target——否则回复会 PRIVMSG 给 bot 自己，私聊
/// 永远无法应答。IRC 昵称大小写不敏感，判别用等值忽略大小写。
#[tokio::test]
async fn test_privmsg_query_chat_id_takes_sender_nick() {
    let script = vec![
        ":alice!u@h PRIVMSG nb :query hi\r\n".to_string(),
        ":BoB!u@h PRIVMSG NB :cased query\r\n".to_string(),
    ];
    let addr = spawn_fake_irc(&script).await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let ch = IRCChannel::new(irc_config_for(addr, Vec::new()), tx).unwrap();
    ch.start().await.unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("first query must be published within 5s")
        .unwrap();
    assert_eq!(first.chat_id, "alice");
    assert_eq!(first.sender_id, "alice");
    assert_eq!(first.content, "query hi");
    assert_eq!(first.session_key, "irc:alice");

    let second = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("second query must be published within 5s")
        .unwrap();
    assert_eq!(
        second.chat_id, "BoB",
        "nick casemapping must be case-insensitive"
    );

    ch.stop().await.unwrap();
}

/// allow-list：非空白名单时不在名单内的发送者被拦——零总线发布。
/// 名单匹配发送者昵称（IRCConfig.allow_from 语义 "Allowed sender IDs"）。
#[tokio::test]
async fn test_privmsg_blocked_by_allow_list() {
    let script = vec![
        ":alice!u@h PRIVMSG #test :blocked msg\r\n".to_string(),
        ":bob!u@h PRIVMSG #test :allowed msg\r\n".to_string(),
    ];
    let addr = spawn_fake_irc(&script).await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    let ch = IRCChannel::new(irc_config_for(addr, vec!["bob".to_string()]), tx).unwrap();
    ch.start().await.unwrap();

    // 唯一应到达的是 bob 的消息；alice 的被 allow-list 拦下。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("bob's message must be published within 5s")
        .unwrap();
    assert_eq!(inbound.sender_id, "bob");
    assert_eq!(inbound.content, "allowed msg");

    // 短窗探针：alice 的消息不得在拦截后到达。
    let leaked = tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await;
    assert!(
        leaked.is_err(),
        "allow-listed sender must never reach the bus"
    );

    ch.stop().await.unwrap();
}

use super::*;

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
    assert!(IRCChannel::new(config).is_err());
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
    let ch = IRCChannel::new(config).unwrap();
    assert_eq!(ch.name(), "irc");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
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
    let ch = IRCChannel::new(config).unwrap();
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
    let (target, content) =
        IRCChannel::parse_privmsg("#channel :hello world").unwrap();
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
    let ch = IRCChannel::new(config).unwrap();
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
    let msg = "line1\nline2\nline3";
    let lines = IRCChannel::split_message(msg, 400);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "line1");
    assert_eq!(lines[1], "line2");
    assert_eq!(lines[2], "line3");
}

#[test]
fn test_irc_config_default() {
    let cfg = IRCConfig::default();
    assert!(cfg.server.is_empty());
    assert!(!cfg.use_tls);
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

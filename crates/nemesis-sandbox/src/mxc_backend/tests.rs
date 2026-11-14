//! mxc_backend 单测：策略映射纯函数 + 警告常量契约（spawn 事务不进单测——
//! 需要真容器后端，归集成/e2e 验收）。

use super::*;

// ---------------------------------------------------------------------------
// 警告常量契约（D5：前端文案单一真相源——WSAPI 回传此常量，禁止前端另抄）
// ---------------------------------------------------------------------------

#[test]
fn warning_carries_pinned_version() {
    assert!(
        MXC_WARNING.contains(MXC_VERSION),
        "警告文案必须携带钉版短哈希（升级时三处同更）：{MXC_WARNING}"
    );
    assert!(
        MXC_WARNING.contains("护栏功能，非沙盒"),
        "警告文案必须含用户拍板的定性原文：{MXC_WARNING}"
    );
}

// ---------------------------------------------------------------------------
// build_container_request：策略映射
// ---------------------------------------------------------------------------

fn spec() -> MxcSpawnSpec {
    MxcSpawnSpec {
        exe: PathBuf::from("C:/bot/nemesisbot.exe"),
        env_pairs: vec![("NEMESISBOT_ROLE".to_string(), "executor".to_string())],
        working_dir: PathBuf::from("C:/bot/workspace"),
        home: PathBuf::from("C:/bot"),
        extra_roots: Vec::new(),
        request_line: "{\"tool\":\"exec\"}\n".to_string(),
        timeout: Duration::from_secs(60),
    }
}

#[test]
fn open_mode_grants_full_egress_denies_ingress() {
    let req = build_container_request(&spec(), "open", &[]).expect("open builds");
    let net = req.network.expect("network policy present");
    let egress = net.egress.expect("egress present");
    assert_eq!(
        egress.default,
        Some(mxc_sdk::v1::NetworkAction::Allow),
        "open=egress 全放行"
    );
    assert!(
        egress.allow.as_ref().is_none_or(|v| v.is_empty())
            && egress.deny.as_ref().is_none_or(|v| v.is_empty()),
        "open 无白名单/黑名单"
    );
    let ingress = net.ingress.expect("ingress present");
    assert_eq!(
        ingress.default,
        Some(mxc_sdk::v1::NetworkAction::Deny),
        "ingress 恒 deny"
    );
}

#[test]
fn locked_mode_denies_default_and_parses_allowlist() {
    let allow = vec!["140.82.112.0/20:443".to_string(), "10.0.0.0/8".to_string()];
    let req = build_container_request(&spec(), "locked", &allow).expect("locked builds");
    let egress = req.network.expect("network").egress.expect("egress");
    assert_eq!(
        egress.default,
        Some(mxc_sdk::v1::NetworkAction::Deny),
        "locked=deny-default"
    );
    let rules = egress.allow.as_ref().expect("白名单在场");
    assert_eq!(rules.len(), 2, "两条白名单规则");
    let first = &rules[0];
    let peers = first.to.as_ref().expect("peers 在场");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].cidr, "140.82.112.0/20");
    let ports = first.ports.as_ref().expect("ports 在场");
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].port, Some(443), "cidr:port 解析端口");
    let second = &rules[1];
    assert_eq!(
        second.ports.as_ref().unwrap()[0].port,
        None,
        "纯 cidr 条目不限端口"
    );
}

#[test]
fn locked_mode_rejects_bad_cidr_loudly() {
    let err = build_container_request(&spec(), "locked", &["not-a-cidr".to_string()])
        .expect_err("坏 CIDR 必须 loud 拒绝");
    assert!(err.contains("egress_allow"), "错误信息点名配置键: {err}");
}

#[test]
fn locked_mode_rejects_bad_port_loudly() {
    // 审查二轮：`cidr:port` 尾段非 u16（如 99999）不能整串当 cidr 塞引擎
    for bad in ["10.0.0.0/8:99999", "10.0.0.0/8:0", "10.0.0.0/8:+443"] {
        let err = build_container_request(&spec(), "locked", &[bad.to_string()])
            .expect_err("坏端口必须 loud 拒绝");
        assert!(err.contains("端口段非法"), "错误信息点名端口段: {err}");
    }
}

#[test]
fn locked_mode_accepts_ipv6_cidr_without_port() {
    let req = build_container_request(&spec(), "locked", &["2001:db8::/32".to_string()])
        .expect("IPv6 纯 CIDR（多冒号）按整串放行");
    let rules = req
        .network
        .expect("network")
        .egress
        .expect("egress")
        .allow
        .expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].to.as_ref().unwrap()[0].cidr, "2001:db8::/32");
}

#[test]
fn unknown_network_mode_is_loud_rejection() {
    let err = build_container_request(&spec(), "yolo", &[]).expect_err("未知档位拒绝");
    assert!(err.contains("open|locked"), "错误信息列出合法值: {err}");
}

#[test]
fn filesystem_maps_workspace_rw_home_ro_secrets_denied() {
    let req = build_container_request(&spec(), "open", &[]).expect("builds");
    let fs = req.filesystem.expect("filesystem policy present");
    assert_eq!(
        fs.readwrite_paths,
        vec!["C:/bot/workspace"],
        "工作区=读写根"
    );
    assert_eq!(
        fs.readonly_paths,
        vec!["C:/bot"],
        "home=只读（子进程读 config.json）"
    );
    // 秘密定向拒绝——真实路径在 `<home>/workspace/config/`（与
    // credentials_path_for_home 同源；审查三轮修正：旧条目指向不存在的
    // `<home>/config/` 且被存在性过滤抹掉，保护整体落空）。join 的分隔符
    // 随平台——语义断言而非字节断言。
    let ws_config = PathBuf::from("C:/bot").join("workspace").join("config");
    let secrets: Vec<String> = ["credentials.yaml", "auth.json"]
        .iter()
        .map(|f| ws_config.join(f).display().to_string())
        .collect();
    assert_eq!(fs.denied_paths, secrets, "凭据 + 认证态定向拒绝");
}

#[test]
fn extra_roots_extend_readwrite_paths() {
    // 审查三轮：项目 loop 的 extra_roots 只透传 env 围栏、容器侧不授权 =
    // 项目写类工具全被容器拒绝（两层策略自相矛盾）。容器 readwrite 必须随根扩展。
    let mut s = spec();
    s.extra_roots = vec![PathBuf::from("C:/projects/demo")];
    let req = build_container_request(&s, "open", &[]).expect("builds");
    let fs = req.filesystem.expect("filesystem policy present");
    assert_eq!(
        fs.readwrite_paths,
        vec!["C:/bot/workspace", "C:/projects/demo"],
        "容器读写根 = 工作区 + extra_roots"
    );
}

#[test]
fn request_carries_command_env_timeout_and_process_containment() {
    let req = build_container_request(&spec(), "open", &[]).expect("builds");
    assert!(
        req.command.contains("nemesisbot.exe"),
        "command=引号包的 exe 路径: {}",
        req.command
    );
    assert!(req.command.starts_with('"'), "路径带空格安全：引号开头");
    assert_eq!(
        req.environment.as_ref().unwrap().len(),
        1,
        "env_pairs 逐条注入"
    );
    assert_eq!(
        req.inherit_default_environment,
        Some(true),
        "叠加后端默认环境（PATH 不丢）"
    );
    assert_eq!(req.timeout_ms, Some(60_000), "timeout 秒→毫秒");
    assert_eq!(
        req.containment,
        mxc_sdk::v1::Containment::Process,
        "抽象意图=按宿主解析"
    );
    assert_eq!(req.working_directory.as_deref(), Some("C:/bot/workspace"));
    let ui = req.ui.expect("ui policy present");
    assert!(
        !ui.disable,
        "UI 子系统可初始化（disable=true → GUI 链接子进程 0xC0000142，官方诊断处方）"
    );
    assert!(
        matches!(ui.clipboard, mxc_sdk::v1::ClipboardPolicy::None),
        "剪贴板仍禁（细粒度加固）"
    );
    assert!(!ui.allow_input_injection, "输入注入仍禁");
}

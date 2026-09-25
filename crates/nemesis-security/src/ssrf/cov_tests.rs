// ssrf.rs 覆盖率补充测试（DNS 解析后 IP 落入封锁集的 ResolvesToBlocked 臂
// ——localhost → 127.0.0.1/::1 必中 loopback 封锁）。
//
// 豁免（死防御 / 解析器行为性不可达，仅记录不硬凑）：
// - 208 / 271（BlockedScheme 臂）：resolver::parse_url 在 76-78 已把 scheme
//   限定为 http/https（否则 UnsupportedScheme 上抛），此处 `scheme != http
//   && scheme != https` 恒假——注释自述的「可能漏进的 scheme」实际进不来，
//   纯二道防线。
// - 359 / 484（NoAddresses 臂）：resolve_host_dns 用 std to_socket_addrs，
//   解析失败走 Err→DnsFailed；Ok 但返回空迭代器在 Windows 系统解析器上无
//   已知触发形态，纯防御分支。

use super::*;
use std::net::ToSocketAddrs;

/// ResolvesToBlocked 臂（365-369）：字面 localhost 被 336-347 的主机名检查
/// 先拦（到不了 DNS），所以用「本机主机名」——它必解析为本机地址（私网/
/// link-local/loopback），全部落在封锁集。为保证断言确定性，先自查解析
/// 结果确有被封 IP；若遇到只有全局地址的罕见环境则安静跳过。
#[test]
fn localhost_resolution_hits_resolves_to_blocked() {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| {
        // 非 Windows 兜底：hostname 命令不可用时退回 uname 形态不可靠，
        // 直接用 gethostname 等价物——std 无此 API，此处仅 CI 环境用。
        std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
    });
    assert!(!host.is_empty(), "必须取得本机主机名");
    assert!(
        !host.eq_ignore_ascii_case("localhost"),
        "字面 localhost 走主机名封锁臂，测不到 DNS 路径"
    );

    // 预检：本机地址里至少有一个被封 IP（loopback/私网/link-local/保留）。
    let addrs: Vec<std::net::IpAddr> = format!("{host}:0")
        .to_socket_addrs()
        .map(|it| it.map(|sa| sa.ip()).collect())
        .unwrap_or_default();
    assert!(!addrs.is_empty(), "本机主机名必须可解析");
    let has_blocked = addrs.iter().any(|ip| {
        crate::resolver::is_loopback_ip(ip)
            || crate::resolver::is_private_ip(ip)
            || crate::resolver::is_link_local_ip(ip)
            || crate::resolver::is_reserved_ip(ip)
    });
    if !has_blocked {
        // 全局地址环境（罕见）：无 Blocked IP 可打，跳过断言。
        return;
    }

    let guard = Guard::from_enabled(true);

    let err = guard
        .resolve_and_validate(&format!("http://{host}/x"))
        .unwrap_err();
    // 错误体对 host 做了小写归一，用小写形式断言。
    let msg = err.to_string();
    assert!(
        msg.contains(&host.to_lowercase()) && msg.contains("resolves to blocked IP"),
        "{msg}"
    );

    // collect 变体同路径。
    let err = guard
        .resolve_and_validate_collect(&format!("http://{host}/y"))
        .unwrap_err();
    assert!(err.to_string().contains("resolves to blocked IP"));

    // validate_url 主入口同走 locked 路径。
    let err = guard.validate_url(&format!("http://{host}/z")).unwrap_err();
    assert!(err.to_string().contains("resolves to blocked IP"));
}

/// 对照组：公网形状的 IP 直连 URL 通过校验（Ok），且动态 CIDR 封锁生效。
#[test]
fn public_ip_url_passes_and_dynamic_cidr_blocks() {
    let guard = Guard::from_enabled(true);

    guard.validate_url("http://1.1.1.10/x").unwrap();

    // 动态加封锁段 → 同一 URL 立即被拦。
    guard.add_blocked_cidr("1.1.1.0/24").unwrap();
    let err = guard.validate_url("http://1.1.1.10/x").unwrap_err();
    assert!(err.to_string().contains("1.1.1.10"), "{err}");

    // 非法 CIDR → InvalidCidr。
    assert!(guard.add_blocked_cidr("not-a-cidr").is_err());

    // check_ip：非法字符串 → InvalidIp；合法公网 → Ok。
    assert!(guard.check_ip("999.1.1.1").is_err());
    guard.check_ip("8.8.8.8").unwrap();

    // 白名单：加入 host 后 validate_url 直接放行（不解析不校验）。
    guard.add_allowed_host("Internal.Host.Example");
    assert!(
        guard
            .validate_url("http://internal.host.example/secret")
            .is_ok()
    );
    guard.remove_allowed_host("internal.host.example");
    let err = guard
        .resolve_and_validate("http://internal.host.example/secret")
        .unwrap_err();
    assert!(err.to_string().contains("internal.host.example"), "{err}");
}

/// 禁用守卫：全部入口直通 Ok（enabled=false 早退臂）。
#[test]
fn disabled_guard_passes_everything() {
    let guard = Guard::from_enabled(false);
    assert!(!guard.is_enabled());
    assert!(guard.validate_url("http://localhost/x").is_ok());
    assert!(guard.resolve_and_validate("http://localhost/x").is_ok());
    assert!(
        guard
            .resolve_and_validate_collect("http://localhost/x")
            .unwrap()
            .is_empty()
    );
    assert!(guard.check_ip("127.0.0.1").is_ok());
}

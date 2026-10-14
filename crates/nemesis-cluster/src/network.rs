//! Network utilities for cluster communication.
//!
//! Provides helpers for local IP detection, subnet matching, address parsing,
//! and connection management. Mirrors Go's `network.go` and `node.go` helper
//! functions.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

// ---------------------------------------------------------------------------
// Basic helpers
// ---------------------------------------------------------------------------

/// Resolve a hostname:port string to socket addresses.
pub fn resolve_addr(addr_str: &str) -> Result<Vec<SocketAddr>, String> {
    addr_str
        .to_socket_addrs()
        .map(|iter| iter.collect())
        .map_err(|e| format!("Failed to resolve '{}': {}", addr_str, e))
}

/// Get the first non-loopback IPv4 address of the local machine.
/// Returns `127.0.0.1` if no suitable address is found.
///
/// The kernel's UDP-connect answer is **rejected when it belongs to a
/// virtual interface** (docker0/virbr0/...): libvirt NAT bridges routinely
/// own the default route, and a virtual-bridge address is unreachable to
/// cluster peers — the same filter contract [`get_all_local_ips`] enforces
/// (invariant: primary ∈ full list, pinned by
/// `test_get_all_local_ips_enumerates_all_interfaces`). When rejected, the
/// filtered enumeration's best address is used instead; when enumeration
/// yields no non-virtual address at all, the kernel answer is kept as last
/// resort.
pub fn get_local_ip() -> IpAddr {
    let ip = udp_connect_local_ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return ip;
    }
    if let Some(ips) = enumerate_filtered_ips() {
        if ips.contains(&ip.to_string()) {
            return ip;
        }
        // Kernel picked a filtered-out (virtual) source; prefer the
        // enumeration's best non-virtual address.
        if let Some(best) = ips.first()
            && let Ok(parsed) = best.parse()
        {
            return parsed;
        }
    }
    ip
}

/// Raw UDP-connect trick: the source address the kernel would use to reach a
/// public address. Returns `127.0.0.1` if no suitable address is found.
fn udp_connect_local_ip() -> IpAddr {
    match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            if socket.connect("8.8.8.8:80").is_ok()
                && let Ok(addr) = socket.local_addr()
            {
                let ip = addr.ip();
                if !ip.is_loopback() {
                    return ip;
                }
            }
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        }
        Err(_) => IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
    }
}

/// Parse a socket address string. Returns a default if parsing fails.
pub fn parse_socket_addr(addr_str: &str) -> SocketAddr {
    addr_str
        .parse()
        .unwrap_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9000))
}

/// Check whether an address is a loopback address.
pub fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Build a bind address from an optional bind string and a default port.
pub fn build_bind_address(bind: Option<&str>, default_port: u16) -> String {
    match bind {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => format!("0.0.0.0:{}", default_port),
    }
}

// ---------------------------------------------------------------------------
// Network interface enumeration
// ---------------------------------------------------------------------------

/// A local network interface with its subnet information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetworkInterface {
    pub ip: String,
    pub mask: String,
    pub network_ip: String,
}

/// Get all local network interfaces with their subnet masks.
///
/// Enumerates network interfaces using `if-addrs` (cross-platform).
/// Falls back to the UDP connect trick if enumeration fails.
///
/// Link-local (169.254.0.0/16) interfaces are **kept**: direct Ethernet /
/// DHCP-failure topologies have link-local as the *only* interconnect, and
/// consumers of this list (broadcast-address enumeration in `listener.rs`,
/// subnet matching in `is_same_subnet`) must still see those interfaces or
/// discovery structurally cannot work on such networks. See
/// `docs/REPORT/2026-09-01_link-local-discovery-fix.md`.
pub fn get_local_network_interfaces() -> Vec<NetworkInterface> {
    let mut result = get_local_network_interfaces_impl();
    supplement_apipa_interfaces(&mut result);
    result
}

fn get_local_network_interfaces_impl() -> Vec<NetworkInterface> {
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            let mut result = Vec::new();
            for iface in &interfaces {
                if iface.is_loopback() {
                    continue;
                }
                match &iface.addr {
                    if_addrs::IfAddr::V4(v4_addr) => {
                        let v4 = v4_addr.ip;
                        if is_virtual_interface(&iface.name) {
                            continue;
                        }
                        let mask = v4_addr.netmask;
                        let mask_octets = mask.octets();
                        let octets = v4.octets();
                        result.push(NetworkInterface {
                            ip: v4.to_string(),
                            mask: mask.to_string(),
                            network_ip: format!(
                                "{}.{}.{}.{}",
                                octets[0] & mask_octets[0],
                                octets[1] & mask_octets[1],
                                octets[2] & mask_octets[2],
                                octets[3] & mask_octets[3]
                            ),
                        });
                    }
                    if_addrs::IfAddr::V6(_) => {
                        // Skip IPv6 for now
                    }
                }
            }
            result
        }
        Err(_) => {
            // Fallback: single interface via UDP connect trick
            let local = get_local_ip();
            if local.is_loopback() || local.is_unspecified() {
                return Vec::new();
            }
            match local {
                IpAddr::V4(v4) => {
                    let octets = v4.octets();
                    vec![NetworkInterface {
                        ip: local.to_string(),
                        mask: "255.255.255.0".into(),
                        network_ip: format!("{}.{}.{}.0", octets[0], octets[1], octets[2]),
                    }]
                }
                IpAddr::V6(_) => Vec::new(),
            }
        }
    }
}

/// Convert a prefix length (0-32) to an IPv4 subnet mask.
#[allow(dead_code)]
fn prefix_len_to_mask(prefix_len: u32) -> Ipv4Addr {
    if prefix_len == 0 {
        return Ipv4Addr::new(0, 0, 0, 0);
    }
    let mask = if prefix_len >= 32 {
        0xffffffffu32
    } else {
        !((1u32 << (32 - prefix_len)) - 1)
    };
    Ipv4Addr::from(mask)
}

/// Check if two IP addresses are in the same subnet.
///
/// Checks whether two IPs are in the same subnet **according to the local
/// network interfaces' masks**.
///
/// Note: this is NOT a pure function of `(ip1, ip2)` — the result depends on
/// what network interfaces the host has. If the host has a /16 interface
/// (common under WSL/Docker), `192.168.1.1` and `192.168.2.1` will be
/// considered the same subnet. Callers needing deterministic behavior should
/// use `is_same_subnet_simple` directly with an explicit `/24` assumption.
///
/// Uses real subnet masks from local network interfaces when available,
/// falls back to simple /24 matching.
pub fn is_same_subnet(ip1: &str, ip2: &str) -> bool {
    let local_interfaces = get_local_network_interfaces();
    if local_interfaces.is_empty() {
        return is_same_subnet_simple(ip1, ip2);
    }

    let parsed1: IpAddr = match ip1.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    let parsed2: IpAddr = match ip2.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    for iface in &local_interfaces {
        let mask: IpAddr = match iface.mask.parse() {
            Ok(m) => m,
            Err(_) => continue,
        };

        match (parsed1, parsed2, mask) {
            (IpAddr::V4(a), IpAddr::V4(b), IpAddr::V4(m)) => {
                let ao = a.octets();
                let bo = b.octets();
                let mo = m.octets();
                let same = (0..4).all(|i| (ao[i] & mo[i]) == (bo[i] & mo[i]));
                if same {
                    return true;
                }
            }
            _ => continue,
        }
    }

    false
}

/// Simple /24 subnet check fallback.
fn is_same_subnet_simple(ip1: &str, ip2: &str) -> bool {
    let parts1: Vec<&str> = ip1.split('.').collect();
    let parts2: Vec<&str> = ip2.split('.').collect();
    if parts1.len() < 4 || parts2.len() < 4 {
        return false;
    }
    parts1[0] == parts2[0] && parts1[1] == parts2[1] && parts1[2] == parts2[2]
}

// ---------------------------------------------------------------------------
// GetAllLocalIPs - returns all local IPs sorted by priority
// ---------------------------------------------------------------------------

/// A candidate IP with its interface priority (lower = higher priority).
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CandidateIp {
    ip: String,
    priority: u32,
    /// Link-local (169.254.0.0/16) addresses sort **after** all normal ones
    /// but are not dropped: on direct-cable / DHCP-failure topologies the
    /// link-local interface is the only interconnect, and dropping it made
    /// UDP discovery structurally impossible there.
    link_local: bool,
}

/// Get all local IP addresses by enumerating network interfaces.
///
/// Mirrors Go's `GetAllLocalIPs()`:
/// 1. Enumerates all network interfaces via `if-addrs` (cross-platform).
/// 2. Skips loopback and virtual interfaces.
/// 3. Collects non-loopback IPv4 addresses; link-local (169.254.x.x) is
///    kept but sorted last (normal addresses win when present).
/// 4. Sorts by interface priority (Ethernet > WiFi > Other), link-local last.
/// 5. Falls back to the UDP-connect trick if interface enumeration fails.
/// 6. Windows: if-addrs misses APIPA (169.254/16) addresses entirely
///    (`ipconfig` shows them, enumeration doesn't — 2026-10-12 acceptance
///    verified), so `GetAdaptersAddresses` supplements them at the tail.
///    On direct-cable / ICS topologies link-local is the *only* interconnect.
pub fn get_all_local_ips() -> Vec<String> {
    let mut result = get_all_local_ips_impl();
    supplement_apipa_ips(&mut result);
    result
}

fn get_all_local_ips_impl() -> Vec<String> {
    // Try enumerating real network interfaces first (matches Go's net.Interfaces()).
    match enumerate_filtered_ips() {
        Some(result) if !result.is_empty() => result,
        _ => fallback_local_ip(),
    }
}

/// Enumeration-only core shared by [`get_all_local_ips`] and [`get_local_ip`]:
/// interface enumeration + loopback/virtual/vethernet-NAT filters + priority
/// sort + dedup. Returns `None` when enumeration itself fails; `Some(empty)`
/// when every interface was filtered out. No UDP fallback here — callers
/// decide.
fn enumerate_filtered_ips() -> Option<Vec<String>> {
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let mut candidates: Vec<CandidateIp> = Vec::new();

    for iface in &interfaces {
        // Skip loopback addresses
        if iface.is_loopback() {
            continue;
        }

        let ip = iface.addr.ip();

        // Only consider IPv4; link-local is kept but deprioritized.
        if let IpAddr::V4(v4) = ip {
            let is_link_local = v4.is_link_local();

            // Skip virtual interfaces by name
            if is_virtual_interface(&iface.name) {
                continue;
            }

            // P3：vethernet 命名 + 172.16.0.0/12 NAT 段 = Hyper-V/WSL
            // 内部交换机（Windows 外部 vSwitch 挂真网卡时地址是真实
            // 网段，不受影响）。
            if is_vethernet_nat_addr(&iface.name, v4) {
                continue;
            }

            let priority = get_interface_priority(&iface.name);
            candidates.push(CandidateIp {
                ip: v4.to_string(),
                priority,
                link_local: is_link_local,
            });
        }
    }

    // Sort by (priority, link-local-last); stable sort preserves
    // relative order within the same key.
    candidates.sort_by_key(|c| (c.priority, c.link_local));

    // Extract just the IP strings, deduplicating
    let mut result: Vec<String> = Vec::new();
    for c in candidates {
        if !result.contains(&c.ip) {
            result.push(c.ip);
        }
    }

    Some(result)
}

/// Fallback: get a single local IP via the UDP connect trick.
fn fallback_local_ip() -> Vec<String> {
    let local_ip = get_local_ip();
    if local_ip.is_loopback() || local_ip.is_unspecified() {
        Vec::new()
    } else {
        vec![local_ip.to_string()]
    }
}

// ---------------------------------------------------------------------------
// R1: Windows APIPA (169.254/16) supplement
// ---------------------------------------------------------------------------

/// Supplement link-local (169.254/16) addresses that `if_addrs` missed on
/// Windows. Appends to the tail — callers of `get_all_local_ips` already
/// treat normal addresses as higher priority, so tail position preserves the
/// "link-local sorts last" contract. Dedup against the existing result; on
/// non-Windows this is a no-op (if_addrs enumerates APIPA fine there).
fn supplement_apipa_ips(result: &mut Vec<String>) {
    #[cfg(target_os = "windows")]
    {
        for ip in windows_apipa_ips() {
            if !result.contains(&ip) {
                result.push(ip);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = result;
    }
}

/// Supplement link-local (169.254/16) **interfaces** (ip + netmask) that
/// `if_addrs` missed on Windows — same root gap as [`supplement_apipa_ips`],
/// second consumer: subnet matching (`get_local_interfaces` → dial ordering
/// and bridge arbitration). Without it, link-local-only topologies are
/// invisible to `is_same_subnet` and candidates never get the direct-connect
/// preference. On non-Windows this is a no-op.
fn supplement_apipa_interfaces(result: &mut Vec<NetworkInterface>) {
    #[cfg(target_os = "windows")]
    {
        for (name, entries) in windows_apipa_adapters() {
            collect_adapter_link_local_ifaces(&name, &entries, result);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = result;
    }
}

/// R1 纯函数核（接口形态）：把单个 adapter 的 (IPv4, 前缀长) 列表里该入
/// 表的 link-local 项转成 NetworkInterface（掩码由前缀长推导）追加——
/// 跳过虚拟接口，按 ip 去重。FFI 侧逐 adapter 委托本函数。
#[cfg(target_os = "windows")]
fn collect_adapter_link_local_ifaces(
    name: &str,
    entries: &[(Ipv4Addr, u8)],
    out: &mut Vec<NetworkInterface>,
) {
    if is_virtual_interface(name) {
        return;
    }
    for (ip, prefix) in entries {
        if ip.is_loopback() || !ip.is_link_local() {
            continue;
        }
        let s = ip.to_string();
        if out.iter().any(|i| i.ip == s) {
            continue;
        }
        let mask = prefix_len_to_v4_mask(*prefix);
        let mask_octets: Vec<u8> = mask.split('.').filter_map(|o| o.parse().ok()).collect();
        let octets = ip.octets();
        let network_ip = if mask_octets.len() == 4 {
            format!(
                "{}.{}.{}.{}",
                octets[0] & mask_octets[0],
                octets[1] & mask_octets[1],
                octets[2] & mask_octets[2],
                octets[3] & mask_octets[3]
            )
        } else {
            mask.clone()
        };
        out.push(NetworkInterface {
            ip: s,
            mask,
            network_ip,
        });
    }
}

/// R1 纯函数核：从单个 adapter 的（名称, IPv4 列表）里挑出该入池的
/// link-local 候选——跳过虚拟接口与回环，去重追加。FFI 侧把
/// GetAdaptersAddresses 的链表拉平成这个形状后委托本函数，单测用合成
/// adapter 数据钉死行为（实机断言归真机腿）。
/// allow(dead_code)：唯一生产调用方在 `#[cfg(windows)]` 的
/// windows_apipa_ips 里，非 Windows lib 目标下仅测试可见——不加门控会让
/// Linux clippy -D warnings 门禁红（先例：本文件 prefix_len_to_mask）。
#[allow(dead_code)]
fn collect_adapter_link_local(name: &str, ips: &[Ipv4Addr], out: &mut Vec<String>) {
    if is_virtual_interface(name) {
        return;
    }
    for ip in ips {
        if ip.is_loopback() || !ip.is_link_local() {
            continue;
        }
        let s = ip.to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
}

/// Enumerate adapters via `GetAdaptersAddresses` (IPv4 only) and return the
/// link-local unicast addresses if_addrs fails to report on Windows. Best
/// effort: any FFI/allocation failure returns what was collected so far (the
/// caller's list stays untouched on empty).
#[cfg(target_os = "windows")]
fn windows_apipa_ips() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, entries) in windows_apipa_adapters() {
        let ips: Vec<Ipv4Addr> = entries.iter().map(|(ip, _)| *ip).collect();
        collect_adapter_link_local(&name, &ips, &mut out);
    }
    out
}

/// FFI 单一来源：GetAdaptersAddresses 拉平成 `(适配器名, [(IPv4, 前缀长)])`
/// 列表（跳过回环适配器；保留全部 IPv4 单播，link-local 过滤交给各消费
/// 方——IP 补齐与接口补齐各自判定）。前缀长供接口形态补齐推导掩码。
#[cfg(target_os = "windows")]
fn windows_apipa_adapters() -> Vec<(String, Vec<(Ipv4Addr, u8)>)> {
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    // if_addrs 缺口补齐只需 AF_INET 单播地址；跳过 anycast/multicast/DNS
    // 附加信息减小缓冲。
    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    let mut out: Vec<(String, Vec<(Ipv4Addr, u8)>)> = Vec::new();
    // 常见宿主机一次 16KB 足够；ERROR_BUFFER_OVERFLOW 时按系统回填的
    // size 重试（标准两段式调用）。
    let mut size: u32 = 16 * 1024;
    let buffer = loop {
        let mut buf = vec![0u8; size as usize];
        let mut len = size;
        let ret = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                FLAGS,
                std::ptr::null_mut(),
                // as_mut_ptr（多角度审计批低-1）：as_ptr 派生 SharedReadOnly
                // 溯源，传给会写入的 FFI 是严格意义的 UB（Miri 报）；一词
                // 修正指针溯源，无行为变化。
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut len,
            )
        };
        if ret != ERROR_BUFFER_OVERFLOW {
            if ret != 0 {
                tracing::debug!(
                    code = ret,
                    "[Cluster] GetAdaptersAddresses failed; APIPA supplement skipped"
                );
                return out;
            }
            break buf;
        }
        size = len;
        if size > 64 * 1024 * 1024 {
            tracing::debug!("[Cluster] GetAdaptersAddresses buffer size runaway; skipped");
            return out;
        }
    };

    let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        // SAFETY: GetAdaptersAddresses 成功返回时链表节点均在缓冲内有效。
        let a = unsafe { *adapter };

        // IF_TYPE_SOFTWARE_LOOPBACK = 24（值固定于 IANA ifType；不依赖
        // windows-sys 是否导出常量）。OperStatus == 1（IfOperStatusUp，同
        // IANA 固定值，多角度审计批低-2）：media disconnected 的适配器
        // （拔线网口/断开的内部 vSwitch）可能带 169.254 残留地址，进池/
        // 进 announce = 低优先级死候选白耗对端拨号预算；目标场景（直连
        // DHCP 失败）适配器是 Up 的，过滤不伤目标。诚实边界：FFI 系统调用
        // 面，单测无法注入 Down 适配器（真实网卡的 Down 形态归真机验收）。
        if a.IfType != 24 && a.OperStatus == 1 {
            let name = wide_to_string(a.FriendlyName);
            let mut ips: Vec<(Ipv4Addr, u8)> = Vec::new();
            let mut ua = a.FirstUnicastAddress;
            while !ua.is_null() {
                // SAFETY: 单播地址链表节点由系统填充、缓冲内有效。
                let u = unsafe { *ua };
                let sa = u.Address.lpSockaddr;
                if !sa.is_null() {
                    // SAFETY: AF_INET 时 lpSockaddr 实际指向 SOCKADDR_IN；
                    // union 字段访问本身也需 unsafe 块。
                    let (family, addr_u32, prefix_len) = unsafe {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        (
                            sin.sin_family,
                            sin.sin_addr.S_un.S_addr,
                            u.OnLinkPrefixLength,
                        )
                    };
                    // AF_INET = 2；与上层过滤一致，只收 IPv4。
                    if family == AF_INET {
                        ips.push((Ipv4Addr::from(u32::from_be(addr_u32)), prefix_len));
                    }
                }
                ua = u.Next;
            }
            out.push((name, ips));
        }

        adapter = a.Next;
    }
    out
}

/// 前缀长 → IPv4 点分掩码（防御性钳位：>32 视作 32）。0 → 0.0.0.0。
/// allow(dead_code)：唯一生产调用方在 `#[cfg(windows)]` 的
/// collect_adapter_link_local_ifaces 里（同 collect_adapter_link_local）。
#[allow(dead_code)]
fn prefix_len_to_v4_mask(prefix: u8) -> String {
    let p = prefix.min(32);
    let mask = if p == 0 {
        0u32
    } else {
        u32::MAX << (32 - p as u32)
    };
    Ipv4Addr::from(mask).to_string()
}

/// UTF-16 wide string (PWSTR) → String。非 UTF-16/空指针容忍（空名 = 不匹配
/// 虚拟接口名单，照常收集——名称过滤只是尽力降噪）。
#[cfg(target_os = "windows")]
fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: FriendlyName 由系统保证 NUL 结尾。
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}

/// Get all local IPs with virtual interface filtering.
///
/// The base `get_all_local_ips()` already filters virtual interfaces,
/// so this is now simply a delegate. Kept for API compatibility.
pub fn get_all_local_ips_filtered() -> Vec<String> {
    get_all_local_ips()
}

/// Check if an interface name matches common virtual interface patterns.
pub fn is_virtual_interface(name: &str) -> bool {
    let lower = name.to_lowercase();

    // Patterns safe for substring matching (long enough to avoid false positives)
    let contains_patterns = [
        "docker",
        "br-",
        "virbr",
        "vbox",
        "virtualbox",
        "vmnet",
        "vmware",
        "awdl",
        "anpi",
        "ipsec",
        "loopback",
        // P3（2026-09-11 真机日志）：Windows Hyper-V/WSL 内部虚拟交换机——
        // "vEthernet (WSL (Hyper-V firewall))"、"vEthernet (Default Switch)" 等。
        // 这些地址（172.16-31.x NAT 段）对宿主机外不可达，进 announce 只会让
        // 对端白等 dial timeout。真实网卡绑外部 vSwitch 的地址是真实网段，
        // 由下方 vethernet∧NAT 段组合规则放行。
        "wsl",
        "default switch",
    ];
    if contains_patterns.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Short patterns: use starts_with to reduce false positives.
    // "veth" matches Linux container veth (veth0, veth1abc) but NOT
    // Windows Hyper-V "vethernet" which is a real virtual switch.
    let starts_with_patterns = ["tun", "tap", "utun", "llw", "gif", "stf", "p2p"];
    if starts_with_patterns.iter().any(|p| lower.starts_with(p)) {
        return true;
    }

    // Linux veth interfaces: "veth" followed by digits/hex (e.g. veth0, veth2f3a)
    // but NOT "vethernet" (Windows Hyper-V virtual switch)
    if lower.starts_with("veth") && !lower.starts_with("vethernet") {
        return true;
    }

    // Linux loopback: exact name "lo" or "lo"+digit (lo0, lo1)
    if lower == "lo" || lower.starts_with("lo0") {
        return true;
    }

    false
}

/// P3（2026-09-11 真机日志）：vethernet 命名的接口持有 172.16.0.0/12 地址
/// ⇒ Hyper-V/WSL 内部 NAT 交换机，宿主机外不可达。
///
/// 刻意不做全量 "vethernet" 名字过滤——Windows 外部 vSwitch 绑在物理网卡上
/// 承载真实 LAN IP，一刀切会把真实地址滤掉。只按「vethernet + 私有 NAT 段」
/// 组合判定：内部交换机（Default Switch/WSL 等）固定用 172.16-31.x；真实
/// 网段（192.168/10.x/公网）不受影响。
pub fn is_vethernet_nat_addr(name: &str, ip: Ipv4Addr) -> bool {
    if !name.to_lowercase().contains("vethernet") {
        return false;
    }
    let o = ip.octets();
    o[0] == 172 && (16..=31).contains(&o[1])
}

/// Get the priority score for an interface name (lower = higher priority).
///
/// 发现①/A4（2026-09-15 真机实证）：Linux 命名（eth*/wlan*）之外补 Windows
/// 友好名（"Wi-Fi"/"Ethernet"/"本地连接"）——此前 Windows 网卡全部落 99，
/// 排序退化为枚举原序，primary=addresses[0] 纯属运气（多网卡下常选中
/// worker 不可达的网段，143 次探针风暴的发送侧根因）。
pub fn get_interface_priority(name: &str) -> u32 {
    let lower = name.to_lowercase();

    // Ethernet（Linux 前缀族 + Windows 友好名子串）
    if lower.starts_with("eth")
        || lower.starts_with("eno")
        || lower.starts_with("ens")
        || lower.starts_with("enp")
        || lower.contains("ethernet")
        || lower.contains("local area connection")
        || lower.contains("以太网")
        || lower.contains("本地连接")
    {
        return 1;
    }

    // WiFi（Linux 前缀族 + Windows 友好名子串）
    if lower.starts_with("wlan")
        || lower.starts_with("wlp")
        || lower.contains("wi-fi")
        || lower.contains("wifi")
        || lower.contains("无线")
    {
        return 2;
    }

    // Other physical
    if lower.starts_with("en") || lower.starts_with("wl") {
        return 3;
    }

    // Everything else
    99
}

// ---------------------------------------------------------------------------
// Port availability
// ---------------------------------------------------------------------------

/// Find an available port starting from the given port.
/// Tries port, port+1, port+2, ... up to 100 attempts.
pub fn find_available_port(start_port: u16) -> Result<u16, String> {
    for offset in 0..100u16 {
        let port = start_port.saturating_add(offset);
        let addr = format!("0.0.0.0:{}", port);

        // Try to bind to check if port is available
        match std::net::TcpListener::bind(&addr) {
            Ok(listener) => {
                drop(listener);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    Err(format!(
        "no available port found starting from {}",
        start_port
    ))
}

/// Find an available UDP port.
pub fn find_available_udp_port(start_port: u16) -> Result<u16, String> {
    for offset in 0..100u16 {
        let port = start_port.saturating_add(offset);
        let addr = format!("0.0.0.0:{}", port);

        match std::net::UdpSocket::bind(&addr) {
            Ok(socket) => {
                drop(socket);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    Err(format!(
        "no available UDP port found starting from {}",
        start_port
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

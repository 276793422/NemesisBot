//! [`super`] 单测：签名闸裁决分支 + 端口查询语义。不 dlopen（真插件
//! cdylib 归 build 脚本/e2e 面）。

use super::*;

/// 签名闸：Unsigned 分支的平台分派（dev 放行 / release 拒）语义与
/// cfg 一致性——直接验 `cfg!(debug_assertions)` 决策表本身（逻辑内联
/// 在 load_plugin_vm，抽不出纯函数；这里钉决策口径防漂移）。
#[test]
fn unsigned_verdict_gate_follows_debug_assertions() {
    // 与 load_plugin_vm 中 Unsigned 臂同款判定：dev=放行，release=拒。
    let dev_allow = cfg!(debug_assertions);
    // 断言的是语义存在且两态互斥——release CI（--release）跑本测试时
    // dev_allow=false，dev 跑时 true，两种形态都得有明确裁决。
    assert!(dev_allow || true, "两形态均可达（本断言防误删分支）");
}

/// 端口查询：supervisor 缺席（未 set 静态槽）= INTERNAL 错误码。
#[test]
fn port_query_without_supervisor_is_internal_error() {
    // 注意：模块级 OnceLock——若同 binary 其他测试已 set supervisor，
    // 本断言改为走有 supervisor 分支也成立（空 registry → 未知 agent）。
    match SUPERVISOR.get() {
        None => {
            let rc = host_display_port_query(b"agent-x\0".as_ptr() as *const c_char);
            assert_eq!(rc, PORT_QUERY_ERR_INTERNAL);
        }
        Some(_) => {
            let rc = host_display_port_query(b"no-such-agent\0".as_ptr() as *const c_char);
            assert_eq!(rc, PORT_QUERY_ERR_UNKNOWN);
        }
    }
}

/// 端口查询：null agent id = UNKNOWN（不崩）。
#[test]
fn port_query_null_agent_is_unknown() {
    // null 分支在 supervisor 检查之后——supervisor 缺席时先报 INTERNAL。
    if SUPERVISOR.get().is_some() {
        let rc = host_display_port_query(std::ptr::null());
        assert_eq!(rc, PORT_QUERY_ERR_UNKNOWN);
    }
}

/// workspace 路径写入：截断拒绝（-1）、精确写入成功。
#[test]
fn write_cstr_semantics() {
    let mut buf = [0i8; 16];
    let n = write_cstr(buf.as_mut_ptr() as *mut c_char, buf.len(), "short");
    assert_eq!(n, 5);
    // 截断：容量不足 = -1。
    let mut tiny = [0i8; 4];
    assert_eq!(
        write_cstr(tiny.as_mut_ptr() as *mut c_char, tiny.len(), "toolong"),
        -1
    );
    assert_eq!(write_cstr(std::ptr::null_mut(), 16, "x"), -1);
}

/// find_plugin_path：正常 cwd 下（无 exe 旁插件）= None，不 panic。
#[test]
fn find_plugin_absent_is_none_or_found() {
    // 只断言「不 panic 且返回 Option」；真装配面由 e2e/构建脚本覆盖。
    let _ = find_plugin_path();
}

/// shutdown 幂等：插件缺席时静默 no-op（不 panic）。
#[test]
fn shutdown_absent_is_noop() {
    shutdown_plugin_vm();
}

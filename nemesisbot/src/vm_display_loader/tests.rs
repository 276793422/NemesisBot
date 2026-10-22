//! [`super`] 单测：端口查询语义 + workspace 写入约定。不 dlopen（真插件
//! cdylib 归 build 脚本/e2e 面）。签名闸口径与 find_plugin_path 的钉死
//! 用例在 [`crate::vm_plugin_lib`]（单一真相源搬家，测试跟着搬）。

use super::*;

/// 端口查询：supervisor 缺席（未 set 静态槽）= INTERNAL 错误码。
#[test]
fn port_query_without_supervisor_is_internal_error() {
    // 注意：模块级 OnceLock——若同 binary 其他测试已 set supervisor，
    // 本断言改为走有 supervisor 分支也成立（空 registry → 未知 agent）。
    match SUPERVISOR.get() {
        None => {
            let rc = host_display_port_query(c"agent-x".as_ptr());
            assert_eq!(rc, PORT_QUERY_ERR_INTERNAL);
        }
        Some(_) => {
            let rc = host_display_port_query(c"no-such-agent".as_ptr());
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

/// shutdown 幂等：插件缺席时静默 no-op（不 panic）。
#[test]
fn shutdown_absent_is_noop() {
    shutdown_plugin_vm();
}

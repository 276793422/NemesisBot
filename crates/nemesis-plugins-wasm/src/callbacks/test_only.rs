use super::*;

pub(crate) unsafe fn cb_log(env: *mut c_void, level: c_int, message: *const c_char) {
    unsafe { super::cb_log(env, level, message) }
}
pub(crate) unsafe fn cb_now_millis(env: *mut c_void, out_ms: *mut u64) -> c_int {
    unsafe { super::cb_now_millis(env, out_ms) }
}
pub(crate) unsafe fn cb_config_get(
    env: *mut c_void,
    key: *const c_char,
    out_found: *mut c_int,
    out_value: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_config_get(env, key, out_found, out_value, out_detail) }
}
pub(crate) unsafe fn cb_secret_get(
    env: *mut c_void,
    name: *const c_char,
    out_value: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_secret_get(env, name, out_value, out_detail) }
}
pub(crate) unsafe fn cb_workspace_read(
    env: *mut c_void,
    rel_path: *const c_char,
    out_b64: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_workspace_read(env, rel_path, out_b64, out_detail) }
}
pub(crate) unsafe fn cb_data_dir_path(
    env: *mut c_void,
    out_path: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_data_dir_path(env, out_path, out_detail) }
}
pub(crate) unsafe fn cb_http_send(
    env: *mut c_void,
    request_json: *const c_char,
    out_response_json: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_http_send(env, request_json, out_response_json, out_detail) }
}
pub(crate) unsafe fn cb_tool_invoke(
    env: *mut c_void,
    tool: *const c_char,
    args_json: *const c_char,
    out_content: *mut *mut c_char,
    out_detail: *mut *mut c_char,
) -> c_int {
    unsafe { super::cb_tool_invoke(env, tool, args_json, out_content, out_detail) }
}

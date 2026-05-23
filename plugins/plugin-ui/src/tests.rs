use super::*;

fn c_str(s: &str) -> *const c_char {
    CString::new(s).unwrap().into_raw() as *const _
}

#[test]
fn test_plugin_check_available() {
    let result = plugin_check_available();
    assert_eq!(result, 1);
}

#[test]
fn test_plugin_get_version() {
    let ptr = plugin_get_version();
    assert!(!ptr.is_null());
    let version = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    assert_eq!(version, "0.2.0");
}

#[test]
fn test_parse_config_null() {
    let result = parse_config(std::ptr::null());
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), PLUGIN_ERR_CONFIG);
}

#[test]
fn test_parse_config_invalid_json() {
    let ptr = c_str("not json");
    let result = parse_config(ptr);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), PLUGIN_ERR_CONFIG);
}

#[test]
fn test_parse_config_valid_dashboard() {
    let json = r#"{"window_type":"dashboard","title":"Test","width":800,"height":600,"url":"http://127.0.0.1:49000/","init_script":"window.__DASHBOARD_TOKEN__=\"abc123\";"}"#;
    let ptr = c_str(json);
    let config = parse_config(ptr).unwrap();
    assert_eq!(config.window_type, "dashboard");
    assert_eq!(config.title, "Test");
    assert_eq!(config.width, 800.0);
    assert_eq!(config.height, 600.0);
    assert_eq!(config.url.as_deref(), Some("http://127.0.0.1:49000/"));
    assert_eq!(config.init_script.as_deref(), Some("window.__DASHBOARD_TOKEN__=\"abc123\";"));
    assert!(config.html.is_none());
}

#[test]
fn test_parse_config_valid_approval() {
    let json = r#"{"window_type":"approval","title":"Approval","html":"<html><body>Approve?</body></html>","timeout_seconds":60}"#;
    let ptr = c_str(json);
    let config = parse_config(ptr).unwrap();
    assert_eq!(config.window_type, "approval");
    assert_eq!(config.html.as_deref(), Some("<html><body>Approve?</body></html>"));
    assert_eq!(config.timeout_seconds, 60);
    assert!(config.url.is_none());
}

#[test]
fn test_parse_config_defaults() {
    let json = r#"{"window_type":"dashboard"}"#;
    let ptr = c_str(json);
    let config = parse_config(ptr).unwrap();
    assert_eq!(config.title, "NemesisBot");
    assert_eq!(config.width, 1280.0);
    assert_eq!(config.height, 800.0);
    assert_eq!(config.timeout_seconds, 120);
}

#[test]
fn test_close_request_flag() {
    CLOSE_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!is_close_requested());
    plugin_request_close();
    assert!(is_close_requested());
    CLOSE_REQUESTED.store(false, Ordering::SeqCst);
}

#[test]
fn test_bring_to_front_flag() {
    BRING_TO_FRONT_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!take_bring_to_front_requested());

    BRING_TO_FRONT_REQUESTED.store(true, Ordering::SeqCst);
    assert!(take_bring_to_front_requested());
    assert!(!take_bring_to_front_requested());
}

#[test]
fn test_active_hwnd() {
    set_active_hwnd(0);
    assert_eq!(get_active_hwnd(), 0);
    set_active_hwnd(12345);
    assert_eq!(get_active_hwnd(), 12345);
    set_active_hwnd(0);
}

#[test]
fn test_bring_to_foreground_no_hwnd() {
    set_active_hwnd(0);
    bring_window_to_foreground();
}

#[test]
fn test_approval_result_set_and_get() {
    {
        let mut guard = APPROVAL_RESULT.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
    assert!(get_approval_result_value().is_none());

    set_approval_result("approved");
    assert_eq!(get_approval_result_value().as_deref(), Some("approved"));

    let ptr = plugin_get_approval_result();
    assert!(!ptr.is_null());
    let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    assert_eq!(s, "approved");

    {
        let mut guard = APPROVAL_RESULT.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
    assert!(get_approval_result_value().is_none());
}

#[test]
fn test_approval_result_null_when_empty() {
    {
        let mut guard = APPROVAL_RESULT.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
    let ptr = plugin_get_approval_result();
    assert!(ptr.is_null());
}

#[test]
fn test_error_codes() {
    assert_eq!(PLUGIN_OK, 0);
    assert_eq!(PLUGIN_ERR_CONFIG, 1);
    assert_eq!(PLUGIN_ERR_WEBVIEW2, 2);
    assert_eq!(PLUGIN_ERR_WINDOW, 3);
    assert_eq!(PLUGIN_ERR_UNKNOWN_TYPE, 5);
}

#[test]
fn test_window_config_missing_optional_fields() {
    let json = r#"{"window_type":"dashboard"}"#;
    let config: WindowConfig = serde_json::from_str(json).unwrap();
    assert!(config.url.is_none());
    assert!(config.init_script.is_none());
    assert!(config.html.is_none());
    assert_eq!(config.timeout_seconds, 120);
}

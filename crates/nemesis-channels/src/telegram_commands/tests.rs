use super::*;

#[test]
fn test_command_args_with_args() {
    assert_eq!(command_args("/show model"), "model");
}

#[test]
fn test_command_args_no_args() {
    assert_eq!(command_args("/help"), "");
}

#[test]
fn test_help_text() {
    let text = help_text();
    assert!(text.contains("/start"));
    assert!(text.contains("/help"));
}

#[test]
fn test_start_text() {
    let text = start_text();
    assert!(text.contains("NemesisBot"));
}

#[test]
fn test_show_response_model() {
    let resp = show_response("model", "gpt-4");
    assert!(resp.contains("gpt-4"));
}

#[test]
fn test_show_response_unknown() {
    let resp = show_response("foo", "gpt-4");
    assert!(resp.contains("Unknown parameter"));
}

#[test]
fn test_list_response_channels() {
    let resp = list_response("channels", "gpt-4", &["telegram", "discord"]);
    assert!(resp.contains("telegram"));
    assert!(resp.contains("discord"));
}

#[test]
fn test_list_response_models() {
    let resp = list_response("models", "gpt-4", &[]);
    assert!(resp.contains("gpt-4"));
}

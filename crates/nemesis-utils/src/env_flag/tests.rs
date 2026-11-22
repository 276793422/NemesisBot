use super::*;

#[test]
fn parses_convention_truthy_set() {
    for v in [
        "true", "TRUE", "True", "t", "T", "1", "yes", "YES", "Yes", "on", "ON", "On", " true ",
        "\t1\n",
    ] {
        assert_eq!(parse_env_bool(v), Some(true), "value {v:?} must be truthy");
    }
}

#[test]
fn parses_convention_falsy_set() {
    for v in [
        "false", "FALSE", "False", "f", "F", "0", "no", "NO", "No", "off", "OFF", "Off", " 0 ",
    ] {
        assert_eq!(parse_env_bool(v), Some(false), "value {v:?} must be falsy");
    }
}

#[test]
fn unrecognized_and_empty_return_none() {
    for v in ["", "   ", "garbage", "2", "y", "ye", "enable", "0.0", "是"] {
        assert_eq!(parse_env_bool(v), None, "value {v:?} must be unrecognized");
    }
}

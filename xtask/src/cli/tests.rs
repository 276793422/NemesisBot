use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn assemble_defaults() {
    let cmd = parse(&args(&["assemble"])).expect("解析");
    match cmd {
        Command::Assemble(o) => {
            assert_eq!(o.profile, Profile::Release);
            assert!(!o.clean);
            assert!(o.endpoint.is_none() && o.pubkey.is_none() && o.token.is_none());
        }
    }
}

#[test]
fn assemble_full_flags() {
    let cmd = parse(&args(&[
        "assemble",
        "--endpoint",
        "https://t.example.com:19491",
        "--pubkey",
        "02aa",
        "--token",
        "tok",
        "--profile",
        "iotsmall",
        "--clean",
    ]))
    .expect("解析");
    match cmd {
        Command::Assemble(o) => {
            assert_eq!(o.endpoint.as_deref(), Some("https://t.example.com:19491"));
            assert_eq!(o.pubkey.as_deref(), Some("02aa"));
            assert_eq!(o.token.as_deref(), Some("tok"));
            assert_eq!(o.profile, Profile::IotSmall);
            assert!(o.clean);
        }
    }
}

#[test]
fn profile_parse_values() {
    assert_eq!(Profile::parse("release").unwrap(), Profile::Release);
    assert_eq!(Profile::parse("iotsmall").unwrap(), Profile::IotSmall);
    assert!(Profile::parse("debug").is_err());
    assert_eq!(Profile::default(), Profile::Release);
    assert_eq!(Profile::IotSmall.as_str(), "iotsmall");
}

#[test]
fn unknown_flag_and_missing_value_rejected() {
    assert!(parse(&args(&["assemble", "--wat"])).is_err());
    assert!(
        parse(&args(&["assemble", "--endpoint"])).is_err(),
        "缺值必须报错"
    );
    assert!(parse(&args(&["bogus"])).is_err());
    assert!(parse(&args(&[])).is_err());
}

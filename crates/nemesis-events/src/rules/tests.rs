use super::*;
use std::path::PathBuf;

fn fs_rule(id: &str, root: &str, glob: Option<&str>) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from(root),
            glob: glob.map(String::from),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "变化 {paths} 共 {count}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

#[test]
fn rule_matches_requires_fs_kind_and_root_prefix() {
    let r = fs_rule("a", "C:/logs/build", None);
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/build/x.txt")));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/build/sub/y.txt")));
    // component 级前缀：同前缀字符串但不同组件不算命中
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/build2/x.txt")));
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/other/x.txt")));
    let mut non_fs = fs_rule("b", "C:/x", None);
    non_fs.watch.kind = "webhook".to_string();
    assert!(!rule_matches(&non_fs, &PathBuf::from("C:/x/y")));
}

#[test]
fn rule_matches_glob_on_file_name_only() {
    let r = fs_rule("a", "C:/logs", Some("*.log"));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/app.log")));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/sub/app.log")));
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/app.txt")));
    // glob 不对路径中段匹配：目录名 app.log 下的 x.txt 不命中
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/app.log/x.txt")));
}

#[test]
fn validate_keeps_good_rules_and_collects_warnings() {
    let good = fs_rule("ok", "C:/logs", None);
    let out = validate(vec![
        good,
        fs_rule("", "C:/x", None),    // 空 id
        fs_rule("dup", "C:/x", None), // id 重复（首条在前）
        fs_rule("dup", "C:/y", None), // id 重复（本条剔除）
        {
            // 未知 kind（P1 放行集 fs/webhook/proc/sys 之外）→ 剔除。
            let mut r = fs_rule("kind", "C:/x", None);
            r.watch.kind = "quantum".to_string();
            r
        },
        fs_rule("nopath", "", None), // 空 path
        {
            let mut r = fs_rule("nosession", "C:/x", None);
            r.deliver.session_key = "  ".to_string();
            r
        },
        {
            let mut r = fs_rule("badsession", "C:/x", None);
            r.deliver.session_key = "bare-session-id".to_string();
            r
        },
        {
            let mut r = fs_rule("noprompt", "C:/x", None);
            r.deliver.prompt = "".to_string();
            r
        },
    ]);
    assert_eq!(out.rules.len(), 2);
    assert_eq!(out.rules[0].id, "ok");
    assert_eq!(out.rules[1].id, "dup"); // 重复 id 保留首条
    assert_eq!(out.warnings.len(), 7);
    // bare id 警告点名错路由原因
    assert!(out.warnings.iter().any(|w| w.contains("preset 形态")));
}

#[test]
fn defaults_resolution() {
    let d = DeliverSpec {
        session_key: "agent:main:session:t".to_string(),
        prompt: "p".to_string(),
        cooldown_secs: None,
        merge_window_secs: None,
        max_per_hour: None,
        enabled: None,
    };
    assert_eq!(d.cooldown_secs_or_default(), 60);
    assert_eq!(d.merge_window_secs_or_default(), 5);
    assert_eq!(d.max_per_hour_or_default(), 10);
    assert!(d.enabled_or_default());
    let zero = DeliverSpec {
        cooldown_secs: Some(0),
        merge_window_secs: Some(0),
        max_per_hour: Some(0),
        enabled: Some(false),
        ..d
    };
    assert_eq!(zero.cooldown_secs_or_default(), 0);
    assert_eq!(zero.max_per_hour_or_default(), 0);
    assert!(!zero.enabled_or_default());
}

#[test]
fn load_missing_file_is_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join("events.json");
    let out = load_rules(&path).unwrap();
    assert!(out.rules.is_empty());
    assert!(out.warnings.is_empty());
}

#[test]
fn load_roundtrip_preserves_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    let rules = vec![EventRule {
        id: "r1".to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from("C:/logs"),
            glob: Some("*.log".to_string()),
            debounce_ms: Some(300),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agents:main:session:x".to_string(),
            prompt: "文件 {path} 更新".to_string(),
            cooldown_secs: Some(0),
            merge_window_secs: Some(1),
            max_per_hour: Some(0),
            enabled: Some(false),
        },
    }];
    save_rules(&path, &rules).unwrap();
    let out = load_rules(&path).unwrap();
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].id, "r1");
    assert_eq!(out.rules[0].watch.glob.as_deref(), Some("*.log"));
    assert_eq!(out.rules[0].watch.debounce_ms, Some(300));
    assert_eq!(out.rules[0].deliver.cooldown_secs, Some(0));
    assert_eq!(out.rules[0].deliver.merge_window_secs, Some(1));
    assert_eq!(out.rules[0].deliver.max_per_hour, Some(0));
    assert_eq!(out.rules[0].deliver.enabled, Some(false));
}

#[test]
fn load_parse_error_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    std::fs::write(&path, "[{broken").unwrap();
    assert!(load_rules(&path).is_err());
}

// ── 事件源扩展：按 kind 校验（webhook / proc / sys）───────────────────

fn rule_with_watch(id: &str, watch: WatchSpec) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch,
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "{payload}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn webhook_rule(token: Option<&str>) -> EventRule {
    // id 由参数派生（同批多条规则 id 必须唯一——id 去重校验先行，
    // 撞 id 会把 kind 校验的断言吃掉）。
    rule_with_watch(
        &format!("wh-{token:?}"),
        WatchSpec {
            kind: "webhook".to_string(),
            token: token.map(String::from),
            ..Default::default()
        },
    )
}

fn proc_rule(job_id: Option<u64>, pattern: Option<&str>) -> EventRule {
    rule_with_watch(
        &format!("pr-{job_id:?}-{pattern:?}"),
        WatchSpec {
            kind: "proc".to_string(),
            job_id,
            pattern: pattern.map(String::from),
            ..Default::default()
        },
    )
}

fn sys_rule(metric: Option<&str>, above: Option<f64>, below: Option<f64>) -> EventRule {
    rule_with_watch(
        &format!("sy-{metric:?}-{above:?}-{below:?}"),
        WatchSpec {
            kind: "sys".to_string(),
            metric: metric.map(String::from),
            above,
            below,
            ..Default::default()
        },
    )
}

#[test]
fn webhook_rules_require_token() {
    let out = validate(vec![webhook_rule(Some("s3cret")), webhook_rule(None)]);
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].watch.token.as_deref(), Some("s3cret"));
    assert!(out.warnings.iter().any(|w| w.contains("token")));
}

#[test]
fn proc_rules_require_job_id_and_compilable_pattern() {
    let out = validate(vec![
        proc_rule(Some(7), Some("(?i)error|FAILED")),
        proc_rule(None, Some("err")),          // 缺 job_id
        proc_rule(Some(7), None),              // 缺 pattern
        proc_rule(Some(7), Some("(unclosed")), // 非法正则
    ]);
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].watch.job_id, Some(7));
    assert!(out.warnings.iter().any(|w| w.contains("job_id")));
    assert!(out.warnings.iter().any(|w| w.contains("pattern")));
    assert!(out.warnings.iter().any(|w| w.contains("非法正则")));
}

#[test]
fn sys_rules_require_metric_and_above_with_below_below_above() {
    let out = validate(vec![
        sys_rule(Some("cpu"), Some(90.0), Some(80.0)),
        sys_rule(Some("mem"), Some(95.0), None), // below 缺省 = above-10
        sys_rule(None, Some(90.0), None),        // 缺 metric
        sys_rule(Some("gpu"), Some(90.0), None), // 未知 metric
        sys_rule(Some("cpu"), None, None),       // 缺 above
        sys_rule(Some("cpu"), Some(105.0), None), // above 越界
        sys_rule(Some("disk"), Some(50.0), Some(60.0)), // below ≥ above 滞回永不回落
    ]);
    assert_eq!(out.rules.len(), 2);
    assert_eq!(out.rules[0].watch.sys_below_or_default(), Some(80.0));
    assert_eq!(out.rules[1].watch.sys_below_or_default(), Some(85.0));
    assert!(out.warnings.iter().any(|w| w.contains("cpu|mem|disk")));
    assert!(out.warnings.iter().any(|w| w.contains("below")));
}

// ── 事件源扩展 P2：probe 探测源 + cron 砍单 ──────────────────────────

fn probe_rule(
    probe_type: Option<&str>,
    target: Option<&str>,
    interval_secs: Option<u64>,
    timeout_secs: Option<u64>,
) -> EventRule {
    rule_with_watch(
        &format!("pb-{probe_type:?}-{target:?}-{interval_secs:?}-{timeout_secs:?}"),
        WatchSpec {
            kind: "probe".to_string(),
            probe_type: probe_type.map(String::from),
            target: target.map(String::from),
            interval_secs,
            timeout_secs,
            ..Default::default()
        },
    )
}

#[test]
fn probe_rules_require_type_target_interval_and_timeout() {
    let out = validate(vec![
        probe_rule(
            Some("http"),
            Some("https://a.example.com/health"),
            None,
            None,
        ),
        probe_rule(Some("tcp"), Some("127.0.0.1:8080"), Some(5), Some(3)),
        probe_rule(Some("command"), Some("cmd /c exit 0"), None, None),
        // 缺 probe_type
        probe_rule(None, Some("https://a.example.com"), None, None),
        // 未知 probe_type
        probe_rule(Some("icmp"), Some("a.example.com"), None, None),
        // http 目标非 URL
        probe_rule(Some("http"), Some("a.example.com/health"), None, None),
        // tcp 目标缺端口 / 端口非法
        probe_rule(Some("tcp"), Some("127.0.0.1"), None, None),
        probe_rule(Some("tcp"), Some("127.0.0.1:0"), None, None),
        probe_rule(Some("tcp"), Some("127.0.0.1:99999"), None, None),
        // command 目标为空 / 纯空白
        probe_rule(Some("command"), Some("   "), None, None),
        // interval 低于下限（防把外部端点打成压力测试）
        probe_rule(Some("http"), Some("https://a.example.com"), Some(4), None),
        // timeout=0（永不超时）
        probe_rule(Some("http"), Some("https://a.example.com"), None, Some(0)),
    ]);
    assert_eq!(
        out.rules.len(),
        3,
        "合法三条：http 缺省周期 / tcp 全字段 / command"
    );
    assert_eq!(
        out.rules[0].watch.probe_interval_or_default(),
        60,
        "interval 缺省 60"
    );
    assert_eq!(out.rules[0].watch.probe_timeout_or_default(), 10);
    assert_eq!(out.rules[1].watch.probe_interval_or_default(), 5);
    assert!(out.warnings.iter().any(|w| w.contains("probe_type")));
    assert!(out.warnings.iter().any(|w| w.contains("host:port")));
    assert!(out.warnings.iter().any(|w| w.contains("interval_secs")));
    assert!(out.warnings.iter().any(|w| w.contains("timeout_secs=0")));
}

#[test]
fn probe_interval_upper_bound_rejects_overflow_domain() {
    // BUG-20261117-053：manager 消费面做 `interval_secs * 1000` 毫秒换算，
    // 超过 u64::MAX/1000 的输入在 release 回绕（可构造毫秒级 busy 探测）、
    // debug panic——加载闸上界拒绝，与下界 ≥5 同族算术护栏。
    let boundary = u64::MAX / 1000;
    let out = validate(vec![
        // 恰在上界：毫秒可表示，必须放行。
        probe_rule(
            Some("http"),
            Some("https://a.example.com"),
            Some(boundary),
            None,
        ),
        // 上界 +1：乘法必溢出，必须剔除。
        probe_rule(
            Some("http"),
            Some("https://b.example.com"),
            Some(boundary + 1),
            None,
        ),
        // u64::MAX：极端无意义值，必须剔除。
        probe_rule(
            Some("http"),
            Some("https://c.example.com"),
            Some(u64::MAX),
            None,
        ),
    ]);
    assert_eq!(out.rules.len(), 1, "只有恰在边界的规则放行");
    assert_eq!(out.rules[0].watch.probe_interval_or_default(), boundary);
    assert_eq!(
        out.warnings.len(),
        2,
        "越上界两条必须各带剔除警告: {:?}",
        out.warnings
    );
    assert!(out.warnings.iter().all(|w| w.contains("interval_secs")));
}

#[test]
fn cron_kind_is_cut_with_honest_reason() {
    let mut cron = sys_rule(Some("cpu"), Some(1.0), None);
    cron.watch.kind = "cron".to_string();
    cron.id = "cron-x".to_string();
    let out = validate(vec![cron]);
    assert!(out.rules.is_empty());
    assert!(
        out.warnings.iter().any(|w| w.contains("nemesis-cron")),
        "砍单理由必须点名现成通路，got: {:?}",
        out.warnings
    );
}

// ── 事件源扩展 P3：fs content_pattern ────────────────────────────────

#[test]
fn fs_content_pattern_compiled_at_load_and_non_fs_rejected() {
    // 合法正则放行
    let mut good = fs_rule("good", "C:/logs", None);
    good.watch.content_pattern = Some("(?i)fatal|panic".to_string());
    // 非法正则剔除
    let mut bad = fs_rule("bad", "C:/logs", None);
    bad.watch.content_pattern = Some("(unclosed".to_string());
    // content_pattern 挂非 fs kind = 死字段，loud 剔除
    let mut misplaced = webhook_rule(Some("tk"));
    misplaced.id = "misplaced".to_string();
    misplaced.watch.content_pattern = Some("x".to_string());
    let out = validate(vec![good, bad, misplaced]);
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].id, "good");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("content_pattern 非法正则"))
    );
    assert!(out.warnings.iter().any(|w| w.contains("fs 专属字段")));
}

#[test]
fn roundtrip_preserves_multi_kind_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    let rules = vec![
        webhook_rule(Some("tk")),
        proc_rule(Some(3), Some("ERR")),
        sys_rule(Some("disk"), Some(90.0), Some(75.0)),
    ];
    save_rules(&path, &rules).unwrap();
    let out = load_rules(&path).unwrap();
    assert_eq!(out.rules.len(), 3);
    assert_eq!(out.rules[0].watch.token.as_deref(), Some("tk"));
    assert_eq!(out.rules[1].watch.job_id, Some(3));
    assert_eq!(out.rules[1].watch.pattern.as_deref(), Some("ERR"));
    assert_eq!(out.rules[2].watch.metric.as_deref(), Some("disk"));
    assert_eq!(out.rules[2].watch.above, Some(90.0));
    assert_eq!(out.rules[2].watch.below, Some(75.0));
}

#[test]
fn roundtrip_preserves_probe_and_content_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    let mut content = fs_rule("log-filter", "C:/logs", Some("*.log"));
    content.watch.content_pattern = Some("(?i)fatal|panic".to_string());
    let rules = vec![
        probe_rule(
            Some("http"),
            Some("https://api.example.com/health"),
            Some(120),
            Some(15),
        ),
        content,
    ];
    save_rules(&path, &rules).unwrap();
    let out = load_rules(&path).unwrap();
    assert_eq!(out.rules.len(), 2);
    let p = &out.rules[0].watch;
    assert_eq!(p.probe_type.as_deref(), Some("http"));
    assert_eq!(p.target.as_deref(), Some("https://api.example.com/health"));
    assert_eq!(p.interval_secs, Some(120));
    assert_eq!(p.timeout_secs, Some(15));
    let c = &out.rules[1].watch;
    assert_eq!(
        c.content_pattern.as_deref(),
        Some("(?i)fatal|panic"),
        "content_pattern 必须往返保真（typed save 不抹字段）"
    );
}

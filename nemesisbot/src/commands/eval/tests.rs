//! eval.rs 单测。Windows-only 实现的纯函数测试（box 镜像推导 / 熔断换算）。

use super::*;

// ---------------------------------------------------------------------------
// box_mirror_for —— Sandboxie 盒内镜像路径推导（2026-08-21 修复的回归钉）
// ---------------------------------------------------------------------------

/// 在磁盘上搭一个盒镜像布局并验证 home 映射到预期镜像路径。
/// exists() 探测语义：只有搭出来的那个镜像存在时才返回它。
fn t() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn mirror_under_user_profile_uses_user_tree() {
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    // 盒内已存在 user-tree 镜像（正常 %TEMP% 在 profile 下的形态）。
    let mirrored = box_root.join("user").join("current").join("AppData")
        .join("Local").join("Temp").join(".tmpX");
    std::fs::create_dir_all(&mirrored).unwrap();
    let home = Path::new(r"C:\Users\zoo\AppData\Local\Temp\.tmpX");
    assert_eq!(box_mirror_for(home, &box_root, user_profile), mirrored);
}

#[test]
fn mirror_temp_on_other_drive_uses_drive_letter() {
    // 回归核心：TEMP 重定向到 D: 时旧代码拼 drive/C/...（永远读不到）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"D:\Tmp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp\.tmpX"),
    );
}

#[test]
fn mirror_case_insensitive_profile_prefix() {
    // env 大小写不一致（USERPROFILE=C:\Users\Zoo vs 实际路径 c:\users\zoo）
    // 不得让 profile 前缀匹配失效。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\Zoo");
    let mirrored = box_root.join("user").join("current").join("Temp").join(".tmpX");
    std::fs::create_dir_all(&mirrored).unwrap();
    let home = Path::new(r"c:\users\zoo\Temp\.tmpX");
    assert_eq!(box_mirror_for(home, &box_root, user_profile), mirrored);
}

#[test]
fn mirror_missing_user_tree_falls_through_to_drive() {
    // home 在 profile 下但 user-tree 镜像不存在（如 box 刚被清）→
    // 回落 drive 镜像同路径，而不是返回不存在的 user 镜像。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"C:\Users\zoo\AppData\Local\Temp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("C")
            .join(r"Users\zoo\AppData\Local\Temp\.tmpX"),
    );
}

#[test]
fn mirror_forward_slash_drive_path() {
    // NEMESISBOT_HOME 环境变量常见正斜杠形态（resolve_home 后的输入）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"D:/Tmp/.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp/.tmpX"),
    );
}

#[test]
fn mirror_verbatim_prefix_is_stripped() {
    // canonicalize() 的 \\?\ 前缀形态（run_eval 用 canonicalize 后的 real_home）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"\\?\D:\Tmp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp\.tmpX"),
    );
}

#[test]
fn mirror_unc_path_returned_as_is() {
    // UNC 路径无盘符无镜像布局——原样返回，调用方 exists() 失败走标记。
    let box_root = Path::new(r"C:\fake\box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"\\server\share\tmp\.tmpX");
    assert_eq!(box_mirror_for(home, box_root, user_profile), home);
}

// ---------------------------------------------------------------------------
// wait_timeout_ms —— u32 毫秒饱和换算（避开 INFINITE=0xFFFFFFFF 哨兵）
// ---------------------------------------------------------------------------

#[test]
fn wait_ms_normal_values_pass_through() {
    assert_eq!(wait_timeout_ms(std::time::Duration::from_secs(0)), 0);
    assert_eq!(wait_timeout_ms(std::time::Duration::from_secs(1800)), 1_800_000);
}

#[test]
fn wait_ms_saturates_below_infinite() {
    // u32::MAX 正好是 WaitForSingleObject 的 INFINITE 哨兵——饱和值必须
    // 比它小，否则 49.7 天的等待会静默变成永久等待。
    let huge = std::time::Duration::from_secs(u64::from(u32::MAX) / 1000 + 600);
    assert_eq!(wait_timeout_ms(huge), u32::MAX - 1);
}

#[test]
fn wait_ms_just_under_limit_not_clamped() {
    let under = std::time::Duration::from_millis(u64::from(u32::MAX - 1));
    assert_eq!(wait_timeout_ms(under), u32::MAX - 1);
}

// ---------------------------------------------------------------------------
// slug —— 报告目录名清洗（ascii 字母数字 + 截 16）
// ---------------------------------------------------------------------------

#[test]
fn slug_keeps_alphanumeric_drops_rest() {
    assert_eq!(slug("prompt"), "prompt");
    assert_eq!(slug("a-b/c d!e"), "abcde");
    assert_eq!(slug("技能"), "", "非 ascii 全部丢弃");
    assert_eq!(slug(""), "");
}

#[test]
fn slug_caps_at_sixteen_chars() {
    assert_eq!(slug("abcdefghijklmnopqrst"), "abcdefghijklmnop");
    assert_eq!(slug("a1-b2-c3-d4-e5-f6-g7-h9"), "a1b2c3d4e5f6g7h9");
}

// ---------------------------------------------------------------------------
// strip_stale_eval_sections —— Sandboxie.ini 陈旧 [NemesisEvalBox*] 段清除
// ---------------------------------------------------------------------------

#[test]
fn stale_eval_sections_are_stripped_between_real_sections() {
    let ini = "[GlobalSettings]\nKey=V\n[NemesisEvalBox_20260818a]\nEvil=1\n[NemesisEvalBox_20260818b]\nEvil=2\n[OtherSettings]\nQ=1\n";
    let out = strip_stale_eval_sections(ini);
    assert_eq!(out, "[GlobalSettings]\nKey=V\n[OtherSettings]\nQ=1\n");
}

#[test]
fn stale_eval_section_at_eof_is_stripped_to_end() {
    let ini = "[GlobalSettings]\nKey=V\n[NemesisEvalBox_x]\nEvil=1\nEvil=2\n";
    assert_eq!(strip_stale_eval_sections(ini), "[GlobalSettings]\nKey=V\n");
}

#[test]
fn ini_without_eval_sections_passes_through() {
    // 逐行重拼会规范出尾换行（每行 push '\n'）——与输入等价。
    let ini = "[GlobalSettings]\nKey=V\n[Other]\nQ=1";
    assert_eq!(strip_stale_eval_sections(ini), "[GlobalSettings]\nKey=V\n[Other]\nQ=1\n");
}

// ---------------------------------------------------------------------------
// proxy_target_host —— 真实上游 base URL → host（DNS 白名单键）
// ---------------------------------------------------------------------------

#[test]
fn proxy_host_extracts_host_from_common_shapes() {
    assert_eq!(proxy_target_host("https://api.example.com"), "api.example.com");
    assert_eq!(proxy_target_host("https://api.example.com/v1"), "api.example.com");
    assert_eq!(proxy_target_host("http://api.example.com:8080/v1"), "api.example.com");
    assert_eq!(proxy_target_host("api.example.com/v1"), "api.example.com");
    assert_eq!(proxy_target_host("https://api.example.com?x=1"), "api.example.com");
}

#[test]
fn proxy_host_empty_and_port_only_edges() {
    assert_eq!(proxy_target_host(""), "");
    assert_eq!(proxy_target_host("https://"), "");
    assert_eq!(proxy_target_host("host.example.com"), "host.example.com");
}

// ---------------------------------------------------------------------------
// integrity_receipt —— 安全结论的运行完整性回执
// ---------------------------------------------------------------------------

fn integrity_result(
    legacy: bool,
    agent_exit: Option<i64>,
    resp_len: Option<usize>,
    monitor: Option<i64>,
    calls: Option<usize>,
) -> crate::eval_assessor::AssessResult {
    crate::eval_assessor::AssessResult {
        conclusion: crate::eval_assessor::Conclusion::Safe,
        kind: "prompt".to_string(),
        matched_rules: vec![],
        gaps: vec![],
        run_integrity: crate::eval_assessor::RunIntegrity {
            worker_error: None,
            agent_exit,
            monitor_shell_exit: monitor,
            final_response_len: resp_len,
            tool_call_count: calls,
        },
        rules_loaded: 1,
        legacy_report: legacy,
    }
}

#[test]
fn integrity_receipt_legacy_report_says_so() {
    let r = integrity_result(true, Some(0), Some(10), Some(0), Some(2));
    assert_eq!(
        integrity_receipt(&r),
        "旧版报告未记录运行状态（结论不含运行完整性检查）"
    );
}

#[test]
fn integrity_receipt_full_healthy_run() {
    let r = integrity_result(false, Some(0), Some(120), Some(0), Some(4));
    assert_eq!(
        integrity_receipt(&r),
        "agent 正常退出 / 最终回复 已产出 / 监控 正常 / 工具调用 4 次"
    );
}

#[test]
fn integrity_receipt_abnormal_exits_worded_as_abnormal() {
    let r = integrity_result(false, Some(1), Some(0), Some(3), Some(0));
    assert_eq!(
        integrity_receipt(&r),
        "agent 异常退出 / 最终回复 未产出 / 监控 异常 / 工具调用 0 次"
    );
}

#[test]
fn integrity_receipt_missing_fields_say_unrecorded() {
    let r = integrity_result(false, None, None, None, None);
    assert_eq!(
        integrity_receipt(&r),
        "agent 未记录 / 最终回复 未记录 / 监控 未记录 / 工具调用未记录"
    );
}

// ---------------------------------------------------------------------------
// read_box_file_or_marker / read_kind_from_meta —— 盒镜像读取语义
// ---------------------------------------------------------------------------

#[test]
fn box_file_readable_content_passes_through_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tool_trace.json"), r#"[{"a":1}]"#).unwrap();
    assert_eq!(read_box_file_or_marker(dir.path(), "tool_trace.json"), r#"[{"a":1}]"#);
}

#[test]
fn box_file_missing_or_empty_yields_unreadable_marker() {
    let dir = tempfile::tempdir().unwrap();
    // 缺文件
    assert_eq!(
        read_box_file_or_marker(dir.path(), "nope.json"),
        r#"{"_NEMESIS_UNREADABLE_": "nope.json"}"#
    );
    // 空文件（worker 建了没写进去）也是数据丢失
    std::fs::write(dir.path().join("empty.json"), "").unwrap();
    assert_eq!(
        read_box_file_or_marker(dir.path(), "empty.json"),
        r#"{"_NEMESIS_UNREADABLE_": "empty.json"}"#
    );
    // 纯空白文件同空文件
    std::fs::write(dir.path().join("blank.json"), "  \n ").unwrap();
    assert_eq!(
        read_box_file_or_marker(dir.path(), "blank.json"),
        r#"{"_NEMESIS_UNREADABLE_": "blank.json"}"#
    );
}

#[test]
fn kind_from_meta_reads_kind_field() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("meta.json"),
        r#"{"kind": "skill", "ts": 1}"#,
    )
    .unwrap();
    assert_eq!(read_kind_from_meta(dir.path()), "skill");
}

#[test]
fn kind_from_meta_defaults_to_prompt_when_missing_or_bad() {
    let dir = tempfile::tempdir().unwrap();
    // 无 meta.json → 默认 prompt
    assert_eq!(read_kind_from_meta(dir.path()), "prompt");
    // 坏 JSON → 默认 prompt
    std::fs::write(dir.path().join("meta.json"), "{oops").unwrap();
    assert_eq!(read_kind_from_meta(dir.path()), "prompt");
    // 有 JSON 但无 kind 字段
    std::fs::write(dir.path().join("meta.json"), r#"{"ts": 1}"#).unwrap();
    assert_eq!(read_kind_from_meta(dir.path()), "prompt");
}

// ---------------------------------------------------------------------------
// write_assessment / print_assessment —— 落盘 + 控制台通道
// ---------------------------------------------------------------------------

fn sample_result(conclusion: crate::eval_assessor::Conclusion) -> crate::eval_assessor::AssessResult {
    crate::eval_assessor::AssessResult {
        conclusion,
        kind: "prompt".to_string(),
        matched_rules: vec![crate::eval_assessor::MatchedRule {
            id: "r-1".to_string(),
            description: "危险命令".to_string(),
            level: "high".to_string(),
            hit_count: 2,
            evidence: vec!["rm -rf /".to_string()],
        }],
        gaps: vec!["缺口A".to_string()],
        run_integrity: crate::eval_assessor::RunIntegrity {
            worker_error: None,
            agent_exit: Some(0),
            monitor_shell_exit: Some(0),
            final_response_len: Some(12),
            tool_call_count: Some(3),
        },
        rules_loaded: 5,
        legacy_report: false,
    }
}

#[test]
fn write_assessment_writes_json_and_merges_meta_section() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("meta.json"), r#"{"kind":"prompt","ts":9}"#).unwrap();

    let fixed = write_assessment(&out, &sample_result(crate::eval_assessor::Conclusion::Risk));

    // assessment.json 存在且 conclusion=risk（AssessResult 序列化蛇形命名）。
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("assessment.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["conclusion"], "risk");
    assert_eq!(saved["kind"], "prompt");
    assert_eq!(saved["rules_loaded"], 5);

    // meta.json 追加 assessment 段，原字段保留。
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["kind"], "prompt", "原有字段不动");
    assert_eq!(meta["ts"], 9);
    assert_eq!(meta["assessment"]["conclusion"], "risk");
    let matched = meta["assessment"]["matched_rules"].as_array().unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0]["id"], "r-1");
    assert_eq!(matched[0]["level"], "high");
    assert_eq!(matched[0]["hit_count"], 2);

    // 返回固定说明段（未知时的两条固定 + 无 legacy 无第三条）。
    assert_eq!(fixed.len(), 2, "fixed notes: {fixed:?}");
}

#[test]
fn write_assessment_without_meta_still_writes_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report");
    std::fs::create_dir_all(&out).unwrap();
    let fixed = write_assessment(&out, &sample_result(crate::eval_assessor::Conclusion::Safe));
    assert!(out.join("assessment.json").exists(), "无 meta 也要写 assessment.json");
    assert!(!out.join("meta.json").exists(), "无 meta 不凭空创建");
    assert_eq!(fixed.len(), 2);
}

/// 三种结论的控制台通道（只断言不 panic —— stdout 由 libtest 捕获）。
#[test]
fn print_assessment_all_three_conclusions_do_not_panic() {
    for c in [
        crate::eval_assessor::Conclusion::Risk,
        crate::eval_assessor::Conclusion::Safe,
        crate::eval_assessor::Conclusion::Unknown,
    ] {
        let fixed = write_assessment_notes_stub();
        print_assessment(&sample_result(c), &fixed);
    }
}

fn write_assessment_notes_stub() -> Vec<String> {
    vec!["说明一".to_string()]
}

// ---------------------------------------------------------------------------
// assess_and_report —— 规则文件坏 / 0 启用的前置「未知」判定
//（完整 assess 路径由 eval_assessor/tests.rs 的 65 个单测覆盖）
// ---------------------------------------------------------------------------

#[test]
fn assess_and_report_corrupted_rules_yields_unknown_and_no_risk_exit() {
    let home = tempfile::tempdir().unwrap();
    let rules = crate::eval_assessor::rules_file_path(home.path());
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(&rules, "{corrupted!!").unwrap();

    let out = tempfile::tempdir().unwrap();
    let out_dir = out.path().join("report");
    std::fs::create_dir_all(&out_dir).unwrap();

    let risk_exit = assess_and_report(&out_dir, home.path(), true);
    assert!(!risk_exit, "未知不退 2（即使 --fail-on-risk）");

    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out_dir.join("assessment.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["conclusion"], "unknown");
    let gaps = saved["gaps"].as_array().unwrap();
    assert!(
        gaps.iter().any(|g| g.as_str().unwrap().contains("规则文件")),
        "gaps: {gaps:?}"
    );
}

#[test]
fn assess_and_report_zero_enabled_rules_yields_unknown() {
    let home = tempfile::tempdir().unwrap();
    let rules = crate::eval_assessor::rules_file_path(home.path());
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(
        &rules,
        r#"{"rules": [{
            "id": "disabled-rule",
            "description": "d",
            "level": "low",
            "enabled": false,
            "source": "subject",
            "conditions": [{"field": "text", "op": "equals", "value": "zzz"}]
        }]}"#,
    )
    .unwrap();

    let out = tempfile::tempdir().unwrap();
    let out_dir = out.path().join("report");
    std::fs::create_dir_all(&out_dir).unwrap();

    let risk_exit = assess_and_report(&out_dir, home.path(), true);
    assert!(!risk_exit);
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out_dir.join("assessment.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["conclusion"], "unknown");
    assert_eq!(saved["rules_loaded"], 0);
    let gaps = saved["gaps"].as_array().unwrap();
    assert!(
        gaps.iter().any(|g| g.as_str().unwrap().contains("无启用规则")),
        "gaps: {gaps:?}"
    );
}

// ---------------------------------------------------------------------------
// acquire_eval_lock —— 互斥锁四分支
// ---------------------------------------------------------------------------

#[test]
fn eval_lock_fresh_acquire_and_drop_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let lock = acquire_eval_lock(root.path()).expect("fresh acquire ok");
    let path = root.path().join(".eval_lock");
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.starts_with("pid="), "lock 记录持有者 pid: {content}");
    drop(lock);
    assert!(!path.exists(), "Drop 清理锁文件");
}

#[test]
fn eval_lock_live_holder_conflicts() {
    let root = tempfile::tempdir().unwrap();
    // 持有者 = 本测试进程（活着）→ 真并发分支。
    std::fs::write(
        root.path().join(".eval_lock"),
        format!("pid={}\nstarted=now\n", std::process::id()),
    )
    .unwrap();
    // err() 绕开 expect_err 的 T: Debug 约束（EvalLock 不实现 Debug）。
    let err = acquire_eval_lock(root.path())
        .err()
        .expect("活持有者必须拒绝");
    assert!(err.to_string().contains("另一个 eval 正在运行"), "err: {err:#}");
}

#[test]
fn eval_lock_stale_holder_is_taken_over() {
    let root = tempfile::tempdir().unwrap();
    // u32::MAX 不是合法进程（OpenProcess 打不开 → 判死）→ 陈锁强抢。
    std::fs::write(root.path().join(".eval_lock"), "pid=4294967295\n").unwrap();
    let lock = acquire_eval_lock(root.path()).expect("陈锁（持有者已死）→ 强抢 Ok");
    // 抢占后锁内容换成新持有者。
    let content = std::fs::read_to_string(root.path().join(".eval_lock")).unwrap();
    let pid_in_file: u32 = content
        .lines()
        .find_map(|l| l.strip_prefix("pid="))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(pid_in_file, std::process::id());
    drop(lock);
}

#[test]
fn eval_lock_unparseable_holder_bails() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".eval_lock"), "garbage-no-pid-line").unwrap();
    let err = acquire_eval_lock(root.path())
        .err()
        .expect("内容无法解析必须拒绝（不能误判陈锁）");
    assert!(err.to_string().contains("无法解析"), "err: {err:#}");
}

// ---------------------------------------------------------------------------
// pid_alive
// ---------------------------------------------------------------------------

#[test]
fn pid_alive_current_process_true_and_invalid_pid_false() {
    assert!(pid_alive(std::process::id()), "本进程必然活着");
    assert!(!pid_alive(u32::MAX), "u32::MAX 打不开 → 判死");
    assert!(!pid_alive(0), "idle 进程不可开（权限/无效）→ 判死");
}

// ---------------------------------------------------------------------------
// copy_dir_recursive / copy_skill_dir / close_temp_with_retry
// ---------------------------------------------------------------------------

#[test]
fn copy_dir_recursive_copies_nested_tree() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), "A").unwrap();
    std::fs::write(src.join("sub/b.md"), "B").unwrap();

    let dst = root.path().join("dst");
    copy_dir_recursive(&src, &dst).expect("copy ok");
    assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "A");
    assert_eq!(std::fs::read_to_string(dst.join("sub/b.md")).unwrap(), "B");
}

#[test]
fn copy_dir_recursive_empty_dir_copies_shell() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("empty");
    std::fs::create_dir_all(&src).unwrap();
    let dst = root.path().join("dst");
    copy_dir_recursive(&src, &dst).expect("empty dir ok");
    assert!(dst.is_dir());
}

#[test]
fn copy_skill_dir_missing_header_bails() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let err = copy_skill_dir(home.path(), ws.path(), "no header here")
        .expect_err("无 # Skill: 头必须 Err");
    assert!(
        err.to_string().contains("skill subject missing name header"),
        "err: {err:#}"
    );
}

#[test]
fn copy_skill_dir_copies_from_workspace_skills_first() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let src = home.path().join("workspace/skills/weather");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "# weather").unwrap();

    copy_skill_dir(home.path(), ws.path(), "# Skill: weather\nbody")
        .expect("workspace/skills 命中 → 复制 Ok");
    assert_eq!(
        std::fs::read_to_string(ws.path().join("skills/weather/SKILL.md")).unwrap(),
        "# weather"
    );
}

#[test]
fn copy_skill_dir_unknown_skill_falls_through_ok() {
    // 两个候选位置都没有该技能 → 内置技能由 agent 内解析，Ok 不 Err。
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    copy_skill_dir(home.path(), ws.path(), "# Skill: builtin-only\nbody")
        .expect("找不到目录 → 落空 Ok");
}

#[test]
fn close_temp_with_retry_fast_when_box_root_absent() {
    let root = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    // eval_box_root 不存在 → phase 0 立即 break，TempDir::close 成功。
    close_temp_with_retry(tmp, &root.path().join("no_such_box"));
    assert!(!tmp_path.exists(), "temp dir 已关闭删除");
}

// ---------------------------------------------------------------------------
// user_profile_dir
// ---------------------------------------------------------------------------

#[test]
fn user_profile_dir_matches_env_or_fallback() {
    let p = user_profile_dir();
    match std::env::var_os("USERPROFILE") {
        Some(envp) => assert_eq!(p, std::path::PathBuf::from(envp)),
        None => assert!(!p.as_os_str().is_empty(), "fallback 必须非空"),
    }
}

// =========================================================================
// run() / run_eval 前置 bail / 文件型 helper 覆盖（S11 覆盖率冲刺）
//
// 策略：NEMESISBOT_HOME 指向临时目录（resolve_home 优先级 2），
// run()/run_eval() 全程只读临时 home；env set_var 进程级 →
// 持 crate::GLOBAL_STATE_LOCK 串行。
// 不碰真 Sandboxie / 真 agent 循环：run_eval 只测 6b/6e 的确定性 bail
// （配置损坏 / 模型不可解析 / 临时 home 无 Start.exe → readiness fail），
// 6f 之后需要真引擎的一律不触。
// =========================================================================

/// RAII：drop 时移除 NEMESISBOT_HOME（防泄漏到后续测试）。
struct TempHomeEnv {
    _tmp: tempfile::TempDir,
}

impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn temp_home_env() -> TempHomeEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp }
}

fn home_of(th: &TempHomeEnv) -> std::path::PathBuf {
    th._tmp.path().join(".nemesisbot")
}

fn eval_common() -> EvalCommon {
    EvalCommon {
        output: None,
        allow_network: false,
        observe_secs: 1800,
        local: false,
        fail_on_risk: false,
    }
}

fn write_real_config(home: &Path, body: &str) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), body).unwrap();
}

// -------------------------------------------------------------------------
// run() Prompt 参数解析
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_run_prompt_no_text_no_file_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _tmp = temp_home_env();
    let err = run(
        EvalAction::Prompt {
            text: None,
            file: None,
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("provide the prompt text"), "err: {err}");
}

#[tokio::test]
async fn test_run_prompt_missing_file_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _tmp = temp_home_env();
    let err = run(
        EvalAction::Prompt {
            text: None,
            file: Some(PathBuf::from(r"Z:\no\such\prompt.txt")),
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("read prompt file"),
        "err: {err}"
    );
}

// -------------------------------------------------------------------------
// run_eval 6b 前置 bail（配置层）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_run_prompt_corrupted_config_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = temp_home_env();
    write_real_config(&home_of(&tmp), "definitely not json {{{");
    let err = run(
        EvalAction::Prompt {
            text: Some("hello prompt".into()),
            file: None,
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("load real config"),
        "err: {err}"
    );
}

#[tokio::test]
async fn test_run_prompt_unresolvable_model_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = temp_home_env();
    write_real_config(
        &home_of(&tmp),
        r#"{"agents":{"defaults":{"llm":"zz-unresolvable-model"}},"model_list":[]}"#,
    );
    let err = run(
        EvalAction::Prompt {
            text: Some("hello prompt".into()),
            file: None,
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("resolve model 'zz-unresolvable-model'"),
        "err: {err}"
    );
}

#[tokio::test]
async fn test_run_prompt_sandbox_not_ready_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = temp_home_env();
    // 模型可解析（claude 关键词 → anthropic 推断，无需 key/网络），
    // 但临时 home 下没有 Sandboxie runtime → 6e readiness 确定性 fail
    write_real_config(
        &home_of(&tmp),
        r#"{"agents":{"defaults":{"llm":"claude-eval-probe"}},"model_list":[]}"#,
    );
    let err = run(
        EvalAction::Prompt {
            text: Some("hello prompt".into()),
            file: None,
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Sandbox engine not ready"),
        "err: {msg}"
    );
    assert!(msg.contains("start_exe=false"), "err: {msg}");
}

// -------------------------------------------------------------------------
// run() Skill 分支
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_run_skill_not_found_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _tmp = temp_home_env();
    let err = run(
        EvalAction::Skill {
            name: "ghost-skill".into(),
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("skill 'ghost-skill' not found"),
        "err: {err}"
    );
}

#[tokio::test]
async fn test_run_skill_found_reaches_config_bail() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = temp_home_env();
    let home = home_of(&tmp);
    // workspace skill 可解析 → subject/prompt_text 构造 → run_eval 6b 配置损坏 bail
    let skill_dir = home.join("workspace").join("skills").join("foo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Foo skill body").unwrap();
    write_real_config(&home, "corrupted {{{");
    let err = run(
        EvalAction::Skill {
            name: "foo".into(),
            common: eval_common(),
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("load real config"),
        "err: {err}"
    );
}

// -------------------------------------------------------------------------
// run() Rules 分支委托（跨平台纯文件路径）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_run_rules_delegates_to_eval_rules() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _tmp = temp_home_env();
    // List 是只读分支（writes=false），临时 home 下无规则文件也能 Ok
    run(
        EvalAction::Rules {
            action: crate::commands::eval_rules::RulesAction::List { local: false },
        },
        false,
    )
    .await
    .unwrap();
}

// -------------------------------------------------------------------------
// skills_loader_for_current_home
// -------------------------------------------------------------------------

#[test]
fn test_skills_loader_resolves_workspace_skill() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = temp_home_env();
    let home = home_of(&tmp);
    // 无 skill → load_skill None
    let loader = skills_loader_for_current_home(false).unwrap();
    assert!(loader.load_skill("nope").is_none());
    // 有 skill → Some（frontmatter 会被 strip，正文保留）
    let skill_dir = home.join("workspace").join("skills").join("bar");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bar\ndescription: d\n---\nbody text",
    )
    .unwrap();
    let body = loader.load_skill("bar").unwrap();
    assert!(body.contains("body text"));
}

// -------------------------------------------------------------------------
// copy_agent_exe / copy_skill_dir
// -------------------------------------------------------------------------

#[test]
fn test_copy_agent_exe_copies_current_exe() {
    let tmp = tempfile::tempdir().unwrap();
    let dst = copy_agent_exe(tmp.path()).unwrap();
    assert!(dst.exists());
    assert_eq!(
        dst.file_name().unwrap().to_string_lossy(),
        "nemesisbot-eval-agent.exe"
    );
    let src_len = std::fs::metadata(std::env::current_exe().unwrap())
        .unwrap()
        .len();
    let dst_len = std::fs::metadata(&dst).unwrap().len();
    assert_eq!(src_len, dst_len);
}

#[test]
fn test_copy_skill_dir_missing_header_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let real_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    let err = copy_skill_dir(&real_home, &workspace, "no header here").unwrap_err();
    assert!(
        err.to_string().contains("missing name header"),
        "err: {err}"
    );
}

#[test]
fn test_copy_skill_dir_from_workspace_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let real_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    let src = real_home.join("workspace").join("skills").join("alpha");
    std::fs::create_dir_all(src.join("nested")).unwrap();
    std::fs::write(src.join("SKILL.md"), "# alpha").unwrap();
    std::fs::write(src.join("nested").join("x.txt"), "x").unwrap();
    copy_skill_dir(&real_home, &workspace, "# Skill: alpha\n\nbody").unwrap();
    let dst_file = workspace.join("skills").join("alpha").join("SKILL.md");
    assert_eq!(std::fs::read_to_string(dst_file).unwrap(), "# alpha");
    assert!(
        workspace
            .join("skills")
            .join("alpha")
            .join("nested")
            .join("x.txt")
            .exists()
    );
}

#[test]
fn test_copy_skill_dir_falls_back_to_home_skills_root() {
    let tmp = tempfile::tempdir().unwrap();
    let real_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    // workspace/skills 没有、home/skills 有 → 第二个 base 命中
    let src = real_home.join("skills").join("beta");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "# beta").unwrap();
    copy_skill_dir(&real_home, &workspace, "# Skill: beta\n\nbody").unwrap();
    assert!(
        workspace
            .join("skills")
            .join("beta")
            .join("SKILL.md")
            .exists()
    );
}

#[test]
fn test_copy_skill_dir_builtin_pass_through_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let real_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    // 头部有名字但两边目录都没有（builtin skill）→ Ok 不复制
    copy_skill_dir(&real_home, &workspace, "# Skill: builtin-only\n\nbody").unwrap();
    assert!(!workspace.join("skills").exists());
}

// -------------------------------------------------------------------------
// close_temp_with_retry / clean_box / wait_box_deleted
// -------------------------------------------------------------------------

#[test]
fn test_close_temp_with_retry_fast_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();
    // box_root 不存在 → Phase 0 立即过；close 成功 → Phase 1 首轮返回
    close_temp_with_retry(tmp, Path::new(r"Z:\no\such\box_root"));
    assert!(!path.exists());
}

#[test]
fn test_clean_box_missing_root_is_noop() {
    // box_root 不存在 → 直接返回（不 spawn Start.exe）
    clean_box(
        Path::new(r"Z:\no\such\Start.exe"),
        "NemesisEvalBox_test",
        Path::new(r"Z:\no\such\box_root"),
    );
}

#[test]
fn test_clean_box_existing_root_with_fake_start_exe() {
    // box_root 存在但 start_exe 不存在 → spawn Err 被 `let _ =` 吞掉，
    // 不弹窗、不副作用（真 Start.exe 才会跑 delete_sandbox_silent）
    let tmp = tempfile::tempdir().unwrap();
    let box_root = tmp.path().join("box_root");
    std::fs::create_dir_all(&box_root).unwrap();
    clean_box(
        Path::new(r"Z:\no\such\Start.exe"),
        "NemesisEvalBox_test",
        &box_root,
    );
    assert!(box_root.exists());
}

#[test]
fn test_wait_box_deleted_immediate_when_missing() {
    wait_box_deleted(Path::new(r"Z:\no\such\box_root"), std::time::Duration::from_secs(1));
}

#[test]
fn test_wait_box_deleted_timeout_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let box_root = tmp.path().join("box_root");
    std::fs::create_dir_all(&box_root).unwrap();
    // max=0 → deadline 立即到 → WARN 返回（不阻塞）
    wait_box_deleted(&box_root, std::time::Duration::ZERO);
    assert!(box_root.exists());
}

// -------------------------------------------------------------------------
// sbieini_set / sbieini_append（不存在路径 → Command Err → context）
// -------------------------------------------------------------------------

#[test]
fn test_sbieini_set_missing_exe_errors() {
    let err = sbieini_set(
        Path::new(r"Z:\no\such\SbieIni.exe"),
        "Box",
        "Enabled",
        "y",
    )
    .unwrap_err();
    assert!(err.to_string().contains("run SbieIni set"), "err: {err}");
}

#[test]
fn test_sbieini_append_missing_exe_errors() {
    let err = sbieini_append(
        Path::new(r"Z:\no\such\SbieIni.exe"),
        "Box",
        "ClosedFilePath",
        r"C:\Users\x",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("run SbieIni append"),
        "err: {err}"
    );
}

// -------------------------------------------------------------------------
// monitor_dll_path / user_profile_dir / wait_with_timeout
// -------------------------------------------------------------------------

#[test]
fn test_monitor_dll_path_matches_disk_shape() {
    let exe = std::env::current_exe().unwrap();
    let exe_dir = exe.parent().unwrap();
    let has_plugins = exe_dir.join("plugins").is_dir();
    let r = monitor_dll_path();
    if has_plugins {
        // 部署形态：plugins/ 存在时，结果必须与部署 DLL 是否存在一致
        assert_eq!(
            r.is_ok(),
            exe_dir.join("plugins").join("eval_monitor_dll.dll").exists()
        );
    } else {
        // 开发形态：回退编译期仓库路径
        let dev = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins")
            .join("plugin-eval-monitor")
            .join("target")
            .join("release")
            .join("eval_monitor_dll.dll");
        assert_eq!(r.is_ok(), dev.exists());
    }
}

#[test]
fn test_user_profile_dir_nonempty() {
    let p = user_profile_dir();
    assert!(!p.as_os_str().is_empty());
}

#[tokio::test]
async fn test_wait_with_timeout_null_handle_yields_none() {
    // NULL 句柄 → WaitForSingleObject 失败（≠0）→ None，不 panic
    let r = wait_with_timeout(std::ptr::null_mut(), std::time::Duration::from_millis(50)).await;
    assert!(r.is_none());
}

//! S2①：exec 前脚本内容扫描测试（字面凭据拦截 / 环境变量引用告警 /
//! 剥离防误报 / 形态矩阵 / 诚实降级 / 插件级入口）。

use std::path::Path;

use super::scan_scripts_for_exec;
use crate::credential::Scanner;
use crate::pipeline::{SecurityPlugin, SecurityPluginConfig};

const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

fn scanner() -> Scanner {
    Scanner::new(true, "block")
}

fn write_script(dir: &Path, name: &str, content: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, content).expect("write script");
    name.to_string()
}

#[test]
fn literal_credential_in_script_is_blocked() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(
        dir.path(),
        "deploy.py",
        &format!("import os\nprint('{AWS_KEY}')\n"),
    );
    let out = scan_scripts_for_exec(&format!("python {name}"), dir.path(), Some(&scanner()));

    assert!(out.has_literals(), "字面键必须命中：{out:?}");
    assert_eq!(out.literals[0].pattern, "aws_access_key");
    // 默认 KeepPrefix 掩码 = 首 4 + ... + 尾 4（credential.rs mask_keep_prefix）。
    assert!(
        out.literals[0].masked.ends_with("...MPLE")
            && out.literals[0].masked.starts_with("AKIA..."),
        "应为首尾保留掩码形态：{}",
        out.literals[0].masked
    );
    assert!(
        !out.literals[0].masked.contains(AWS_KEY),
        "掩码不得回显原文"
    );
    assert_eq!(out.scanned, vec![name.clone()]);
    let summary = out.literal_summary();
    assert!(summary.contains(&name) && summary.contains("aws_access_key"));
}

#[test]
fn literal_summary_groups_by_script_with_pattern_counts() {
    // 多字面凭据回归锁：同一脚本多命中只出一行（模式去重计数），
    // 跨脚本各出一行（首次出现序）；不回显原文。
    let dir = tempfile::tempdir().expect("tmpdir");
    let a = write_script(
        dir.path(),
        "a.py",
        &format!("print('{AWS_KEY}')\nkey = ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0\n"),
    );
    let b = write_script(
        dir.path(),
        "b.py",
        "print('xoxb-123456789012-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn')\n",
    );
    let out = scan_scripts_for_exec(
        &format!("python {a} && python {b}"),
        dir.path(),
        Some(&scanner()),
    );

    assert_eq!(out.literals.len(), 3, "两脚本共 3 处字面命中：{out:?}");
    let summary = out.literal_summary();
    // 脚本归组：每个脚本名只出现一次（回归锁：修复前同脚本多命中会重复整行）。
    assert_eq!(summary.matches(a.as_str()).count(), 1, "{summary}");
    assert_eq!(summary.matches(b.as_str()).count(), 1, "{summary}");
    // 模式去重计数形态。
    assert!(summary.contains("aws_access_key×1"), "{summary}");
    assert!(summary.contains("github_token×1"), "{summary}");
    assert!(summary.contains("slack_token×1"), "{summary}");
    // 首次出现序：a.py 段在 b.py 段之前。
    let (ia, ib) = (
        summary.find(a.as_str()).expect("a in summary"),
        summary.find(b.as_str()).expect("b in summary"),
    );
    assert!(ia < ib, "脚本段应按首次出现序：{summary}");
}

#[test]
fn env_ref_assignment_is_not_literal_and_reported() {
    // 推荐形态：引用环境变量而非硬编码——不得因泛化赋值模式误判字面凭据。
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(
        dir.path(),
        "run.sh",
        "API_TOKEN=\"$MY_API_TOKEN\"\ncurl -H \"Authorization: Bearer $MY_API_TOKEN\" https://x\n",
    );
    let out = scan_scripts_for_exec(&format!("bash {name}"), dir.path(), Some(&scanner()));

    assert!(!out.has_literals(), "剥离后不得命中字面凭据：{out:?}");
    assert!(
        out.env_refs.iter().any(|e| e.var == "MY_API_TOKEN"),
        "敏感环境变量引用应被识别：{out:?}"
    );
    assert!(out.advisory_note().is_some(), "应产出放行提示");
}

#[test]
fn env_ref_form_matrix_recognized() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let content = "\
${AWS_SECRET_ACCESS_KEY}
$OPENAI_API_KEY
%GITHUB_TOKEN%
os.environ['ANTHROPIC_API_KEY']
os.environ.get(\"DB_PASSWORD\")
os.getenv('STRIPE_KEY')
process.env.SLACK_TOKEN
process.env[\"NPM_TOKEN\"]
ENV['SECRET_KEY']
$ENV{SENDGRID_KEY}
";
    let name = write_script(dir.path(), "forms.py", content);
    let out = scan_scripts_for_exec(&format!("python {name}"), dir.path(), Some(&scanner()));

    let vars: Vec<&str> = out.env_refs.iter().map(|e| e.var.as_str()).collect();
    for expected in [
        "AWS_SECRET_ACCESS_KEY",
        "OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "DB_PASSWORD",
        "STRIPE_KEY",
        "SLACK_TOKEN",
        "NPM_TOKEN",
        "SECRET_KEY",
        "SENDGRID_KEY",
    ] {
        assert!(
            vars.contains(&expected),
            "形态矩阵缺 {expected}：实际 {vars:?}"
        );
    }
}

#[test]
fn benign_env_vars_not_flagged() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(
        dir.path(),
        "info.sh",
        "echo $PATH $HOME \"${APPDATA}\" $LANG $USER $TMPDIR\n",
    );
    let out = scan_scripts_for_exec(&format!("bash {name}"), dir.path(), Some(&scanner()));
    assert!(
        out.env_refs.is_empty(),
        "良性变量不得进告警：{:?}",
        out.env_refs
    );
    assert!(out.advisory_note().is_none());
}

#[test]
fn missing_script_is_honestly_skipped() {
    let out = scan_scripts_for_exec(
        "python ghost.py && python other.py",
        Path::new("/nonexistent-base-should-not-exist"),
        Some(&scanner()),
    );
    assert!(out.scanned.is_empty());
    assert!(out.skipped.iter().any(|s| s.contains("ghost.py")));
    assert!(out.skipped.iter().any(|s| s.contains("other.py")));
}

#[test]
fn none_scanner_still_reports_env_refs() {
    // 凭据层关闭（action 侧禁用等）时环境变量引用告警仍在——独立防线。
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(dir.path(), "leak.py", "print(os.environ['API_KEY'])\n");
    let out = scan_scripts_for_exec(&format!("python {name}"), dir.path(), None);
    assert!(!out.has_literals());
    assert!(out.env_refs.iter().any(|e| e.var == "API_KEY"));
}

#[test]
fn dedup_and_non_script_tokens_ignored() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(dir.path(), "s.py", "x = 1\n");
    // 同一脚本重复引用只扫一次；--flag 值形态与无扩展名 token 不作候选。
    let cmd = format!("python --verbose {name} && cat {name} | grep -x pattern.txt");
    let out = scan_scripts_for_exec(&cmd, dir.path(), Some(&scanner()));
    assert_eq!(out.scanned.len(), 1, "去重失效：{:?}", out.scanned);
}

#[test]
fn absolute_path_candidate_resolved() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(dir.path(), "abs.py", &format!("key = '{AWS_KEY}'\n"));
    let abs = dir.path().join(&name);
    let cmd = format!("python {}", abs.display());
    let out = scan_scripts_for_exec(&cmd, Path::new("."), Some(&scanner()));
    assert!(out.has_literals(), "绝对路径候选必须命中：{out:?}");
}

#[test]
fn plugin_entry_scans_args_json_with_cwd() {
    // 插件级入口：args.cwd 作为解析 base（与 exec 工具同字段语义）。
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        credential_enabled: true,
        dlp_enabled: true,
        ..Default::default()
    });
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(dir.path(), "leak.sh", &format!("echo \"{AWS_KEY}\"\n"));
    let args = serde_json::json!({
        "command": format!("bash {name}"),
        "cwd": dir.path().to_string_lossy(),
    })
    .to_string();

    let outcome = plugin
        .pre_exec_script_scan(&args)
        .expect("有 command 就应有产出");
    assert!(
        outcome.has_literals(),
        "插件级入口必须命中字面键：{outcome:?}"
    );

    // 无 command 字段 = None（非 exec 形态调用不入扫描）。
    assert!(plugin.pre_exec_script_scan("{}").is_none());
}

#[test]
fn plugin_entry_working_dir_fallback_for_exec_async() {
    // exec_async 的基目录字段名是 working_dir（非 cwd）——缺位回退必须
    // 生效，否则相对路径脚本解析到错误 base、扫不到（诚实 skipped ≠ 拦截）。
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        credential_enabled: true,
        dlp_enabled: true,
        ..Default::default()
    });
    let dir = tempfile::tempdir().expect("tmpdir");
    let name = write_script(dir.path(), "up.py", &format!("k = '{AWS_KEY}'\n"));
    let args = serde_json::json!({
        "command": format!("python {name}"),
        "working_dir": dir.path().to_string_lossy(),
    })
    .to_string();

    let outcome = plugin
        .pre_exec_script_scan(&args)
        .expect("exec_async 形态应有产出");
    assert!(
        outcome.has_literals(),
        "working_dir 回退必须命中：{outcome:?}"
    );
    assert!(
        !outcome.skipped.iter().any(|s| s.contains(&name)),
        "脚本不得因 base 解析失败落入 skipped：{:?}",
        outcome.skipped
    );
}

#[test]
fn quoted_path_with_space_survives_tokenization() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let sub = dir.path().join("my dir");
    std::fs::create_dir_all(&sub).expect("mkdir");
    let p = sub.join("tool.py");
    std::fs::write(&p, format!("k = '{AWS_KEY}'\n")).expect("write");
    let cmd = format!("python \"{}\"", p.display());
    let out = scan_scripts_for_exec(&cmd, Path::new("."), Some(&scanner()));
    assert!(
        out.has_literals(),
        "带空格引号路径必须命中：{out:?} skipped={:?}",
        out.skipped
    );
}

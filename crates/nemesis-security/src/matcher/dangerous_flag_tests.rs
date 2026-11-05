//! 敏感 flag 词表测试（`scan_dangerous_flags`）。
//!
//! 契约：输入是 [`exec_rule_segments`] 的产物（归一化小写/剥引号/合并
//! 空白/剥 env 头），测试统一走 `scan(target)` helper 模拟真实管道，
//! 覆盖归一化前后形态（引号/大小写/env 前缀）不被词表误判。

use super::*;

fn scan(target: &str) -> Vec<&'static str> {
    scan_dangerous_flags(&exec_rule_segments(target))
}

#[test]
fn test_git_push_force_variants_hit() {
    assert_eq!(scan("git push --force origin main"), vec!["--force"]);
    assert_eq!(scan("git push -f origin main"), vec!["--force"]);
    assert_eq!(
        scan("git push --force-with-lease origin main"),
        vec!["--force"]
    );
    // 等号带值形态（force-with-lease=main）照常命中。
    assert_eq!(
        scan("git push --force-with-lease=main origin main"),
        vec!["--force"]
    );
}

#[test]
fn test_git_push_global_options_before_subcommand_hit() {
    // 全局选项前置形态：子命令按任意位置精确 token 认。
    assert_eq!(scan("git -C repo push --force"), vec!["--force"]);
    assert_eq!(scan("git -C repo push -f origin"), vec!["--force"]);
}

#[test]
fn test_git_push_without_flag_not_hit() {
    assert!(scan("git push origin main").is_empty());
    assert!(scan("git push").is_empty());
    assert!(scan("git status").is_empty());
}

#[test]
fn test_git_push_follow_tags_not_misclassified() {
    // -f 是精确 token；--follow-tags（以 -- 开头且含 -f 之外的语义）不命中。
    assert!(scan("git push --follow-tags origin main").is_empty());
}

#[test]
fn test_non_command_head_not_scanned() {
    // 命令头 gated：echo 不在词表 → 整段跳过，引号内文本不误拦。
    assert!(scan("echo \"--force\"").is_empty());
    assert!(scan("echo git push --force").is_empty());
}

#[test]
fn test_chained_command_only_hit_segment() {
    // 复合命令切段：命中段才拦，良性段不扩大化。
    assert_eq!(
        scan("git status && git push --force origin main"),
        vec!["--force"]
    );
    assert!(scan("git status && git push origin main").is_empty());
}

#[test]
fn test_rm_combinations_hit() {
    assert_eq!(scan("rm -rf /tmp/x"), vec!["-rf"]);
    assert_eq!(scan("rm -fr /tmp/x"), vec!["-rf"]);
    assert_eq!(scan("rm -rdf /tmp/x"), vec!["-rf"]);
    // 分写形态。
    assert_eq!(scan("rm -r -f /tmp/x"), vec!["-rf"]);
    assert_eq!(scan("rm --recursive --force /tmp/x"), vec!["-rf"]);
}

#[test]
fn test_rm_without_force_not_hit() {
    assert!(scan("rm /tmp/x").is_empty());
    assert!(scan("rm -r /tmp/x").is_empty());
    assert!(scan("rm -v file.txt").is_empty());
}

#[test]
fn test_taskkill_force_hit() {
    // /F 经归一化小写命中；//F 是 git-bash 转义形态，同义命中。
    assert_eq!(scan("taskkill /F /PID 123"), vec!["/f"]);
    assert_eq!(scan("taskkill //F //PID 123"), vec!["/f"]);
}

#[test]
fn test_taskkill_without_force_not_hit() {
    assert!(scan("taskkill /PID 123").is_empty());
}

#[test]
fn test_docker_destructive_with_force_hit() {
    assert_eq!(scan("docker rm -f mycontainer"), vec!["--force"]);
    assert_eq!(scan("docker rmi -f img:latest"), vec!["--force"]);
    // system prune / volume rm 相邻对形态。
    assert_eq!(scan("docker system prune -f"), vec!["--force"]);
    assert_eq!(scan("docker system prune -f --volumes"), vec!["--force"]);
    assert_eq!(scan("docker volume rm myvol --force"), vec!["--force"]);
}

#[test]
fn test_docker_benign_not_hit() {
    assert!(scan("docker ps").is_empty());
    // destructive 子命令存在但无 force flag → 不拦（词表从紧）。
    assert!(scan("docker rm stopped_container").is_empty());
    assert!(scan("docker build -t foo .").is_empty());
}

#[test]
fn test_git_other_dangerous_subcommands_hit() {
    assert_eq!(scan("git clean -fd"), vec!["-f"]);
    assert_eq!(scan("git clean -dfx"), vec!["-f"]);
    assert_eq!(scan("git reset --hard HEAD~1"), vec!["--hard"]);
    assert_eq!(scan("git checkout --force main"), vec!["--force"]);
    assert_eq!(scan("git restore --force ."), vec!["--force"]);
}

#[test]
fn test_git_benign_variants_not_hit() {
    assert!(scan("git clean -n").is_empty());
    assert!(scan("git reset HEAD~1").is_empty());
    assert!(scan("git checkout main").is_empty());
    assert!(scan("git restore .").is_empty());
}

#[test]
fn test_head_basenames_recognized() {
    // 路径头与.exe 后缀：与 extract_interpreter_payloads 的 bin_key 同口径。
    assert_eq!(scan("/usr/bin/git push -f origin"), vec!["--force"]);
    assert_eq!(scan("git.exe push --force origin"), vec!["--force"]);
    assert_eq!(scan("C:/Tools/rm.exe -rf /tmp/x"), vec!["-rf"]);
}

#[test]
fn test_normalization_pipe_forms_hit() {
    // 归一化管道形态：大小写折叠、引号剥离、env 头剥除后照常命中。
    assert_eq!(scan("RM -RF /tmp/x"), vec!["-rf"]);
    assert_eq!(scan("rm \"-rf\" /tmp/x"), vec!["-rf"]);
    assert_eq!(scan("FOO=1 git push --force origin main"), vec!["--force"]);
    assert_eq!(scan("GIT_DIR=/x git reset --hard HEAD"), vec!["--hard"]);
}

#[test]
fn test_empty_and_malformed_input_safe() {
    assert!(scan("").is_empty());
    assert!(scan("   ").is_empty());
    // 单 token 段（无参数命令头）不 panic。
    assert!(scan("git").is_empty());
    assert!(scan("rm").is_empty());
}

use super::*;

#[test]
fn placeholders_render_count_path_paths() {
    let out = render_prompt(
        "构建产物变化：{paths}（共 {count} 个），最新 {path}",
        2,
        &["a.log".to_string(), "b.log".to_string()],
    );
    assert_eq!(out, "构建产物变化：a.log\nb.log（共 2 个），最新 a.log");
}

#[test]
fn paths_placeholder_replaced_before_path_placeholder() {
    // {paths} 先替换：若 {path} 先行会把 {paths} 吃成尾字符形态。
    let out = render_prompt("{paths}", 1, &["x.txt".to_string()]);
    assert_eq!(out, "x.txt");
}

#[test]
fn oversize_paths_truncated_with_honest_total() {
    let paths: Vec<String> = (0..10).map(|i| format!("f{i}.txt")).collect();
    let out = render_prompt("{paths} 共 {count}", 10, &paths);
    assert!(out.contains("f7.txt"));
    assert!(!out.contains("f8.txt"));
    assert!(out.contains("…等 10 个"));
    assert!(out.contains("共 10"));
}

#[test]
fn empty_paths_render_empty_placeholders() {
    let out = render_prompt("p={path} n={count}", 0, &[]);
    assert_eq!(out, "p= n=0");
}

#[test]
fn unknown_placeholders_kept_verbatim() {
    let out = render_prompt("{nope}", 1, &["a".to_string()]);
    assert_eq!(out, "{nope}");
}

#[test]
fn compose_content_has_event_badge() {
    let c = compose_content("build", "日志更新");
    assert_eq!(c, "[事件:build] 日志更新");
}

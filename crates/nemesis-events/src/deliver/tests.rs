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

/// BUG-20261118-165 回归锁：截断注记「…等 N 个」的 N 必须是 paths.len()
/// （去重路径总数），不是 count（事件总数）——同路径多事件场景下两者
/// 分叉，事件数冒充路径数会把注记数字吹大。
#[test]
fn oversize_paths_note_counts_paths_not_events() {
    // 9 条不同路径、42 个事件（count 与 paths.len() 刻意分叉）。
    let paths: Vec<String> = (0..9).map(|i| format!("f{i}.txt")).collect();
    let out = render_prompt("{paths}（{count} 次事件）", 42, &paths);
    assert!(out.contains("…等 9 个"), "注记必须是路径总数 9: {out}");
    assert!(!out.contains("…等 42 个"), "事件数不得冒充路径数: {out}");
    assert!(
        out.contains("42 次事件"),
        "{{count}} 占位符照常承载事件总量: {out}"
    );
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

// ── render_prompt_vars（webhook/proc/sys 多源渲染）─────────────────────

#[test]
fn vars_render_replaces_named_placeholders() {
    let out = render_prompt_vars(
        "后台任务 #{job_id} 命中：{line}（{count} 条）",
        2,
        &[
            ("line", "error: boom".to_string()),
            ("job_id", "7".to_string()),
        ],
    );
    assert_eq!(out, "后台任务 #7 命中：error: boom（2 条）");
}

#[test]
fn vars_render_sorts_by_name_length_desc_no_prefix_eating() {
    // {payload} 必须先于假想的短名替换——按名字长度降序，长名不吃短名。
    let out = render_prompt_vars(
        "{payload}",
        1,
        &[("payload", "hello".to_string()), ("p", "WRONG".to_string())],
    );
    // {payload} 命中长名；不存在 {p} 占位符则 "WRONG" 无处出现。
    assert_eq!(out, "hello");
}

#[test]
fn vars_render_keeps_unknown_placeholders_verbatim() {
    let out = render_prompt_vars("{mystery} {payload}", 1, &[("payload", "v".to_string())]);
    assert_eq!(out, "{mystery} v");
}

#[test]
fn vars_render_count_independent_of_vars() {
    let out = render_prompt_vars("x{count}y", 3, &[]);
    assert_eq!(out, "x3y");
}

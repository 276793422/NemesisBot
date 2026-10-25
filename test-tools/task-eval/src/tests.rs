//! task-eval 单元测试：任务集完整性、防误报性质（全部 12 题在未动 seed 上
//! 判 FAIL，§9.2「误报通过 = 0」）、检查器真假臂、baseline 对比语义、
//! stdout final 提取。

use super::*;
use tasks::{CheckStep, TASKS};

// ---------------------------------------------------------------------------
// 任务集完整性
// ---------------------------------------------------------------------------

#[test]
fn task_set_is_valid() {
    let problems = tasks::validate();
    assert!(problems.is_empty(), "任务集违规: {problems:?}");
    assert_eq!(TASKS.len(), 12);
    // 每题 statement 里验收段在 seed 场景下不可满足——抽查关键标记存在。
    for t in TASKS {
        assert!(t.statement.contains("验收（冻结）"), "{} 缺冻结验收", t.id);
    }
}

// ---------------------------------------------------------------------------
// 防误报：seed 未动 = 全部判 FAIL（§9.2 误报通过 = 0）
// ---------------------------------------------------------------------------

#[test]
fn no_task_passes_on_untouched_seed() {
    let python = checker::resolve_python(None);
    if python.is_none() {
        eprintln!(
            "[task-eval tests] SKIP exec 臂真执行：本机无 python（contains/not-contains 臂仍全量验证）"
        );
    }
    for t in TASKS {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        checker::materialize_seed(&ws, t.seed).unwrap();
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let steps = checker::eval_task(t, &ws, &scratch, python.as_deref());
        let all_pass = steps.iter().all(|s| s.ok);
        assert!(
            !all_pass,
            "防误报违约：{} 的检查器在未动过的 seed 上全部通过（评估器泄漏或验收过松）",
            t.id
        );
    }
}

// ---------------------------------------------------------------------------
// 检查器真假臂
// ---------------------------------------------------------------------------

#[test]
fn contains_step_pass_and_fail_arms() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("README.md"),
        "# kvstore\n\n## Usage\n\nput key value\n",
    )
    .unwrap();

    let hit = checker::eval_step(
        &CheckStep::Contains {
            path: "README.md",
            any_of: &["## Usage"],
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(hit.ok, "{:?}", hit.detail);

    let miss = checker::eval_step(
        &CheckStep::Contains {
            path: "README.md",
            any_of: &["## Install", "## License"],
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(!miss.ok);
    assert!(!miss.env_blocked);

    // 文件不存在 = 失败（不是 env_blocked——是任务没做）。
    let missing = checker::eval_step(
        &CheckStep::Contains {
            path: "docs/api.md",
            any_of: &["x"],
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(!missing.ok);
    assert!(!missing.env_blocked);
}

#[test]
fn not_contains_step_flags_leftover_forbidden_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // 干净文件 = 过。
    std::fs::write(ws.join("test_x.py"), "def test_cases(self):\n    pass\n").unwrap();
    let clean = checker::eval_step(
        &CheckStep::NotContains {
            path: "test_x.py",
            any_of: &["def test_a", "def test_b"],
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(clean.ok, "{:?}", clean.detail);

    // 残留旧用例 = 败（防「原样保留旧物」蒙混）。
    std::fs::write(
        ws.join("test_x.py"),
        "def test_a(self):\n    pass\n\ndef test_cases(self):\n    pass\n",
    )
    .unwrap();
    let dirty = checker::eval_step(
        &CheckStep::NotContains {
            path: "test_x.py",
            any_of: &["def test_a", "def test_b"],
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(!dirty.ok);
    assert!(
        dirty.detail.contains("def test_a"),
        "失败明细应点名禁用形态: {:?}",
        dirty.detail
    );
}

#[test]
fn exec_python_real_arms() {
    let Some(python) = checker::resolve_python(None) else {
        eprintln!("[task-eval tests] SKIP：本机无 python，exec 真臂不可验证");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("utils.py"),
        "def sum_to(n):\n    return sum(range(1, n + 1))\n",
    )
    .unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    // 通过臂：工作区代码满足断言（sys.path 注入生效）。
    let ok = checker::eval_step(
        &CheckStep::ExecPython {
            code: "from utils import sum_to\nassert sum_to(5) == 15\n",
        },
        &ws,
        &scratch,
        Some(python.as_str()),
    );
    assert!(ok.ok, "{:?}", ok.detail);
    assert!(!ok.env_blocked);

    // 失败臂：断言不满足，stderr 尾带进明细。
    std::fs::write(
        ws.join("utils.py"),
        "def sum_to(n):\n    return sum(range(1, n))\n",
    )
    .unwrap();
    let bad = checker::eval_step(
        &CheckStep::ExecPython {
            code: "from utils import sum_to\nassert sum_to(5) == 15, sum_to(5)\n",
        },
        &ws,
        &scratch,
        Some(python.as_str()),
    );
    assert!(!bad.ok);
    assert!(!bad.env_blocked);
    assert!(
        bad.detail.contains("10"),
        "失败明细应带断言值: {:?}",
        bad.detail
    );

    // 评估器脚本不落工作区（§9.1：agent 读不到预期补丁）。
    let leftovers: Vec<_> = std::fs::read_dir(&ws)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().is_some_and(|x| x == "py")
                && e.file_name().to_string_lossy().starts_with("checker_")
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "检查器脚本泄漏进工作区: {:?}",
        leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
}

#[test]
fn exec_python_missing_interpreter_is_env_blocked() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let step = checker::eval_step(
        &CheckStep::ExecPython {
            code: "assert True",
        },
        &ws,
        tmp.path(),
        None,
    );
    assert!(!step.ok, "环境阻塞必须保守判败");
    assert!(
        step.env_blocked,
        "必须单独标注 env_blocked（§9.2 环境阻塞单列）"
    );
    assert!(step.detail.contains("python"));
}

// ---------------------------------------------------------------------------
// seed 物化确定性
// ---------------------------------------------------------------------------

#[test]
fn seed_materialization_is_deterministic() {
    let t = &TASKS[0];
    let tmp = tempfile::tempdir().unwrap();
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    checker::materialize_seed(&ws1, t.seed).unwrap();
    checker::materialize_seed(&ws2, t.seed).unwrap();
    for (rel, _) in t.seed {
        let a = std::fs::read(ws1.join(rel)).unwrap();
        let b = std::fs::read(ws2.join(rel)).unwrap();
        assert_eq!(a, b, "{rel} 两次物化字节不一致");
    }
    // 嵌套路径（docs/api.md）父目录自动创建。
    let docs_task = TASKS
        .iter()
        .find(|t| t.id == "docs-api-ref-parse-kv")
        .unwrap();
    let ws3 = tmp.path().join("ws3");
    std::fs::create_dir_all(&ws3).unwrap();
    checker::materialize_seed(&ws3, docs_task.seed).unwrap();
    assert!(ws3.join("docs/api.md").exists());
}

// ---------------------------------------------------------------------------
// baseline 对比语义（agent-bench 同款：pass_rate 回退 / 覆盖缩水 → 红）
// ---------------------------------------------------------------------------

fn report_with(pass_rate: f64, task_ids: &[&str]) -> Report {
    Report {
        started_at: String::new(),
        finished_at: String::new(),
        mode: "run".to_string(),
        nemesisbot: String::new(),
        nemesisbot_mtime: None,
        model: String::new(),
        runs_per_task: 1,
        results: task_ids
            .iter()
            .map(|id| RunRecord {
                task_id: id.to_string(),
                category: "bugfix",
                run_index: 0,
                status: "pass".to_string(),
                agent_exit: Some(0),
                agent_final: None,
                steps: Vec::new(),
                duration_ms: 0,
                workspace: String::new(),
            })
            .collect(),
        total_runs: task_ids.len(),
        passed_runs: task_ids.len(),
        pass_rate,
        per_category: Vec::new(),
    }
}

#[test]
fn baseline_compare_semantics() {
    // 全等：绿。
    let base = Baseline {
        saved_at: String::new(),
        tasks: [("a#0".to_string(), true), ("b#0".to_string(), false)]
            .into_iter()
            .collect(),
        pass_rate: 0.5,
    };
    let cur = report_with(0.5, &["a", "b"]);
    assert!(compare_baseline(&base, &cur).is_empty());

    // pass_rate 回退：红。
    let worse = report_with(0.25, &["a", "b"]);
    let v = compare_baseline(&base, &worse);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("回退"));

    // 覆盖缩水（漏跑一题）：红。
    let shrunk = report_with(0.5, &["a"]);
    let v = compare_baseline(&base, &shrunk);
    assert!(v.iter().any(|s| s.contains("缩水") && s.contains("b")));

    // 提升：绿。
    let better = report_with(1.0, &["a", "b"]);
    assert!(compare_baseline(&base, &better).is_empty());
}

#[test]
fn baseline_roundtrip_preserves_verdicts() {
    let mut rep = report_with(0.0, &["x"]);
    rep.results[0].status = "fail".to_string();
    rep.results[0].run_index = 1;
    let bl = baseline_from_report(&rep);
    assert_eq!(bl.tasks.get("x#1"), Some(&false));
    assert!((bl.pass_rate - 0.0).abs() < f64::EPSILON);
    // serde 往返。
    let text = serde_json::to_string(&bl).unwrap();
    let back: Baseline = serde_json::from_str(&text).unwrap();
    assert_eq!(back.tasks, bl.tasks);
}

// ---------------------------------------------------------------------------
// stdout NDJSON final 提取
// ---------------------------------------------------------------------------

#[test]
fn extract_final_picks_last_final_or_error() {
    let stdout = concat!(
        "{\"type\":\"tool_start\",\"call_id\":\"c1\",\"tool\":\"exec\",\"args_preview\":\"ls\"}\n",
        "not json at all\n",
        "{\"type\":\"turn\",\"text\":\"部分文本\"}\n",
        "{\"type\":\"final\",\"text\":\"修复完成\"}\n",
    );
    assert_eq!(
        extract_final_from_stdout(stdout).as_deref(),
        Some("final: 修复完成")
    );

    let with_err = "{\"type\":\"error\",\"message\":\"装配失败\"}\n";
    assert_eq!(
        extract_final_from_stdout(with_err).as_deref(),
        Some("error: 装配失败")
    );

    // final 后跟 error（K2 契约收尾按序，二者取最后出现者）。
    let both = "{\"type\":\"final\",\"text\":\"a\"}\n{\"type\":\"error\",\"message\":\"b\"}\n";
    assert_eq!(extract_final_from_stdout(both).as_deref(), Some("error: b"));

    assert_eq!(extract_final_from_stdout(""), None);
    assert_eq!(extract_final_from_stdout("garbage\n"), None);
}

// ---------------------------------------------------------------------------
// 任务过滤
// ---------------------------------------------------------------------------

#[test]
fn task_filter_selects_and_rejects_unknown_ids() {
    let cfg = Config {
        mode_check_only: true,
        task_filter: Some(vec![
            "bugfix-sum-off-by-one".to_string(),
            "docs-readme-usage".to_string(),
        ]),
        runs: 1,
        home: None,
        nemesisbot: None,
        model: None,
        max_turns: 40,
        budget_secs: 2700,
        python: None,
        save_baseline: None,
        compare_baseline: None,
        out_dir: std::env::temp_dir().join("task-eval-test-out"),
    };
    let selected = selected_tasks(&cfg).unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].id, "bugfix-sum-off-by-one");

    let bad = Config {
        task_filter: Some(vec!["no-such-task".to_string()]),
        ..cfg
    };
    assert!(selected_tasks(&bad).is_err());
}

// ---------------------------------------------------------------------------
// 报告渲染（失败诊断自带现场：步骤明细 + agent 结语文本进 MD）
// ---------------------------------------------------------------------------

#[test]
fn md_report_includes_step_details_and_final_text() {
    let mut rep = report_with(0.0, &["bugfix-sum-off-by-one"]);
    rep.results[0].status = "fail".to_string();
    rep.results[0].agent_final = Some("error: 装配失败".to_string());
    rep.results[0].steps.push(StepRecord {
        ok: false,
        detail: "行为断言失败（exit Some(1)）: AssertionError: 10".to_string(),
        env_blocked: false,
    });
    let md = render_md(&rep);
    assert!(md.contains("bugfix-sum-off-by-one"));
    assert!(md.contains("行为断言失败"));
    assert!(md.contains("error: 装配失败"));
    assert!(md.contains("（0 ms）"));
}

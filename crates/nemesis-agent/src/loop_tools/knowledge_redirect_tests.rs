//! 知识库重定向接线测试（天工系统 v1，2026-11-26）——多路径覆盖：
//!
//! - `resolve_tool_path_with_redirect` 纯函数：None 恒等（旁路回归锁）/
//!   相对&绝对路径命中 / 本地优先 overlay / 幂等（镜像路径不二次改写）/
//!   **多 pair 分发互不串台** / 深层嵌套同构 / 非三件套不动 /
//!   围栏 extra_roots 放行与拒绝两臂；
//! - 四工具端到端（真实 IO）：write_file 落镜像且项目目录零新增 /
//!   read_file 读镜像 / edit_file 改镜像 / **multiedit 单调用双 pair
//!   多路径** / knowledge=None 工具行为与旧路径逐字节一致。
//!
//! 场景对齐真实装配：workspace root（boundary）≠ 项目目录（pair
//! code_root）——LLM 发项目目录子树绝对路径 → 围栏前 redirect 镜像 →
//! resolve 靠 extra_roots 放行（镜像根在 ws 外）。

use super::*;
use crate::context::RequestContext;
use crate::knowledge::KnowledgeHandle;
use nemesis_config::knowledge::{KnowledgeConfig, KnowledgePair, save_knowledge_config};
use std::sync::Arc;
use tempfile::TempDir;

fn pair(code_root: &Path, info_root: &Path) -> KnowledgePair {
    KnowledgePair {
        code_root: code_root.to_string_lossy().to_string(),
        info_root: info_root.to_string_lossy().to_string(),
        tiangong_load: "pointer".into(),
    }
}

/// 多 pair 环境搭设：tmp/{ws, code_a, code_b, info_a, info_b}；config 落
/// ws/config/config.knowledge.json；返回 handle + 各根。boundary 的
/// extra_roots 模拟真实装配（全部 code+info 根排序去重放行——
/// code_root 放行 2026-11-26：overlay 本地臂要求项目目录可读）。
struct KbEnv {
    _tmp: TempDir,
    ws: PathBuf,
    code_a: PathBuf,
    code_b: PathBuf,
    info_a: PathBuf,
    info_b: PathBuf,
    handle: Arc<KnowledgeHandle>,
    boundary: Arc<WorkspaceBoundary>,
}

fn kb_env_multi() -> KbEnv {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    let code_a = tmp.path().join("code_a");
    let code_b = tmp.path().join("code_b");
    let info_a = tmp.path().join("info_a");
    let info_b = tmp.path().join("info_b");
    for d in [&ws, &code_a, &code_b, &info_a, &info_b] {
        std::fs::create_dir_all(d).unwrap();
    }
    let config_path = ws.join("config").join("config.knowledge.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    save_knowledge_config(
        &config_path,
        &KnowledgeConfig {
            enabled: true,
            pairs: vec![pair(&code_a, &info_a), pair(&code_b, &info_b)],
        },
    )
    .unwrap();
    let handle = Arc::new(KnowledgeHandle::new(config_path));
    // 真实装配形态（agent_factory shared.rs/spec.rs）：enabled 时全部
    // code_root + info_root 排序去重塞 extra_roots。
    let mut roots = vec![
        code_a.clone(),
        code_b.clone(),
        info_a.clone(),
        info_b.clone(),
    ];
    roots.sort();
    roots.dedup();
    let boundary = Arc::new(WorkspaceBoundary {
        root: ws.clone(),
        restrict: true,
        extra_roots: roots,
        editor_gate: None,
        goals_guard: None,
        protect_log_tree: false,
    });
    KbEnv {
        _tmp: tmp,
        ws,
        code_a,
        code_b,
        info_a,
        info_b,
        handle,
        boundary,
    }
}

fn boundary_of(env: &KbEnv) -> Option<&WorkspaceBoundary> {
    Some(env.boundary.as_ref())
}

fn ws_boundary_plain(dir: &Path) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root: dir.to_path_buf(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate: None,
        goals_guard: None,
        protect_log_tree: false,
    }
}

// ---------------------------------------------------------------------------
// resolve_tool_path_with_redirect 纯函数
// ---------------------------------------------------------------------------

/// knowledge=None → 与 resolve_tool_path 逐字节一致（旁路零回归锁：
/// 相对 join 形态、围栏内绝对、越界拒绝文案三臂）。
#[test]
fn none_handle_matches_plain_resolve_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let b = ws_boundary_plain(dir.path());

    let rel =
        resolve_tool_path_with_redirect("sub/new.txt", Some(&b), FenceOpFamily::WriteDelete, None)
            .unwrap();
    assert_eq!(
        rel,
        resolve_tool_path("sub/new.txt", Some(&b), FenceOpFamily::WriteDelete).unwrap()
    );

    let inside_abs = dir.path().join("inside.txt");
    let got = resolve_tool_path_with_redirect(
        &inside_abs.to_string_lossy(),
        Some(&b),
        FenceOpFamily::WriteDelete,
        None,
    )
    .unwrap();
    assert_eq!(
        got,
        resolve_tool_path(
            &inside_abs.to_string_lossy(),
            Some(&b),
            FenceOpFamily::WriteDelete
        )
        .unwrap()
    );

    let outside = if cfg!(windows) {
        "Z:/elsewhere/x.txt"
    } else {
        "/elsewhere/x.txt"
    };
    let err = resolve_tool_path_with_redirect(outside, Some(&b), FenceOpFamily::WriteDelete, None)
        .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
}

/// 主链路（项目目录在 ws 外的典型形态）：LLM 发 pairA 子树绝对路径
/// （本地缺席）→ 围栏前 redirect → 镜像路径经 extra_roots 放行。
#[test]
fn multi_pair_dispatch_no_crosstalk() {
    let env = kb_env_multi();

    // pairA 子树三件套 → info_a 镜像。
    let a = env.code_a.join("x").join("AGENTS.md");
    let got = resolve_tool_path_with_redirect(
        &a.to_string_lossy(),
        boundary_of(&env),
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, env.info_a.join("x").join("AGENTS.md"));

    // pairB 子树 → info_b 镜像，不串 A 的镜像。
    let b = env.code_b.join("y").join("CLAUDE.md");
    let got = resolve_tool_path_with_redirect(
        &b.to_string_lossy(),
        boundary_of(&env),
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, env.info_b.join("y").join("CLAUDE.md"));

    // 不在任何 pair 子树的三件套 → 原样（无界形态比较，隔离围栏噪声）。
    let other = env._tmp.path().join("other").join("TIANGONG.md");
    let got = resolve_tool_path_with_redirect(
        &other.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, other);
}

/// 幂等：redirect 的输出（镜像路径）再喂回函数 → 原样（镜像路径不在任何
/// code_root 子树 → 不二次改写；重复消费同一路径安全）。
#[test]
fn redirect_is_idempotent() {
    let env = kb_env_multi();
    let first = resolve_tool_path_with_redirect(
        &env.code_a.join("AGENTS.md").to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(first, env.info_a.join("AGENTS.md"));
    let second = resolve_tool_path_with_redirect(
        &first.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(second, first, "镜像路径不得被二次改写");
}

/// 深层嵌套同构：code 子树内四级路径 → 镜像同级同构。
#[test]
fn deep_nested_path_redirects_isomorphic() {
    let env = kb_env_multi();
    let deep = env
        .code_a
        .join("l1")
        .join("l2")
        .join("l3")
        .join("TIANGONG.md");
    let got = resolve_tool_path_with_redirect(
        &deep.to_string_lossy(),
        None,
        FenceOpFamily::Read,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(
        got,
        env.info_a
            .join("l1")
            .join("l2")
            .join("l3")
            .join("TIANGONG.md")
    );
}

/// 非三件套文件永不重定向（write main.rs 落项目目录本地语义不受影响）。
#[test]
fn non_instruction_file_never_redirected() {
    let env = kb_env_multi();
    let src = env.code_a.join("src").join("main.rs");
    let got = resolve_tool_path_with_redirect(
        &src.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, src);
}

/// 本地优先 overlay：项目目录里三件套真实在场 → 原样（零变化）。
#[test]
fn local_existing_wins_over_mirror() {
    let env = kb_env_multi();
    let dir = env.code_a.join("docs");
    std::fs::create_dir_all(&dir).unwrap();
    let local = dir.join("AGENTS.md");
    std::fs::write(&local, "local rules").unwrap();
    let got = resolve_tool_path_with_redirect(
        &local.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, local);
    let _ = std::fs::remove_dir_all(&env._tmp);
}

/// 围栏交互两臂：镜像根在 extra_roots → 放行；缺席 → 围栏拒绝
/// （装配层忘放行时 fail-closed，不静默写穿）。
#[test]
fn fence_allows_mirror_only_via_extra_roots() {
    let env = kb_env_multi();
    let target = env.code_a.join("AGENTS.md");

    // 臂 1：extra_roots 含放行根（真实装配形态）→ 放行到镜像。
    let got = resolve_tool_path_with_redirect(
        &target.to_string_lossy(),
        boundary_of(&env),
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, env.info_a.join("AGENTS.md"));

    // 臂 2：extra_roots 空（忘放行）→ 围栏拒绝镜像路径。
    let bare = WorkspaceBoundary {
        extra_roots: Vec::new(),
        ..(*env.boundary).clone()
    };
    let err = resolve_tool_path_with_redirect(
        &target.to_string_lossy(),
        Some(&bare),
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
}

/// 围栏 × overlay 本地臂（真实链路 UAT 轮 4 回归锁，2026-11-26）：
/// 本地三件套在场 → resolve 原样返回本地路径，围栏须放行（code_root
/// 在 extra_roots）。初版装配只放行 info_root 时，此臂 access denied
/// （overlay 承诺虚假）——断言新形态放行 + 旧形态拒绝，双向钉死。
#[test]
fn fence_allows_local_overlay_via_code_root() {
    let env = kb_env_multi();
    let dir = env.code_a.join("docs");
    std::fs::create_dir_all(&dir).unwrap();
    let local = dir.join("AGENTS.md");
    std::fs::write(&local, "local rules").unwrap();

    // 新装配形态（code+info 双根）→ 本地路径原样返回且围栏放行。
    let got = resolve_tool_path_with_redirect(
        &local.to_string_lossy(),
        boundary_of(&env),
        FenceOpFamily::Read,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, local);

    // 旧装配形态（仅 info 根）→ 同一调用围栏拒绝（bug 形态封存）。
    let legacy = WorkspaceBoundary {
        extra_roots: vec![env.info_a.clone(), env.info_b.clone()],
        ..(*env.boundary).clone()
    };
    let err = resolve_tool_path_with_redirect(
        &local.to_string_lossy(),
        Some(&legacy),
        FenceOpFamily::Read,
        Some(env.handle.as_ref()),
    )
    .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
}

/// Windows 大小写混用：config 写大写盘符/目录、请求发小写 → 命中。
/// 仅 windows：大小写不敏感匹配是 Windows 产品形态（其余平台路径敏感，
/// 无此行为面可测）——平台形态测试按仓库约定挂 #[cfg(windows)]，Linux
/// 编译期消失。原 Linux 运行分支误把 exact 命中断言成 identity（exact
/// 路径照常命中重定向，CI coverage 腿 ubuntu 实录红）。
#[cfg(windows)]
#[test]
fn case_insensitive_dispatch_on_windows() {
    let env = kb_env_multi();
    // 大写 code_a 路径的小写形态（盘符与目录名逐段小写）。
    let p = PathBuf::from(env.code_a.to_string_lossy().to_lowercase()).join("AGENTS.md");
    let got = resolve_tool_path_with_redirect(
        &p.to_string_lossy(),
        None,
        FenceOpFamily::Read,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, env.info_a.join("AGENTS.md"));
}

// ---------------------------------------------------------------------------
// 四工具端到端（真实 IO，走 with_boundary + with_knowledge 生产装配形态）
// ---------------------------------------------------------------------------

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-kb", "user-kb", "agent:kb/session")
}

/// write_file：本地缺席 + pair 命中 → 内容落镜像；**项目目录零新增**
/// （枚举条目断言，不赌目录存在性——Windows 删除挂起幻影纪律）。
#[tokio::test]
async fn write_file_lands_in_mirror_project_dir_untouched() {
    let env = kb_env_multi();
    let tool =
        WriteFileTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let target = env.code_a.join("docs").join("AGENTS.md");
    let args = serde_json::json!({
        "path": target.to_string_lossy(),
        "content": "mirror rules v1",
    })
    .to_string();
    tool.execute(&args, &ctx()).await.unwrap();

    // 镜像落位。
    let mirror = env.info_a.join("docs").join("AGENTS.md");
    let written = std::fs::read_to_string(&mirror).unwrap();
    assert_eq!(written, "mirror rules v1");
    // 项目目录零新增（枚举 code_a 顶层条目——不应出现 docs/）。
    let entries: Vec<String> = std::fs::read_dir(&env.code_a)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !entries.iter().any(|n| n == "docs"),
        "项目目录出现新增条目：{entries:?}"
    );
}

/// read_file：本地缺席 → 读到镜像内容（overlay 读侧）。
#[tokio::test]
async fn read_file_reads_mirror() {
    let env = kb_env_multi();
    let mirror_dir = env.info_a.join("src");
    std::fs::create_dir_all(&mirror_dir).unwrap();
    std::fs::write(mirror_dir.join("AGENTS.md"), "from knowledge mirror").unwrap();

    let tool =
        ReadFileTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let target = env.code_a.join("src").join("AGENTS.md");
    let args = serde_json::json!({ "path": target.to_string_lossy() }).to_string();
    let out = tool.execute(&args, &ctx()).await.unwrap();
    assert!(out.contains("from knowledge mirror"), "{out}");
}

/// edit_file：本地缺席 → 编辑命中镜像（读改写全在镜像，项目目录零新增）。
#[tokio::test]
async fn edit_file_edits_mirror() {
    let env = kb_env_multi();
    let mirror_dir = env.info_a.join("src");
    std::fs::create_dir_all(&mirror_dir).unwrap();
    std::fs::write(mirror_dir.join("CLAUDE.md"), "rule A\nrule B\nrule C\n").unwrap();

    let tool =
        EditFileTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let target = env.code_a.join("src").join("CLAUDE.md");
    let args = serde_json::json!({
        "path": target.to_string_lossy(),
        "old_text": "rule B",
        "new_text": "rule B2",
    })
    .to_string();
    tool.execute(&args, &ctx()).await.unwrap();

    let edited = std::fs::read_to_string(mirror_dir.join("CLAUDE.md")).unwrap();
    assert_eq!(edited, "rule A\nrule B2\nrule C\n");
    // 项目目录零新增。
    let entries: Vec<String> = std::fs::read_dir(&env.code_a)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !entries.iter().any(|n| n == "src"),
        "项目目录出现新增条目：{entries:?}"
    );
}

/// multiedit 单调用双 pair 多路径：同一调用的两条编辑分别落在两个 pair
/// 的镜像（per-edit redirect 分发互不串台），项目目录双零新增。
#[tokio::test]
async fn multiedit_single_call_dispatches_both_pairs() {
    let env = kb_env_multi();
    let dir_a = env.info_a.join("x");
    let dir_b = env.info_b.join("y");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    std::fs::write(dir_a.join("AGENTS.md"), "alpha one\nalpha two\n").unwrap();
    std::fs::write(dir_b.join("TIANGONG.md"), "beta one\nbeta two\n").unwrap();

    let tool =
        MultiEditTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let args = serde_json::json!({
        "edits": [
            {
                "path": env.code_a.join("x").join("AGENTS.md").to_string_lossy(),
                "old_text": "alpha two",
                "new_text": "alpha TWO",
            },
            {
                "path": env.code_b.join("y").join("TIANGONG.md").to_string_lossy(),
                "old_text": "beta one",
                "new_text": "beta ONE",
            },
        ],
    })
    .to_string();
    tool.execute(&args, &ctx()).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(dir_a.join("AGENTS.md")).unwrap(),
        "alpha one\nalpha TWO\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir_b.join("TIANGONG.md")).unwrap(),
        "beta ONE\nbeta two\n"
    );
    // 项目目录双零新增。
    for code in [&env.code_a, &env.code_b] {
        let entries: Vec<String> = std::fs::read_dir(code)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let forbidden = if code == &env.code_a { "x" } else { "y" };
        assert!(
            !entries.iter().any(|n| n == forbidden),
            "{code:?} 出现新增条目：{entries:?}"
        );
    }
}

/// knowledge=None 旁路零回归：无句柄 → 零重定向。真实装配边界
/// （code_root 在 extra_roots）下写项目目录 = 落本地（围栏放行，非
/// 三件套的常规项目写同语义）；无放行根的裸边界下同路径仍围栏拒绝
/// （越界拦的兜底臂不受知识库装配影响）。两臂均不产生镜像。
#[tokio::test]
async fn none_handle_tools_write_local_legacy_path() {
    let env = kb_env_multi();
    // 真实装配边界 + 无句柄：写项目目录三件套路径 → 不重定向，落本地。
    let tool = WriteFileTool::with_boundary(env.boundary.clone()).with_knowledge(None);
    let target = env.code_a.join("AGENTS.md");
    let args = serde_json::json!({
        "path": target.to_string_lossy(),
        "content": "plain write",
    })
    .to_string();
    tool.execute(&args, &ctx()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "plain write");
    assert!(!env.info_a.join("AGENTS.md").exists());

    // 裸边界（extra_roots 空 = 知识库未启用形态）：同路径围栏拒绝。
    let bare = Arc::new(WorkspaceBoundary {
        extra_roots: Vec::new(),
        ..(*env.boundary).clone()
    });
    let tool = WriteFileTool::with_boundary(bare).with_knowledge(None);
    let args = serde_json::json!({
        "path": target.to_string_lossy(),
        "content": "blocked write",
    })
    .to_string();
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
    assert!(!env.info_a.join("AGENTS.md").exists());

    // ws 内路径（boundary root 子树）：落本地，不落镜像。
    let ws_target = env.ws.join("AGENTS.md");
    let args = serde_json::json!({
        "path": ws_target.to_string_lossy(),
        "content": "plain write",
    })
    .to_string();
    tool.execute(&args, &ctx()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&ws_target).unwrap(), "plain write");
    assert!(!env.info_a.join("AGENTS.md").exists());
    assert!(!env.info_b.join("AGENTS.md").exists());
}

/// 声明注入面（多 pair 多层）已在 knowledge/tests.rs 覆盖；此处补一个
/// 端到端形态：handle 读到的配置即时生效（写盘 → 下一次消费点重读）——
/// 热生效锁：追加 pairB 后，pairA 路径照常、pairB 路径从拒绝变命中。
#[test]
fn hot_reload_new_pair_takes_effect_without_rebuild() {
    let env = kb_env_multi();
    let b = env.code_b.join("AGENTS.md");

    // 初始 config 只有 pairA（kb_env_multi 写了 A+B；这里重写只留 A）。
    let config_path = env.ws.join("config").join("config.knowledge.json");
    save_knowledge_config(
        &config_path,
        &KnowledgeConfig {
            enabled: true,
            pairs: vec![pair(&env.code_a, &env.info_a)],
        },
    )
    .unwrap();
    let got = resolve_tool_path_with_redirect(
        &b.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, b, "未挂载 pair 的子树应原样");

    // 写盘追加 pairB → 下一次消费点自动重读（mtime 缓存热生效）。
    save_knowledge_config(
        &config_path,
        &KnowledgeConfig {
            enabled: true,
            pairs: vec![
                pair(&env.code_a, &env.info_a),
                pair(&env.code_b, &env.info_b),
            ],
        },
    )
    .unwrap();
    let got = resolve_tool_path_with_redirect(
        &b.to_string_lossy(),
        None,
        FenceOpFamily::WriteDelete,
        Some(env.handle.as_ref()),
    )
    .unwrap();
    assert_eq!(got, env.info_b.join("AGENTS.md"), "新 pair 应热生效");
}

// ---------------------------------------------------------------------------
// preview 重定向感知（第五轮横扫 2026-11-27）
// checkpoint 预检（tool_dispatch preview → turn_file_changes + snapshot）
// 声明的必须是 execute 的真实落盘路径——镜像写不设防 = 快照记原路径、
// rewind 永不回滚镜像、file_changes 错位。
// ---------------------------------------------------------------------------

/// write_file preview：声明镜像路径；kind 按镜像侧现状如实（缺席 Create /
/// 在场 Modify）。
#[test]
fn preview_write_declares_mirror_path_and_true_kind() {
    let env = kb_env_multi();
    let tool =
        WriteFileTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let target = env.code_a.join("docs").join("AGENTS.md");
    let args = serde_json::json!({
        "path": target.to_string_lossy(),
        "content": "x",
    })
    .to_string();

    // 臂 1：镜像缺席 → Create。
    let fc = tool.preview(&args).expect("preview 应声明");
    assert_eq!(
        fc.path,
        env.info_a.join("docs").join("AGENTS.md").to_string_lossy()
    );
    assert_eq!(fc.kind, FileChangeKind::Create);

    // 臂 2：镜像在场 → Modify（存在性判定用解析后路径）。
    let mirror_dir = env.info_a.join("docs");
    std::fs::create_dir_all(&mirror_dir).unwrap();
    std::fs::write(mirror_dir.join("AGENTS.md"), "mirror").unwrap();
    let fc = tool.preview(&args).expect("preview 应声明");
    assert_eq!(fc.path, mirror_dir.join("AGENTS.md").to_string_lossy());
    assert_eq!(fc.kind, FileChangeKind::Modify);
    let _ = std::fs::remove_dir_all(&env._tmp);
}

/// edit_file preview：edit/append 双 action 声明镜像路径（append 存在性
/// 判定同样用解析后路径）。
#[test]
fn preview_edit_declares_mirror_path_both_actions() {
    let env = kb_env_multi();
    let mirror_dir = env.info_a.join("src");
    std::fs::create_dir_all(&mirror_dir).unwrap();
    std::fs::write(mirror_dir.join("CLAUDE.md"), "rule A\n").unwrap();

    let tool =
        EditFileTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));

    // edit action → 镜像 Modify。
    let args = serde_json::json!({
        "path": env.code_a.join("src").join("CLAUDE.md").to_string_lossy(),
        "old_text": "rule A",
        "new_text": "rule B",
    })
    .to_string();
    let fc = tool.preview(&args).expect("preview 应声明");
    assert_eq!(fc.path, mirror_dir.join("CLAUDE.md").to_string_lossy());
    assert_eq!(fc.kind, FileChangeKind::Modify);

    // append 到缺席镜像 → Create。
    let args2 = serde_json::json!({
        "path": env.code_a.join("src").join("AGENTS.md").to_string_lossy(),
        "action": "append",
        "content": "more",
    })
    .to_string();
    let fc2 = tool.preview(&args2).expect("preview 应声明");
    assert_eq!(fc2.path, mirror_dir.join("AGENTS.md").to_string_lossy());
    assert_eq!(fc2.kind, FileChangeKind::Create);
    let _ = std::fs::remove_dir_all(&env._tmp);
}

/// multiedit preview_all：逐条解析 + 按**解析后路径**去重（同一镜像经
/// `.` 组件噪声形态两次声明只快照一次——与 execute 内存工作区同键）。
#[test]
fn preview_multiedit_resolves_and_dedups_by_mirror_path() {
    let env = kb_env_multi();
    let tool =
        MultiEditTool::with_boundary(env.boundary.clone()).with_knowledge(Some(env.handle.clone()));
    let edits = serde_json::json!({
        "edits": [
            {
                "path": env.code_a.join("x").join("AGENTS.md").to_string_lossy(),
                "old_text": "a",
                "new_text": "b",
            },
            {
                "path": env.code_a.join("x").join(".").join("AGENTS.md").to_string_lossy(),
                "old_text": "c",
                "new_text": "d",
            },
            {
                "path": env.code_b.join("y").join("TIANGONG.md").to_string_lossy(),
                "old_text": "e",
                "new_text": "f",
            },
        ],
    })
    .to_string();
    let changes = tool.preview_all(&edits);
    assert_eq!(changes.len(), 2, "同一镜像重复声明应去重: {changes:?}");
    assert_eq!(
        changes[0].path,
        env.info_a.join("x").join("AGENTS.md").to_string_lossy()
    );
    assert_eq!(
        changes[1].path,
        env.info_b.join("y").join("TIANGONG.md").to_string_lossy()
    );
    let _ = std::fs::remove_dir_all(&env._tmp);
}

/// knowledge=None 旁路零回归锁（preview 面）：无句柄 → preview 声明
/// **原样入参路径**（与旧 preview 行为逐字节一致——快照/回滚对无知识库
/// 部署零影响）。
#[test]
fn preview_without_knowledge_declares_raw_path() {
    let env = kb_env_multi();
    let tool = WriteFileTool::with_boundary(env.boundary.clone()).with_knowledge(None);
    let target = env.code_a.join("AGENTS.md");
    let args = serde_json::json!({ "path": target.to_string_lossy(), "content": "x" }).to_string();
    let fc = tool.preview(&args).expect("preview 应声明");
    assert_eq!(
        fc.path,
        target.to_string_lossy(),
        "无句柄 = 旧 preview 行为（原样路径声明）"
    );
    let _ = std::fs::remove_dir_all(&env._tmp);
}

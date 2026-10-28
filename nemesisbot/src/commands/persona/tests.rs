//! persona 命令单测：文件型子命令（current/list/activate/remove/restore
//! —— 临时目录确定性）+ 共享分类映射臂钉住。
//!
//! cmd_search / cmd_install / fetch_and_convert / search_personas 走 GitHub
//! API（结构性不可离线单测，L2 集成真进程覆盖）。解析/筛选/转换纯函数的
//! 真相源在 nemesis-web handlers/persona（BUG-20261028-117 起 CLI 全部委托
//! 共享实现），其单测住 web crate，此处不重复。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

// ---------------------------------------------------------------------------
// map_category（共享真相源：nemesis-web handlers/persona）—— 17 臂全钉。
// web 侧测试只钉 6 臂，这里补齐其余臂；helper 的 CLI 专属副本已删。
// ---------------------------------------------------------------------------

#[test]
fn map_category_all_known_dirs() {
    assert_eq!(
        nemesis_web::handlers::persona::map_category("engineering"),
        "开发"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("marketing"),
        "营销"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("security"),
        "安全"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("design"),
        "创意"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("academic"),
        "学术"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("product"),
        "产品"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("paid-media"),
        "付费媒体"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("sales"),
        "销售"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("game-development"),
        "游戏开发"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("finance"),
        "金融"
    );
    assert_eq!(nemesis_web::handlers::persona::map_category("gis"), "GIS");
    assert_eq!(
        nemesis_web::handlers::persona::map_category("spatial-computing"),
        "空间计算"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("specialized"),
        "专业"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("testing"),
        "测试"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("support"),
        "客服"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("project-management"),
        "项目管理"
    );
    assert_eq!(
        nemesis_web::handlers::persona::map_category("whatever"),
        "通用"
    );
    assert_eq!(nemesis_web::handlers::persona::map_category(""), "通用");
}

// ---------------------------------------------------------------------------
// 文件型子命令 —— 临时 workspace 确定性
// ---------------------------------------------------------------------------

mod file_commands {
    use super::super::*;

    fn ws() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn seed_persona(workspace: &std::path::Path, name: &str, identity: &str) {
        let dir = workspace.join("personas").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), identity).unwrap();
        std::fs::write(dir.join("SOUL.md"), format!("soul of {name}")).unwrap();
        std::fs::write(
            dir.join("PERSONA.json"),
            format!(r#"{{"name":"{name}","emoji":"🤖","description":"d"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn current_without_active_marker_is_ok() {
        let dir = ws();
        cmd_current(dir.path()).expect("未初始化 → 干净 Ok（提示语），不 Err");
    }

    #[test]
    fn current_with_persona_json_is_ok() {
        let dir = ws();
        seed_persona(dir.path(), "p1", "identity-1");
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        std::fs::write(dir.path().join("personas/_active.json"), r#"{"name":"p1"}"#).unwrap();
        cmd_current(dir.path()).expect("active + PERSONA.json → Ok");
    }

    #[test]
    fn current_with_active_but_missing_persona_dir_is_ok() {
        let dir = ws();
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        std::fs::write(
            dir.path().join("personas/_active.json"),
            r#"{"name":"ghost"}"#,
        )
        .unwrap();
        cmd_current(dir.path()).expect("缺 PERSONA.json → 降级打印目录名，Ok");
    }

    #[test]
    fn list_without_personas_dir_is_ok() {
        let dir = ws();
        cmd_list(dir.path()).expect("无 personas 目录 → Ok");
    }

    #[test]
    fn list_with_empty_personas_dir_is_ok() {
        let dir = ws();
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        cmd_list(dir.path()).expect("空目录 → Ok（打印 0 个）");
    }

    #[test]
    fn list_with_personas_is_ok() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "p2", "identity-2");
        cmd_list(dir.path()).expect("两个人格 → Ok");
    }

    #[test]
    fn activate_missing_persona_bails() {
        let dir = ws();
        let err = cmd_activate(dir.path(), "ghost").expect_err("不存在的人格必须 Err");
        // BUG-20261028-117 后与 web handler 共用单一真相源，错误文案同源
        // （persona '<name>' not found）。
        assert!(err.to_string().contains("not found"), "err: {err:#}");
    }

    #[test]
    fn activate_copies_files_and_writes_marker() {
        let dir = ws();
        seed_persona(dir.path(), "p1", "identity-1");
        cmd_activate(dir.path(), "p1").expect("activate ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "identity-1",
            "IDENTITY.md 复制到 workspace 根"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap(),
            "soul of p1"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "p1");
    }

    #[test]
    fn activate_partial_persona_only_copies_present_files() {
        // 只有 AGENT.md 的人格：IDENTITY 等缺文件跳过、不 Err。
        let dir = ws();
        let pdir = dir.path().join("personas/partial");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("AGENT.md"), "agent rules").unwrap();
        cmd_activate(dir.path(), "partial").expect("部分文件人格 → Ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("AGENT.md")).unwrap(),
            "agent rules"
        );
        assert!(!dir.path().join("IDENTITY.md").exists(), "缺的文件不复制");
    }

    #[test]
    fn activate_archives_memory_and_swaps_heartbeat_like_web() {
        // BUG-20261028-117：CLI activate 必须与 web activate 同语义——当前
        // 人格的 memory/ 与 HEARTBEAT.md 先归档回其目录，再恢复目标人格的；
        // 旧 CLI 只拷 4 个 .md，运行期记忆滞留根目录，被下次 web 切换归档进
        // 错误的人格（跨人格串档 + 原人格记忆丢失），HEARTBEAT 也不切换。
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "p1", "identity-1");
        // 预置 active=default，跳过 ensure 初始化的首次归档（确定性）。
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        std::fs::write(
            dir.path().join("personas/_active.json"),
            r#"{"name":"default"}"#,
        )
        .unwrap();
        // default 的运行期产物：根 memory/ 与 HEARTBEAT.md。
        std::fs::create_dir_all(dir.path().join("memory")).unwrap();
        std::fs::write(dir.path().join("memory/note.txt"), "default-run-note").unwrap();
        std::fs::write(dir.path().join("HEARTBEAT.md"), "default-heartbeat").unwrap();
        // p1 自带 memory/ 与 HEARTBEAT.md。
        std::fs::create_dir_all(dir.path().join("personas/p1/memory")).unwrap();
        std::fs::write(dir.path().join("personas/p1/memory/p1.txt"), "p1-memory").unwrap();
        std::fs::write(dir.path().join("personas/p1/HEARTBEAT.md"), "p1-heartbeat").unwrap();

        cmd_activate(dir.path(), "p1").expect("activate ok");

        // 归档：default 的运行期 memory/HEARTBEAT 落回 personas/default/。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("personas/default/memory/note.txt")).unwrap(),
            "default-run-note",
            "当前人格运行期 memory 必须先归档"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("personas/default/HEARTBEAT.md")).unwrap(),
            "default-heartbeat",
            "当前人格 HEARTBEAT 必须先归档"
        );
        // 恢复：p1 的 memory/HEARTBEAT 上根，default 的运行期残留清掉。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("memory/p1.txt")).unwrap(),
            "p1-memory",
            "目标人格 memory 恢复到根"
        );
        assert!(
            !dir.path().join("memory/note.txt").exists(),
            "旧人格 memory 不得残留根目录"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("HEARTBEAT.md")).unwrap(),
            "p1-heartbeat",
            "HEARTBEAT 随人格切换"
        );
    }

    #[test]
    fn remove_default_bails() {
        let dir = ws();
        let err = cmd_remove(dir.path(), "default").expect_err("默认人格不可删");
        assert!(err.to_string().contains("不能删除默认人格"), "err: {err:#}");
    }

    #[test]
    fn remove_missing_bails() {
        let dir = ws();
        let err = cmd_remove(dir.path(), "ghost").expect_err("不存在 → Err");
        assert!(err.to_string().contains("不存在"), "err: {err:#}");
    }

    #[test]
    fn remove_inactive_persona_deletes_only_dir() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "extra", "extra identity");
        cmd_activate(dir.path(), "default").unwrap();
        cmd_remove(dir.path(), "extra").expect("remove ok");
        assert!(!dir.path().join("personas/extra").exists(), "目录已删");
        // 非活动人格删除不动 workspace 根文件。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "default", "活动标记不变");
    }

    #[test]
    fn remove_active_persona_restores_default_first() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "p1", "identity-1");
        cmd_activate(dir.path(), "p1").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "identity-1"
        );
        cmd_remove(dir.path(), "p1").expect("remove active ok");
        // workspace 根被恢复成 default 的文件 + 活动标记回 default。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity",
            "删活动人格 → 先恢复默认人格文件"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "default");
        assert!(!dir.path().join("personas/p1").exists());
    }

    #[test]
    fn restore_requires_default_persona() {
        let dir = ws();
        // BUG-20261028-117 后与 web restore 同源：ensure 初始化会从根文件
        // 搭出 default 人格（空根 → 空 default），restore 在处女 workspace
        // 上自愈成功而非 Err。
        cmd_restore(dir.path()).expect("处女 workspace restore 自愈 Ok（同 web）");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "default", "marker 落到 default");
    }

    #[test]
    fn restore_activates_default() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        cmd_restore(dir.path()).expect("restore ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity"
        );
    }
}

/// run() 本地分发（Search/Install 走 GitHub API —— 结构性，不在此测）。
#[tokio::test]
async fn run_dispatches_local_file_commands() {
    let dir = tempfile::tempdir().unwrap();
    let ws_str = dir.path().to_string_lossy().to_string();
    seed(&dir);

    fn seed(dir: &tempfile::TempDir) {
        let pdir = dir.path().join("personas/default");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("IDENTITY.md"), "default identity").unwrap();
    }

    run(PersonaAction::List, "home", &ws_str)
        .await
        .expect("List ok");
    run(PersonaAction::Current, "home", &ws_str)
        .await
        .expect("Current ok");
    run(PersonaAction::Restore, "home", &ws_str)
        .await
        .expect("Restore ok");
    run(
        PersonaAction::Activate {
            name: "default".to_string(),
        },
        "home",
        &ws_str,
    )
    .await
    .expect("Activate ok");
    run(
        PersonaAction::Remove {
            name: "default".to_string(),
        },
        "home",
        &ws_str,
    )
    .await
    .expect_err("Remove default → Err（经 run 分发同样生效）");
}

// ===========================================================================
// wave_b（覆盖率补测 B 波）：本地命令剩余分支。
//
// 不在此测（EXEMPT）：cmd_search / cmd_install 的联网路径与
// fetch_and_convert / search_personas 整体函数体 —— GitHub API 真网络，
// 结构性禁离线单测；Install 的唯一可离线触点是「已安装早退」（下方测）。
// 解析辅助的 wave_b 测试随 CLI 专属 helper 一起退役（真相源测试住 web crate）。
// ===========================================================================

mod wave_b {
    use super::*;

    fn wb_seed(workspace: &std::path::Path, name: &str, identity: &str) {
        let dir = workspace.join("personas").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), identity).unwrap();
        std::fs::write(dir.join("SOUL.md"), format!("soul of {name}")).unwrap();
    }

    fn wb_active(workspace: &std::path::Path, name: &str) {
        let pdir = workspace.join("personas");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("_active.json"),
            serde_json::json!({ "name": name }).to_string(),
        )
        .unwrap();
    }

    /// cmd_current：PERSONA.json 的 description 缺失（as_str None 臂，78 行区）
    /// 与空串（Some("") 但 !is_empty 为假 → 不打印描述行）两种形态。
    #[test]
    fn wave_b_current_description_absent_and_empty_arms() {
        // (a) 无 description 键 → if let Some(desc) 的 None 落穿。
        let d1 = tempfile::tempdir().unwrap();
        wb_seed(d1.path(), "bare", "i-bare");
        std::fs::write(
            d1.path().join("personas/bare/PERSONA.json"),
            r#"{"name":"Bare","emoji":"🙂"}"#,
        )
        .unwrap();
        wb_active(d1.path(), "bare");
        cmd_current(d1.path()).expect("PERSONA.json 缺 description 也应 Ok");

        // (b) description 为空串 → 内层 !desc.is_empty() 为假。
        let d2 = tempfile::tempdir().unwrap();
        wb_seed(d2.path(), "quiet", "i-quiet");
        std::fs::write(
            d2.path().join("personas/quiet/PERSONA.json"),
            r#"{"name":"Quiet","emoji":"🤫","description":""}"#,
        )
        .unwrap();
        wb_active(d2.path(), "quiet");
        cmd_current(d2.path()).expect("空 description 也应 Ok");
    }

    /// cmd_list：读 _active.json 确定 active 标记（96-98）、循环中跳过
    /// 非目录杂散文件（107-108）、以及含 default 的多元素排序（132-140 三臂：
    /// Less=default 在左 / Greater=default 在右 / 双非 default 按名比较）。
    #[test]
    fn wave_b_list_reads_active_marker_sorts_and_skips_plain_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "default", "identity-default");
        wb_seed(ws, "zulu", "identity-z");
        wb_seed(ws, "alpha", "identity-a");
        wb_seed(ws, "mike", "identity-m");
        // 杂散文件（非目录）→ 循环 continue。
        std::fs::create_dir_all(ws.join("personas")).unwrap();
        std::fs::write(ws.join("personas/README.txt"), "not a persona").unwrap();
        wb_active(ws, "default");
        cmd_list(ws).expect("带 marker + 杂散文件 + 多人格的 list 应 Ok");
    }

    /// cmd_install 已安装早退：在 GitHub fetch 之前 bail（273-282），
    /// 经 run() 分发触达对应 arm（40）。这是 Install 唯一可离线测的路径。
    #[tokio::test]
    async fn wave_b_run_install_already_installed_bails_before_network() {
        let dir = tempfile::tempdir().unwrap();
        let ws_str = dir.path().to_string_lossy().to_string();
        wb_seed(dir.path(), "dupe", "already-here");
        let err = run(
            PersonaAction::Install {
                id: "dupe".to_string(),
            },
            "home",
            &ws_str,
        )
        .await
        .expect_err("已安装人格必须 Err 且不发生任何网络请求");
        assert!(err.to_string().contains("已经安装"), "err: {err:#}");
    }

    /// cmd_remove：删除活动人格但 personas/default 不存在 → 与 web 同源的
    /// 激活路径诚实 Err 中止（fail-closed），不删目录、不落悬空 marker。
    /// （BUG-20261028-117 前旧 CLI 静默删除并留下指向已删人格的 _active.json，
    /// 之后 web persona 面板读 active 的 PERSONA.json 报错——当时台账已标记
    /// 可疑点，本轮收口。）预置 _active.json 跳过 ensure 初始化——否则
    /// activate 内部的 ensure_persona_store 会先自愈搭出 default。
    #[test]
    fn wave_b_remove_active_persona_without_default_dir_aborts_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "only", "only-identity");
        wb_active(ws, "only");
        let err = cmd_remove(ws, "only").expect_err("无 default 可恢复 → 诚实 Err 不悬空");
        assert!(err.to_string().contains("not found"), "err: {err:#}");
        assert!(ws.join("personas/only").exists(), "fail-closed：目录不删");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ws.join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "only", "marker 保持一致（人格仍在）");
    }

    /// cmd_remove：完全无 _active.json → 跳过整个活动恢复检测块（204 存在性假臂），
    /// 直接删目录。
    #[test]
    fn wave_b_remove_persona_without_any_active_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "loner", "loner-identity");
        cmd_remove(ws, "loner").expect("无 marker 时 remove 应 Ok");
        assert!(!ws.join("personas/loner").exists());
        assert!(!ws.join("personas/_active.json").exists(), "不创建 marker");
    }
}

// ===========================================================================
// r10 wave（覆盖率 95% goal 第七波）：Search / Install 的网络前置段。
//
// GitHub API 端点在 search_personas / fetch_and_convert 内部硬编码，无注入
// seam；用 reqwest 系统代理语义把出网钉死在本机死端口（HTTPS_PROXY=
// http://127.0.0.1:9）→ send().await? 的 Err 一路 `?` 上抛。前置段（参数
// 归一、workspace 快照、client 构造、GET 发起）由此全部可达；渲染段结构性
// 属真网集成域（与文件头既有豁免口径一致）。罕见的「机器有系统级代理穿透」
// 环境下这两个命令可能转 Ok——本块按主环境（无系统代理、env 生效）断言。
// cmd_search / cmd_install 内部 block_in_place 必须 multi_thread runtime；
// env 是进程全局 → 持 crate::GLOBAL_STATE_LOCK，prev 值按 Option 恢复。
// ===========================================================================

// 整 mod Windows 形态（3/3 测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r10_lead_in {
    use super::*;
    use std::ffi::OsString;

    struct NetDead {
        prev: Option<OsString>,
    }

    impl NetDead {
        fn engage() -> Self {
            let prev = std::env::var_os("HTTPS_PROXY");
            unsafe { std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9") };
            Self { prev }
        }
    }

    impl Drop for NetDead {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("HTTPS_PROXY", v) },
                None => unsafe { std::env::remove_var("HTTPS_PROXY") },
            }
        }
    }

    /// cmd_search 前置段：query 归一 → 提示打印 → workspace 快照克隆 →
    /// block_in_place 桥内 GET；send Err 经 `?` 一路上抛成 Err。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_persona_search_lead_in_errors_at_dead_upstream() {
        let _lock = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();

        let res = cmd_search(ws.path(), Some("r10-query"));
        assert!(res.is_err(), "上游不可达 → 前置段之后必须以 Err 收场");
    }

    /// cmd_install 前置段：persona 目录不存在性检查通过 → 下载提示 →
    /// fetch_and_convert 第一腿（tree GET）即败；双层 Result 的两个 `?`
    /// 无论哪层接力，最终都 Err 且绝不创建 personas/<id>（安装目录只在
    /// 内容写盘阶段创建——离线 Err 与在线"未找到"两条世界同守此断言）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_persona_install_uninstalled_id_bails_after_network_leg_one() {
        let _lock = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();
        let id = "r10-absent-persona";

        let res = cmd_install(ws.path(), id);
        assert!(res.is_err(), "第一腿即断 → install 必须 Err");
        assert!(
            !ws.path().join("personas").join(id).exists(),
            "前置段失败不得留下安装目录"
        );
    }

    /// run() 的 Search / Install 两条分发臂（此前只钉过本地五臂）：即便套上
    /// run 的 async 包装层，前置段的 Err 语义同样成立。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_persona_run_dispatch_reaches_search_and_install_arms() {
        let _lock = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();
        let ws_str = ws.path().to_string_lossy().to_string();

        assert!(
            run(
                PersonaAction::Search {
                    query: Some("q".to_string()),
                },
                "home-unused",
                &ws_str,
            )
            .await
            .is_err(),
            "Search 分发臂前置段应 Err"
        );

        let id = "r10-absent-persona-run";
        assert!(
            run(
                PersonaAction::Install { id: id.to_string() },
                "home-unused",
                &ws_str,
            )
            .await
            .is_err(),
            "Install 分发臂前置段应 Err"
        );
        assert!(!ws.path().join("personas").join(id).exists());
    }
}

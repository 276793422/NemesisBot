//! sandbox CLI 命令纯函数测试（M2 补测，2026-08-25）。
//!
//! 其余函数（start/stop/install/kill/ensure_ready/...）与真实 Windows 服务/
//! UAC 提权/Sandboxie 引擎交互，属结构性不可单测——真机链路由
//! `nemesisbot/tests/executor.rs`（spawn_and_call 端到端）与 Dashboard
//! 沙盒页真机验证覆盖。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

#[test]
fn format_size_boundaries() {
    assert_eq!(format_size(0), "0B");
    assert_eq!(format_size(512), "512B");
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1024), "1K");
    // 整除显示（非四舍五入）：1500B = 1K（1464..2047 都显示 1K）
    assert_eq!(format_size(1500), "1K");
    assert_eq!(format_size(1024 * 1024 - 1), "1023K");
    assert_eq!(format_size(1024 * 1024), "1M");
    assert_eq!(format_size(3 * 1024 * 1024), "3M");
}

#[test]
fn box_file_root_parses_ini_section_and_strips_nt_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("Sandboxie.ini");
    std::fs::write(
        &ini,
        "[GlobalSettings]\nEnabled=y\n\n\
         [NemesisBox]\nEnabled=y\nAllowNetworkAccess=n\n\
         FileRootPath=\\??\\C:\\Users\\bot\\.nemesisbot\\workspace\\tools\\sandboxie\\box\\NemesisBox\n\
         DropAdminRights=y\n",
    )
    .unwrap();
    let root = box_file_root(&ini, "NemesisBox").expect("FileRootPath present in section");
    assert_eq!(
        root,
        std::path::PathBuf::from(
            r"C:\Users\bot\.nemesisbot\workspace\tools\sandboxie\box\NemesisBox"
        ),
        "NT \\??\\ prefix must be stripped"
    );
    // 其他段里没有 FileRootPath → None（段匹配生效）
    assert!(box_file_root(&ini, "GlobalSettings").is_none());
    // 不存在的段 → None
    assert!(box_file_root(&ini, "NoSuchBox").is_none());
}

#[test]
fn box_file_root_handles_missing_file_plain_path_and_section_switch() {
    // 文件不存在 → None（read_to_string().ok()?）
    assert!(box_file_root(std::path::Path::new("Z:/definitely/missing.ini"), "X").is_none());

    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("Sandboxie.ini");
    // 两个段各有一个 FileRootPath：段切换必须让查询命中正确段的值
    std::fs::write(
        &ini,
        "[BoxA]\nFileRootPath=D:\\boxes\\a\n\n[BoxB]\nFileRootPath=\\??\\D:\\boxes\\b\n",
    )
    .unwrap();
    assert_eq!(
        box_file_root(&ini, "BoxA"),
        Some(std::path::PathBuf::from(r"D:\boxes\a")),
        "plain path (no NT prefix) returned as-is"
    );
    assert_eq!(
        box_file_root(&ini, "BoxB"),
        Some(std::path::PathBuf::from(r"D:\boxes\b"))
    );
}

#[test]
fn relaunch_args_shape() {
    assert_eq!(
        relaunch_args("stop", false),
        vec!["sandbox".to_string(), "stop".to_string(), "--internal".to_string()]
    );
    assert_eq!(
        relaunch_args("ensure-ready", true),
        vec![
            "sandbox".to_string(),
            "ensure-ready".to_string(),
            "--internal".to_string(),
            "--local".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// user_profile / workspace_dir / ensure_sandbox_ready 决策表 Row 1-2（M2 补测）
// ---------------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn user_profile_matches_env() {
    // USERPROFILE 存在（Windows 测试环境恒有）→ 原样返回。
    let envp = std::env::var_os("USERPROFILE").expect("USERPROFILE set on Windows");
    assert_eq!(user_profile(), std::path::PathBuf::from(envp));
}

#[test]
fn workspace_dir_local_flag_points_at_cwd_dotdir() {
    // --local → {cwd}/.nemesisbot/workspace（不经 env，确定性形状）。
    let ws = workspace_dir(true);
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(ws, cwd.join(".nemesisbot").join("workspace"));
}

#[test]
fn ensure_sandbox_ready_disabled_is_noop() {
    // 决策表 Row 1：开关关 → 直接返回（不做任何探测/安装）。
    let tmp = tempfile::tempdir().unwrap();
    ensure_sandbox_ready(tmp.path(), false);
}

#[test]
fn ensure_sandbox_ready_missing_runtime_skips_engine_ensure() {
    // 决策表 Row 2：开但 runtime 文件缺失（临时 home 无 Sandboxie）→ 提前
    // 返回，不触发 UAC / 服务操作（本测试环境下无弹窗即通过）。
    let tmp = tempfile::tempdir().unwrap();
    ensure_sandbox_ready(tmp.path(), true);
}

// =========================================================================
// S11b 覆盖率冲刺：sandbox run() 分发 arm（Status/Pending/Commit/Clear/Kill）
// + pending/commit 假盒路径 + kill 全分支 + run_startexe_timeout 三分支
// + stop_service_if_ours。
// 豁免（真引擎/真 UAC/真下载，不测）：Install / Start / Stop /
// EnsureReady（internal=ensure_installed 真装；external=relaunch_elevated UAC）。
// 假 Start.exe 用 .bat（CreateProcess 经 cmd 自动执行）。
// =========================================================================

struct S11bTempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

fn s11b_paths(home: &std::path::Path) -> nemesis_sandbox::SandboxPaths {
    nemesis_sandbox::SandboxPaths::new(home)
}

/// 真路径 → 盒内镜像路径（`C:\a\b` → `<box_root>/drive/C/a/b`）。
/// 注意 real_path_for_box 期望 `drive/<L>` 的 L 不带冒号。
fn s11b_box_mirror(real: &std::path::Path, box_root: &std::path::Path) -> std::path::PathBuf {
    let s = real.to_string_lossy().replace('/', "\\");
    let (drive, rest) = s.split_at(2);
    assert!(drive.ends_with(':'), "期望绝对路径带盘符: {s}");
    let letter = &drive[..1]; // "C:" → "C"
    box_root
        .join("drive")
        .join(letter)
        .join(rest.trim_start_matches('\\'))
}

/// 放一个假 Start.exe：复制系统 cmd.exe（真 PE）——
/// `Start.exe /box:X delete_sandbox` / `/box:X /silent /terminate` 实测 rc=0。
/// 注意不能用「.bat 内容改名为 .exe」：CreateProcess 按 .exe 后缀做 PE 加载，
/// 批处理文本会报 os error 216（版本不兼容）。
fn s11b_fake_start_exe(paths: &nemesis_sandbox::SandboxPaths) -> bool {
    let src = std::path::PathBuf::from(
        std::env::var_os("SystemRoot")
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| r"C:\Windows".into()),
    )
    .join("System32")
    .join("cmd.exe");
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    std::fs::copy(&src, paths.start_exe()).is_ok()
}

// ------------------------------ status ------------------------------------

#[tokio::test]
async fn test_s11b_run_status_fresh_home() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    run(SandboxCommand::Status, false).await.unwrap();
    // SCM 读取安全（SbieSvc/SbieDrv 未装 → NotFound 打印）
    assert!(!s11b_paths(&th.home).start_exe().exists());
}

#[test]
fn test_s11b_stop_service_if_ours_no_side_effects() {
    // 服务未跑 → 早退；在跑但二进制不在（临时）runtime 下 → 不动它。
    // 两种机器状态都无副作用。
    let tmp = tempfile::tempdir().unwrap();
    stop_service_if_ours(tmp.path());
}

#[test]
fn test_s11b_workspace_dir_env_resolution() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    assert_eq!(workspace_dir(false), th.home.join("workspace"));
}

// -------------------------- pending / commit ------------------------------

#[tokio::test]
async fn test_s11b_run_pending_and_commit_paths() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let paths = s11b_paths(&th.home);

    // 空盒 → "No pending workspace files"
    run(SandboxCommand::Pending, false).await.unwrap();
    run(
        SandboxCommand::Commit {
            all: true,
            files: vec![],
        },
        false,
    )
    .await
    .unwrap();

    // 造 2 个盒内文件：1 个映射进工作区子树、1 个在 C:\Windows（被过滤）
    let ws = th.home.join("workspace");
    let in_ws = ws.join("notes").join("a.txt");
    let boxed_in = s11b_box_mirror(&in_ws, &paths.box_root);
    std::fs::create_dir_all(boxed_in.parent().unwrap()).unwrap();
    std::fs::write(&boxed_in, "s11b-a").unwrap();
    let outside = s11b_box_mirror(
        &std::path::PathBuf::from(r"C:\Windows\s11b_evil.dll"),
        &paths.box_root,
    );
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "evil").unwrap();

    // pending：只列工作区子树那 1 个
    run(SandboxCommand::Pending, false).await.unwrap();

    // commit：needle 不匹配 → 无操作
    run(
        SandboxCommand::Commit {
            all: false,
            files: vec!["no-match-xyz".into()],
        },
        false,
    )
    .await
    .unwrap();
    assert!(!in_ws.exists());

    // commit：needle 大小写不敏感（代码双侧 lowercase）→ 命中并落真盘
    run(
        SandboxCommand::Commit {
            all: false,
            files: vec!["NOTES".into()],
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&in_ws).unwrap(), "s11b-a");

    // commit --all：工作区子树全提交（含已提交过的，幂等）
    run(
        SandboxCommand::Commit {
            all: true,
            files: vec![],
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&in_ws).unwrap(), "s11b-a");
    assert!(
        !std::path::PathBuf::from(r"C:\Windows\s11b_evil.dll").exists(),
        "工作区外镜像文件不该被 commit 落到真盘"
    );

    // 清掉盒 → 恢复空态
    std::fs::remove_dir_all(&paths.box_root).unwrap();
    run(SandboxCommand::Pending, false).await.unwrap();
}

// -------------------------------- clear -----------------------------------

#[tokio::test]
async fn test_s11b_run_clear_force_and_missing_startexe() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let paths = s11b_paths(&th.home);

    // 无 pending + 非 force → 跳过 stdin 提问 → delete_box_contents：
    // Start.exe 缺失 → spawn Err → 上抛
    let err = run(SandboxCommand::Clear { force: false }, false).await;
    assert!(err.is_err(), "缺 Start.exe 的 clear 应上抛 Err");

    // 放假 Start.exe（cmd.exe 副本）→ force=true → Box cleared
    assert!(s11b_fake_start_exe(&paths));
    run(SandboxCommand::Clear { force: true }, false)
        .await
        .unwrap();

    // 有 pending + force=true → 跳过提问直接清（不做 commit）
    let ws = th.home.join("workspace");
    let in_ws = ws.join("p.txt");
    let boxed_in = s11b_box_mirror(&in_ws, &paths.box_root);
    std::fs::create_dir_all(boxed_in.parent().unwrap()).unwrap();
    std::fs::write(&boxed_in, "x").unwrap();
    run(SandboxCommand::Clear { force: true }, false)
        .await
        .unwrap();
    assert!(!in_ws.exists(), "force clear 不提交，直接丢");
}

// -------------------------------- kill ------------------------------------

#[tokio::test]
async fn test_s11b_kill_all_branches() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let paths = s11b_paths(&th.home);

    // 1) Start.exe 缺失 → bail
    assert!(
        run(SandboxCommand::Kill { box_name: None }, false)
            .await
            .is_err()
    );

    // 2) 假 Start.exe + ini 只有非 eval 段：
    assert!(s11b_fake_start_exe(&paths));
    let dead_root = th.home.join("dead_box_root");
    let live_root = th.home.join("live_box_root");
    std::fs::create_dir_all(&live_root).unwrap();
    let ini = format!(
        "[GlobalSettings]\n\n[NemesisBox]\nEnabled=y\n\n\
         [NemesisEvalBox_dead]\nFileRootPath={}\n\n\
         [NemesisEvalBox_nofr]\nEnabled=y\n\n\
         [NemesisEvalBox_live]\nFileRootPath={}\n",
        dead_root.display(),
        live_root.display()
    );
    std::fs::create_dir_all(paths.ini_path.parent().unwrap()).unwrap();
    std::fs::write(&paths.ini_path, ini).unwrap();

    // 2a) 指定盒不在 ini → 早退 Ok
    run(
        SandboxCommand::Kill {
            box_name: Some("NemesisEvalBox_missing".into()),
        },
        false,
    )
    .await
    .unwrap();

    // 2b) 全量 kill：dead 段跳过 / 缺 FileRootPath 段跳过 / live 段走
    //     /terminate + delete_sandbox_silent（.bat 均 0 退出）
    run(SandboxCommand::Kill { box_name: None }, false)
        .await
        .unwrap();
    assert!(live_root.exists(), "kill 不删 FileRootPath 本体目录");
}

// ----------------------- run_startexe_timeout 三分支 -----------------------

#[test]
fn test_s11b_run_startexe_timeout_fast_exit_true() {
    let tmp = tempfile::tempdir().unwrap();
    let bat = tmp.path().join("start.bat");
    std::fs::write(&bat, "@exit /b 0\r\n").unwrap();
    assert!(run_startexe_timeout(
        &bat,
        "Box",
        "/terminate",
        std::time::Duration::from_secs(5)
    ));
}

#[test]
fn test_s11b_run_startexe_timeout_spawn_err_false() {
    let tmp = tempfile::tempdir().unwrap();
    let txt = tmp.path().join("not_executable.txt");
    std::fs::write(&txt, "hello").unwrap();
    assert!(!run_startexe_timeout(
        &txt,
        "Box",
        "/terminate",
        std::time::Duration::from_secs(5)
    ));
}

#[test]
fn test_s11b_run_startexe_timeout_hang_tree_killed_false() {
    let tmp = tempfile::tempdir().unwrap();
    let bat = tmp.path().join("hang.bat");
    // ping 30s 模拟无响应；400ms 超时 → taskkill 树杀 → false
    std::fs::write(&bat, "@ping -n 30 127.0.0.1 > nul\r\n").unwrap();
    let t0 = std::time::Instant::now();
    let ok = run_startexe_timeout(
        &bat,
        "Box",
        "/terminate",
        std::time::Duration::from_millis(400),
    );
    assert!(!ok);
    assert!(t0.elapsed() < std::time::Duration::from_secs(5), "超时后必须树杀返回");
}

// =========================================================================
// wave_b（覆盖率补测 2026-08-27）：CLI 编排层剩余可测臂。沿用 S11b 夹具
// （临时 NEMESISBOT_HOME + GLOBAL_STATE_LOCK + 假 Start.exe），零 UAC /
// 零真实引擎触碰：
//   1) commit 失败臂：真落盘路径的父目录被普通文件占位 → commit_file 的
//      create_dir_all 必然失败 → CLI 打印 FAILED 行并继续（sandbox.rs:182）；
//   2) kill 指定盒且该盒确在 ini（vec![b.clone()] 臂，sandbox.rs:281-282）；
//   3) kill 全量但 ini 无任何 NemesisEvalBox_* 段（早退提示臂 :294-295）；
//   4) kill 时 Start.exe 为非法 PE（垃圾字节）→ spawn 秒败 → term_ok=false
//      → 打印「无响应跳过」臂（sandbox.rs:328-331）——全程不产生任何进程；
//   5) status 三 present 分支（Start.exe / Sandboxie.ini / runtime 目录都
//      存在时，sandbox.rs:768/777/786）——status 内部只做存在性判断 +
//      只读 SCM 查询（与既有 test_s11b_run_status_fresh_home 同安全级别）。
// 豁免不变式：Install（真下载）、Start/Stop/EnsureReady（UAC/真引擎，
// 本机 SbieSvc RUNNING 绝不触碰）、ensure_sandbox_ready Row≥gate（可确定
// 到达需伪造服务注册＝动注册表；干净机器上会落入 install/UAC 臂，跨环境
// 不安全）、clear 非 force 交互提问（真 stdin）、ChildJob API 失败注入、
// run_startexe_timeout 的 try_wait Err 臂（句柄有效后 Windows 几乎不会 Err）。
// =========================================================================
mod wave_b {
    use super::*;

    /// box 内镜像一个「real 路径」文件（drive/<L>/… 形态），返回盒内路径。
    fn wb_mirror(real: &std::path::Path, box_root: &std::path::Path) -> std::path::PathBuf {
        s11b_box_mirror(real, box_root)
    }

    #[tokio::test]
    async fn wave_b_commit_failed_line_when_real_parent_is_regular_file() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let paths = s11b_paths(&th.home);

        let ws = th.home.join("workspace");
        // 可正常提交的文件：ws/ok.txt
        let ok_real = ws.join("ok.txt");
        let ok_box = wb_mirror(&ok_real, &paths.box_root);
        std::fs::create_dir_all(ok_box.parent().unwrap()).unwrap();
        std::fs::write(&ok_box, "ok-body").unwrap();

        // 阻断文件：ws/zed 是普通文件 → blocked.txt 的提交需要
        // create_dir_all(ws/zed) → 必失败 → FAILED 行 + 循环继续。
        let blocked_real = ws.join("zed").join("blocked.txt");
        let blocked_box = wb_mirror(&blocked_real, &paths.box_root);
        std::fs::create_dir_all(blocked_box.parent().unwrap()).unwrap();
        std::fs::write(&blocked_box, "doomed").unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("zed"), b"occupies parent slot").unwrap();

        run(
            SandboxCommand::Commit {
                all: true,
                files: vec![],
            },
            false,
        )
        .await
        .expect("commit 单文件失败只打印行，不整体报错");

        // 正常那份落了真盘；被阻断那份没落、占位文件原样保留。
        assert_eq!(std::fs::read_to_string(&ok_real).unwrap(), "ok-body");
        assert!(!blocked_real.exists(), "被普通文件阻断的目标不得写入");
        assert!(ws.join("zed").is_file(), "占位文件保持为文件");
    }

    #[tokio::test]
    async fn wave_b_kill_specific_box_present_in_ini_runs_terminate_chain() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let paths = s11b_paths(&th.home);
        assert!(s11b_fake_start_exe(&paths));

        let root = th.home.join("waveb_root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(paths.ini_path.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.ini_path,
            format!(
                "[GlobalSettings]\nEnabled=y\n\n[WaveBBox]\nFileRootPath={}\n",
                root.display()
            ),
        )
        .unwrap();

        // 指定盒在 ini 且 FileRootPath 存活 → vec![b.clone()] 后走完整
        // /terminate + delete_sandbox_silent（假 Start.exe = cmd.exe 副本，
        // 与 S11b 已实证的同款调用形态，秒退无副作用）。
        run(
            SandboxCommand::Kill {
                box_name: Some("WaveBBox".into()),
            },
            false,
        )
        .await
        .unwrap();

        assert!(root.exists(), "kill 只清盒内容登记，不删 FileRootPath 本体");
    }

    #[tokio::test]
    async fn wave_b_kill_all_with_no_eval_sections_returns_early_hint() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let paths = s11b_paths(&th.home);
        assert!(s11b_fake_start_exe(&paths));
        std::fs::create_dir_all(paths.ini_path.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.ini_path,
            "[GlobalSettings]\nEnabled=y\n\n[NemesisBox]\nEnabled=y\n",
        )
        .unwrap();

        // 无任何 NemesisEvalBox_* 段 → 枚举空 → 早退提示 + Ok。
        run(SandboxCommand::Kill { box_name: None }, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wave_b_kill_with_unspawnable_startexe_reports_no_response_and_skips() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let paths = s11b_paths(&th.home);

        // 「存在但非 PE」的 Start.exe：CreateProcess 直接 BAD_EXE_FORMAT
        // 秒拒 → run_startexe_timeout 进 spawn-Err 即刻返 false（不等待、
        // 不树杀、零进程创建）→ kill 打印 /terminate 无响应臂后照常收尾。
        std::fs::create_dir_all(&paths.runtime_dir).unwrap();
        std::fs::write(paths.start_exe(), b"not a PE image - wave_b marker").unwrap();

        let root = th.home.join("waveb_garbage_root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(paths.ini_path.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.ini_path,
            format!(
                "[GlobalSettings]\n\n[NemesisEvalBox_wb]\nFileRootPath={}\n",
                root.display()
            ),
        )
        .unwrap();

        run(SandboxCommand::Kill { box_name: None }, false)
            .await
            .unwrap();
        assert!(paths.start_exe().is_file(), "垃圾 Start.exe 保持原样");
    }

    #[tokio::test]
    async fn wave_b_status_present_arms_when_runtime_ini_startexe_exist() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let paths = s11b_paths(&th.home);

        // 三件套就位：Start.exe 占位文件（status 只判存在，不执行它）、
        // Sandboxie.ini、runtime 目录本体。
        std::fs::create_dir_all(&paths.runtime_dir).unwrap();
        std::fs::write(paths.start_exe(), b"placeholder, not executed").unwrap();
        std::fs::create_dir_all(paths.ini_path.parent().unwrap()).unwrap();
        std::fs::write(&paths.ini_path, "[GlobalSettings]\nEnabled=y\n").unwrap();

        run(SandboxCommand::Status, false).await.unwrap();

        assert!(paths.start_exe().is_file());
        assert!(paths.ini_path.is_file());
    }
}

use super::spawn_detached;
use std::process::Command;
use std::time::Duration;

/// 成功路径：spawn 即返回 Ok(pid)，子进程自主退出并被收尸线程回收。
/// 平台分歧只在本测试内收敛（Unix `sh -c` / Windows `cmd /C`）。
fn trivial_success_command() -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "exit 0"]);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg("exit 0");
        c
    }
}

#[test]
fn spawn_detached_success_returns_pid() {
    let result = spawn_detached(|| trivial_success_command().spawn());
    let pid = result.expect("trivial spawn must succeed");
    assert!(pid > 0, "pid must be positive, got {pid}");
}

#[test]
fn spawn_detached_spawn_failure_is_honest_error() {
    // 不存在的二进制：Err 必须描述 spawn 失败（子进程未启动），不能 Ok。
    let result = spawn_detached(|| Command::new("definitely-not-a-real-binary-8a7f3c").spawn());
    let err = result.expect_err("nonexistent binary must fail");
    assert!(
        !err.contains("spawn-reap thread"),
        "failure must be from spawn, not the reaper thread: {err}"
    );
}

/// 副作用证明：helper 返回后子进程仍在跑（写 marker 文件），且写成功——
/// 「立即返回不等子进程」+「子进程不因 Child drop 被杀」两个契约一并锁住。
fn assert_marker_appears(marker: &std::path::Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if marker.exists() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "marker file never appeared; child did not run after spawn_detached returned"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
#[test]
fn spawn_detached_process_runs_after_return() {
    // raw_arg：整段命令串原样进 cmd 命令行（`>` 重定向必须让 cmd 自己
    // 解析；逐元素 argv 传递时 std 的引号规则会把 `>` 变普通参数）。
    use std::os::windows::process::CommandExt;
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("marker.txt");
    let marker_str = marker.to_string_lossy().to_string();
    let pid = spawn_detached(move || {
        let mut c = Command::new("cmd");
        c.raw_arg("/C")
            .raw_arg(format!("echo ran> \"{marker_str}\""));
        c.spawn()
    })
    .expect("side-effect spawn must succeed");
    assert!(pid > 0);
    assert_marker_appears(&marker);
}

#[cfg(unix)]
#[test]
fn spawn_detached_process_runs_after_return() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("marker.txt");
    let marker_str = marker.to_string_lossy().to_string();
    let pid = spawn_detached(move || {
        let mut c = Command::new("sh");
        c.arg("-c").arg(format!("echo ran > '{marker_str}'"));
        c.spawn()
    })
    .expect("side-effect spawn must succeed");
    assert!(pid > 0);
    assert_marker_appears(&marker);
}

#[cfg(unix)]
#[test]
fn spawn_detached_reaps_exited_child_no_zombie() {
    // 僵尸断言（Unix 专属、按本进程 pid 精确判定）：子进程快速退出后
    // ps 侧短暂呈现 defunct（Z 状态），收尸线程 wait() 完成后条目消失。
    // 按 pid 轮询不与系统其他僵尸（共享机环境噪声）混淆。
    let pid = spawn_detached(|| {
        let mut c = Command::new("sh");
        c.arg("-c").arg("exit 0");
        c.spawn()
    })
    .expect("spawn must succeed");

    // 轮询直到 `ps -o stat= -p <pid>` 条目消失为止（唯一终态：收尸完成）。
    // 中间态两种都继续等：条目在但非 Z = 子进程尚未退出；条目在且 Z =
    // 已退未收尸。收尸线程 wait() 是条目消失的唯一通路（fork 炸弹级
    // pid 复用在 10s 窗口内视为不可达）。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let out = Command::new("ps")
            .arg("-o")
            .arg("stat=")
            .arg("-p")
            .arg(pid.to_string())
            .output()
            .expect("ps available on unix");
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if stat.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pid {pid} entry persisted for >10s (stat={stat}); zombie not reaped"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

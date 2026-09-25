//! agent wave-5 round-2 补测：REPL 离线可达臂 + 装配侧分支。
//!
//! 结构性边界（本轮裁决）：
//! - build_agent_loop Err 臂（agent.rs 218-226）不可达：run() 在 89 行
//!   已用同一 load_config 闸过（失败走 90 行既有覆盖分支）；工厂侧模型
//!   解析失败是降级装配（NullProvider）不是 Err。不再强行造。
//! - REPL 的 Ok(line) 分支（slash 命令 / process_direct）需要真实交互
//!   输入，rustyline 不吃测试注入的 stdin——结构性豁免。
//! - 本测覆盖：skills_registry 读分支、observer_manager 两臂、REPL 头段
//!   （session mgr / history dir / Editor / load_history）、Eof 退出臂。
//!
//! 进程全局态纪律：GLOBAL_STATE_LOCK 串行 + singleton_test_home() 永久
//! 沙箱（nemesis-path 单例）+ EnvHomeGuard 指向沙箱 parent（用完还原）。
//! config.json 用完即删（沙箱 home 跨测试共享）。
#![cfg(target_os = "windows")]

use super::*;

mod wave5 {
    use super::*;
    use crate::tests::{EnvHomeGuard, singleton_test_home};

    /// REPL 非 TTY 才安全（交互终端下 rustyline 会等真输入）。
    fn skip_if_interactive() -> bool {
        use std::io::IsTerminal as _;
        std::io::stdin().is_terminal()
    }

    fn seed_config(home: &std::path::Path, logging_llm_enabled: bool) {
        let cfg = if logging_llm_enabled {
            r#"{"logging":{"llm":{"enabled":true}}}"#
        } else {
            "{}"
        };
        std::fs::write(home.join("config.json"), cfg).unwrap();
    }

    // GLOBAL_STATE_LOCK 是纯测试串行化锁（产品代码从不锁它，被 await 的 run()
    // 内部也不锁），跨 await 持有不构成死锁；singleton home + env 重定向必须
    // 整个测试期间持有，故对 lint 显式豁免并留档理由。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn w5_repl_eof_exits_cleanly_without_logging_and_observer_none_arm() {
        if skip_if_interactive() {
            return;
        }
        let lock = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        seed_config(&home, false);

        let res = run(
            None,
            None,
            "w5cov-repl-a".to_string(),
            false,
            false,
            false,
            false,
        )
        .await;

        let _ = std::fs::remove_file(home.join("config.json"));
        drop(_env);
        drop(lock);
        res.expect("REPL 非 TTY → 首次 readline Eof → Goodbye → Ok");
    }

    // multi_thread flavor：request-logger 注册路径里有 block_in_place，
    // current-thread runtime（#[tokio::test] 默认）会 panic。
    #[allow(clippy::await_holding_lock)] // 理由同上：测试串行化锁，非死锁面
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w5_repl_with_skills_registry_and_request_logger_loads_history_then_eof() {
        if skip_if_interactive() {
            return;
        }
        let lock = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        seed_config(&home, true);
        // skills registry 配置存在 → 读 + 解析分支（145-154）。
        let skills_cfg = home.join("workspace/config/config.skills.json");
        std::fs::create_dir_all(skills_cfg.parent().unwrap()).unwrap();
        std::fs::write(&skills_cfg, "{}").unwrap(); // 全字段 serde default
        // 预置历史文件 → REPL 的 load_history 分支（260-261）。
        let history = home.join("workspace/logs/agent_history");
        std::fs::create_dir_all(history.parent().unwrap()).unwrap();
        std::fs::write(&history, "").unwrap();

        let res = run(
            None,
            None,
            "w5cov-repl-b".to_string(),
            false,
            false,
            false,
            false,
        )
        .await;

        let _ = std::fs::remove_file(home.join("config.json"));
        let _ = std::fs::remove_file(&skills_cfg);
        drop(_env);
        drop(lock);
        res.expect("REPL 非 TTY → Eof → Ok（装配走 request-logger + registry 分支）");
    }
}

//! MXC 真机冒烟（feature = "mxc"，PLAN M5 验收件）。逐变体隔离 PSEC
//! 策略面失败源：probe / 裸 echo / fs 策略（仅存在路径）/ denied 含不
//! 存在路径。运行：`cargo run -p nemesis-sandbox --features mxc --example
//! mxc_smoke [-- <variant>]`。

#[cfg(feature = "mxc")]
fn main() {
    let variant = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());

    // 1. probe
    if variant == "all" || variant == "probe" {
        match mxc_sdk::v1::probe(None) {
            Ok(out) => println!(
                "[probe] tier={:?} api_present={} error={:?}",
                out.tier, out.probes.base_container_api_present, out.error
            ),
            Err(e) => println!("[probe] ERR {e}"),
        }
    }

    // 2. 裸 echo（无任何策略段）
    if variant == "all" || variant == "echo" {
        let req = mxc_sdk::v1::ContainerRequest::new("cmd /c echo mxc-echo-ok");
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[echo] exit={:?} stdout={:?}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[echo] ERR {e}"),
        }
    }

    // 3. fs 策略（全部真实存在的路径 + locked-ish deny 用存在文件）
    if variant == "all" || variant == "fs" {
        let tmp = std::env::temp_dir().join("mxc-smoke-rw");
        let _ = std::fs::create_dir_all(&tmp);
        let marker = tmp.join("exists.txt");
        std::fs::write(&marker, b"x").expect("write marker");
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![tmp.display().to_string()],
                readonly_paths: vec![std::env::temp_dir().display().to_string()],
                denied_paths: vec![marker.display().to_string()],
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo fs-ok")
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[fs] exit={:?} stdout={:?}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[fs] ERR {e}"),
        }
    }

    // 4. denied 引用不存在路径（复现假设：PSEC 对缺席 denied 路径报
    //    ERROR_PATH_NOT_FOUND）
    if variant == "all" || variant == "denied-missing" {
        let tmp = std::env::temp_dir().join("mxc-smoke-rw");
        let _ = std::fs::create_dir_all(&tmp);
        let missing = tmp.join("definitely-not-exists.yaml");
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![tmp.display().to_string()],
                readonly_paths: Vec::new(),
                denied_paths: vec![missing.display().to_string()],
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo denied-missing-ok")
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[denied-missing] exit={:?} stdout={:?}（未复现——缺席 denied 路径被接受）",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[denied-missing] ERR {e}（复现：缺席 denied 路径 → PSEC 失败）"),
        }
    }

    // ── gateway 请求形态逐项隔离（e2e 抓到的 PSEC 0x80070003）──
    // 完整形态 = gateway spawn_contained 产出的 ContainerRequest 同构；
    // 减法变体逐个摘掉嫌疑面（env / wd / net / ui / timeout / 引号 exe）。
    let exe = r"C:\AI\NemesisBot_Rust_2\target\debug\nemesisbot.exe";
    let ws_dir = r"C:\AI\NemesisBot_Rust_2\target\mxc-e2e-home\.nemesisbot\workspace";
    let home = r"C:\AI\NemesisBot_Rust_2\target\mxc-e2e-home\.nemesisbot";
    let full = |keep_env: bool, keep_wd: bool, keep_net: bool, keep_ui: bool, quote: bool| {
        let cmd = if quote {
            format!("\"{exe}\"")
        } else {
            exe.to_string()
        };
        mxc_sdk::v1::ContainerRequest {
            command: cmd,
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: vec![home.to_string()],
                denied_paths: vec![format!("{ws_dir}\\config\\credentials.yaml")],
                clear_policy_on_exit: None,
            }),
            network: if keep_net {
                Some(mxc_sdk::v1::NetworkPolicy {
                    egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
                        default: Some(mxc_sdk::v1::NetworkAction::Allow),
                        allow: None,
                        deny: None,
                    }),
                    ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                        default: Some(mxc_sdk::v1::NetworkAction::Deny),
                        host_loopback: Some(mxc_sdk::v1::NetworkAction::Deny),
                    }),
                    runtime_config: None,
                })
            } else {
                None
            },
            ui: if keep_ui {
                Some(mxc_sdk::v1::UiPolicy::default())
            } else {
                None
            },
            timeout_ms: Some(86_400_000),
            containment: mxc_sdk::v1::Containment::Process,
            container_name: None,
            working_directory: if keep_wd {
                Some(ws_dir.to_string())
            } else {
                None
            },
            environment: if keep_env {
                Some(vec![
                    ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                    (
                        "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                        ws_dir.to_string(),
                    ),
                ])
            } else {
                None
            },
            inherit_default_environment: if keep_env { Some(true) } else { None },
        }
    };
    let run_req = |label: &str, req: mxc_sdk::v1::ContainerRequest| {
        // nemesisbot.exe 在 executor 角色会等 stdin 协议行——不会退出。
        // 用 spawn + 立即 kill 探「容器创建成功与否」（创建失败在 spawn 阶段
        // 就 Err；创建成功 = PSEC 通路 OK）。stdout 读到任何字节也算证据。
        match mxc_sdk::v1::spawn_with_stdin(
            req,
            mxc_sdk::v1::SpawnOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(mut p) => {
                let _ = p.take_stdin(); // 丢掉（不给协议行，子进程会等到被杀）
                let _ = p.kill();
                println!("[{label}] OK（容器创建成功）");
            }
            Err(e) => println!("[{label}] ERR {e}"),
        }
    };
    if variant == "all" || variant == "full" {
        run_req("full", full(true, true, true, true, true));
    }
    if variant == "all" || variant == "full-noenv" {
        run_req("full-noenv", full(false, true, true, true, true));
    }
    if variant == "all" || variant == "full-nowd" {
        run_req("full-nowd", full(true, false, true, true, true));
    }
    if variant == "all" || variant == "full-nonet" {
        run_req("full-nonet", full(true, true, false, true, true));
    }
    if variant == "all" || variant == "full-noui" {
        run_req("full-noui", full(true, true, true, false, true));
    }
    if variant == "all" || variant == "full-noquote" {
        run_req("full-noquote", full(true, true, true, true, false));
    }
    // 二分：真实路径（ws/home/denied）+ echo 命令（摘掉 exe/timeout）
    if variant == "paths-only" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: vec![home.to_string()],
                denied_paths: vec![format!("{ws_dir}\\config\\credentials.yaml")],
                clear_policy_on_exit: None,
            }),
            timeout_ms: Some(86_400_000),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo paths-ok")
        };
        run_req("paths-only", req);
    }
    // 二分：真实 exe 作命令（无 fs 策略）
    if variant == "exe-cmd" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest::new(format!("\"{exe}\" --version"));
        run_req("exe-cmd", req);
    }
    // 二分：无 timeout（全真实路径 + echo）
    if variant == "paths-notimeout" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: vec![home.to_string()],
                denied_paths: vec![format!("{ws_dir}\\config\\credentials.yaml")],
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo paths-notimeout-ok")
        };
        run_req("paths-notimeout", req);
    }
    // 三分：单面策略
    if variant == "rw-only" {
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: Vec::new(),
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo rw-ok")
        };
        run_req("rw-only", req);
    }
    if variant == "ro-only" {
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: Vec::new(),
                readonly_paths: vec![home.to_string()],
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo ro-ok")
        };
        run_req("ro-only", req);
    }
    if variant == "deny-only" {
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: Vec::new(),
                readonly_paths: Vec::new(),
                denied_paths: vec![format!("{ws_dir}\\config\\credentials.yaml")],
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo deny-ok")
        };
        run_req("deny-only", req);
    }
    // 写实证（审查三轮发现：冒烟从没实写过工作区——嵌套策略 ro 父 [home] +
    // rw 子 [home/workspace] 的优先级语义未验证。write-probe = 生产形状实写；
    // write-probe-noro = 去 RO 父对照组。真盘文件存在 + 内容正确 = 通过；
    // probe 败而 noro 过 = RO 父压制 RW 子，fs 策略需重设计（放弃 home 整根
    // RO，改枚举具体只读路径）。
    if variant == "write-probe" || variant == "write-probe-all" {
        let probe = format!("{ws_dir}\\mxc-write-probe.txt");
        let _ = std::fs::remove_file(&probe);
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: vec![home.to_string()],
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new(format!("cmd /c echo mxc-write-ok> \"{probe}\""))
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => {
                let content = std::fs::read_to_string(&probe)
                    .map(|c| c.trim().to_string())
                    .unwrap_or_else(|e| format!("READ-FAIL({e})"));
                println!(
                    "[write-probe] outcome={:?} real-disk file={:?} content={content}",
                    r.outcome,
                    std::path::Path::new(&probe).exists()
                );
                let _ = std::fs::remove_file(&probe);
            }
            Err(e) => println!("[write-probe] ERR {e}"),
        }
    }
    if variant == "write-probe-noro" || variant == "write-probe-all" {
        let probe = format!("{ws_dir}\\mxc-write-probe-noro.txt");
        let _ = std::fs::remove_file(&probe);
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: Vec::new(),
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            ..mxc_sdk::v1::ContainerRequest::new(format!("cmd /c echo mxc-write-ok> \"{probe}\""))
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => {
                let content = std::fs::read_to_string(&probe)
                    .map(|c| c.trim().to_string())
                    .unwrap_or_else(|e| format!("READ-FAIL({e})"));
                println!(
                    "[write-probe-noro] outcome={:?} real-disk file={:?} content={content}",
                    r.outcome,
                    std::path::Path::new(&probe).exists()
                );
                let _ = std::fs::remove_file(&probe);
            }
            Err(e) => println!("[write-probe-noro] ERR {e}"),
        }
    }
    // 观察态：executor 角色子进程在容器内的完整行为（启动 → 存活检查 →
    // 协议行 → 响应/EOF → 退出码），全量打印诊断。
    if variant == "exec-child" || variant == "all" {
        use std::io::Read as _;
        use std::io::Write as _;
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: vec![home.to_string()],
                // 不带 denied——本变体目的是协议观察（denied 的存在性闸在
                // spawn_contained 里，此处裸构造不经过）。
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            timeout_ms: Some(86_400_000),
            containment: mxc_sdk::v1::Containment::Process,
            working_directory: Some(ws_dir.to_string()),
            environment: Some(vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    ws_dir.to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
            ]),
            inherit_default_environment: Some(true),
            command: format!("\"{exe}\""),
            container_name: None,
            ui: None,
            network: None,
        };
        match mxc_sdk::v1::spawn_with_stdin(
            req,
            mxc_sdk::v1::SpawnOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(mut p) => {
                println!("[exec-child] 容器创建成功 pid={}", p.id());
                let mut stdin = p.take_stdin().expect("stdin");
                let mut stdout = p.take_stdout().expect("stdout");
                let stderr = p.take_stderr();
                // 启动窗口：3 秒后子进程应仍活着（等 stdin 行）。
                std::thread::sleep(std::time::Duration::from_secs(3));
                println!("[exec-child] 启动 3s，未写协议行（子进程应阻塞在 stdin 读）");
                // 写协议行（exec 工具：echo）。
                let line = format!(
                    "{}\n",
                    serde_json::json!({
                        "tool": "exec",
                        "args": "{\"command\":\"echo mxc-child-ok\"}",
                        "context": {"channel":"web","chat_id":"mxc-smoke","user":"smoke","session_key":"mxc-smoke"},
                    })
                );
                println!("[exec-child] 写协议行: {} bytes", line.len());
                stdin.write_all(line.as_bytes()).expect("write line");
                stdin.flush().expect("flush");
                // 不关 stdin（exec_worker 按行分发、EOF 才退）——读响应。
                let mut out = Vec::new();
                let read_result = stdout.read_to_end(&mut out);
                println!(
                    "[exec-child] stdout read={read_result:?} bytes={}",
                    out.len()
                );
                println!(
                    "[exec-child] stdout 前 500 字: {}",
                    String::from_utf8_lossy(&out[..out.len().min(500)])
                );
                if let Some(mut s) = stderr {
                    let mut eb = Vec::new();
                    let _ = s.read_to_end(&mut eb);
                    println!(
                        "[exec-child] stderr {} 字: {}",
                        eb.len(),
                        String::from_utf8_lossy(&eb[..eb.len().min(500)])
                    );
                }
                let w = p.wait();
                println!("[exec-child] wait={w:?}");
            }
            Err(e) => println!("[exec-child] ERR {e}"),
        }
    }
    // env 注入验证：环境条目是否真的进容器（echo 未设变量时输出字面 %VAR%）。
    if variant == "env-check" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest {
            environment: Some(vec![(
                "NEMESISBOT_ROLE".to_string(),
                "executor".to_string(),
            )]),
            inherit_default_environment: Some(true),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo ROLE=[%NEMESISBOT_ROLE%]")
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[env-check] outcome={:?} stdout={:?}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[env-check] ERR {e}"),
        }
    }
    // 对照：不注入环境条目（应输出 ROLE=[]）。
    if variant == "env-baseline" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest::new("cmd /c echo ROLE=[%NEMESISBOT_ROLE%]");
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[env-baseline] outcome={:?} stdout={:?}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[env-baseline] ERR {e}"),
        }
    }
    // executor 代码路径验证：ROLE=executor + 假 workspace → exec_worker 应
    // 早退并在 stderr 留话（证明子进程真的进了 executor 代码路径 + stdio
    // 通）。「无输出 + 快速退出」= 路由失败（ROLE 没生效或更早的短路）。
    if variant == "nb-badws" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest {
            environment: Some(vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    r"C:\nonexistent-xyz-2026".to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
            ]),
            inherit_default_environment: Some(true),
            command: format!("\"{exe}\""),
            ..mxc_sdk::v1::ContainerRequest::new("")
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[nb-badws] outcome={:?} stdout={}B stderr={:?}",
                r.outcome,
                r.stdout.len(),
                String::from_utf8_lossy(&r.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
            Err(e) => println!("[nb-badws] ERR {e}"),
        }
    }
    // exe 位置验证：把 exe 拷进授权路径（workspace）再跑 --version——
    // 「DLL init 失败 = exe 在未授权路径」假设的直接判定。
    if variant == "nb-granted" || variant == "all" {
        let granted_dir = std::path::Path::new(ws_dir).join("mxc-smoke-bin");
        let _ = std::fs::create_dir_all(&granted_dir);
        let granted_exe = granted_dir.join("nemesisbot.exe");
        std::fs::copy(exe, &granted_exe).expect("copy exe");
        let req = mxc_sdk::v1::ContainerRequest {
            filesystem: Some(mxc_sdk::v1::FilesystemPolicy {
                readwrite_paths: vec![ws_dir.to_string()],
                readonly_paths: Vec::new(),
                denied_paths: Vec::new(),
                clear_policy_on_exit: None,
            }),
            timeout_ms: Some(60_000),
            working_directory: Some(granted_dir.display().to_string()),
            ..mxc_sdk::v1::ContainerRequest::new(format!("\"{}\" --version", granted_exe.display()))
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[nb-granted] outcome={:?} stdout={:?}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout)
                    .trim()
                    .chars()
                    .take(200)
                    .collect::<String>()
            ),
            Err(e) => println!("[nb-granted] ERR {e}"),
        }
    }
    // UI 细粒度档验证（官方 launch_diagnostics 的 0xC0000142 处方）：
    // disable=false（放开 Win32k/UI 子系统初始化）+ 剪贴板/输入注入仍禁。
    if variant == "ui-on" || variant == "all" {
        let req = mxc_sdk::v1::ContainerRequest {
            ui: Some(mxc_sdk::v1::UiPolicy {
                disable: false,
                clipboard: mxc_sdk::v1::ClipboardPolicy::None,
                allow_input_injection: false,
            }),
            environment: Some(vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    r"C:\nonexistent-xyz-2026".to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
            ]),
            inherit_default_environment: Some(true),
            command: format!("\"{exe}\""),
            ..mxc_sdk::v1::ContainerRequest::new("")
        };
        match mxc_sdk::v1::run(
            req,
            mxc_sdk::v1::RunOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(r) => println!(
                "[ui-on] outcome={:?} stdout={}B stderr={:?}",
                r.outcome,
                r.stdout.len(),
                String::from_utf8_lossy(&r.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
            Err(e) => println!("[ui-on] ERR {e}"),
        }
    }

    // ═══ 功能面验收矩阵（2026-11-14 全功能面测试轮）═══
    // 生产路径逐面验证「正确使用 + 正确限制」：FS 授权/拒绝/越界、
    // NET open/locked/白名单命中/未命中/入站、TIME 引擎超时/诚实化、
    // UI 剪贴板隔离、PROD spawn_contained 全事务。
    // 独立变体（不挂 `all`——网络面含真实公网 curl，整体数分钟）。
    let ws_config_dir = format!("{ws_dir}\\config");
    let _ = std::fs::create_dir_all(&ws_config_dir);
    let cred = format!("{ws_config_dir}\\credentials.yaml");
    if !std::path::Path::new(&cred).exists() {
        std::fs::write(
            &cred,
            "# mxc 功能测试占位（非真实凭据）\napi_key: dummy-not-a-real-key\n",
        )
        .expect("write dummy credentials.yaml");
    }
    let authj = format!("{ws_config_dir}\\auth.json");
    if !std::path::Path::new(&authj).exists() {
        std::fs::write(&authj, r#"{"dummy": true}"#).expect("write dummy auth.json");
    }
    // 生产 fs 形状（build_container_request 同构：ws 读写 / home 只读 /
    // 秘密定向拒绝两件）。
    let prod_fs = || mxc_sdk::v1::FilesystemPolicy {
        readwrite_paths: vec![ws_dir.to_string()],
        readonly_paths: vec![home.to_string()],
        denied_paths: vec![
            format!("{ws_config_dir}\\credentials.yaml"),
            format!("{ws_config_dir}\\auth.json"),
        ],
        clear_policy_on_exit: None,
    };
    let run_opt = || mxc_sdk::v1::RunOptions {
        telemetry: Some(mxc_sdk::v1::TelemetryConfig {
            enabled: Some(false),
        }),
    };
    let run_fs = |cmd: String,
                  fs: mxc_sdk::v1::FilesystemPolicy|
     -> Result<mxc_sdk::v1::ExecutionResult, String> {
        mxc_sdk::v1::run(
            mxc_sdk::v1::ContainerRequest {
                filesystem: Some(fs),
                ..mxc_sdk::v1::ContainerRequest::new(cmd)
            },
            run_opt(),
        )
        .map_err(|e| e.to_string())
    };
    let stderr_head = |r: &mxc_sdk::v1::ExecutionResult| {
        String::from_utf8_lossy(&r.stderr)
            .trim()
            .chars()
            .take(200)
            .collect::<String>()
    };

    // ── A. 文件系统面 ──
    // A1: 写只读根（home 根下 workspace 外）→ 应拒绝，宿主无文件。
    if variant == "fs-ro-write-denied" {
        let target = format!("{home}\\mxc-ro-denied.txt");
        let _ = std::fs::remove_file(&target);
        match run_fs(format!("cmd /c echo intruder> \"{target}\""), prod_fs()) {
            Ok(r) => {
                let on_disk = std::path::Path::new(&target).exists();
                let pass = !on_disk;
                println!(
                    "[fs-ro-write-denied] {} outcome={:?} 宿主文件在场={on_disk} stdout={}B stderr={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    r.stdout.len(),
                    stderr_head(&r)
                );
                let _ = std::fs::remove_file(&target);
            }
            Err(e) => println!("[fs-ro-write-denied] ERR {e}"),
        }
    }
    // A2: 覆写 denied 秘密 → 应拒绝，宿主内容原样。
    if variant == "fs-secret-write-denied" {
        let before = std::fs::read_to_string(&cred).unwrap_or_default();
        match run_fs(
            format!("cmd /c echo hacked-by-container> \"{cred}\""),
            prod_fs(),
        ) {
            Ok(r) => {
                let after = std::fs::read_to_string(&cred).unwrap_or_default();
                let pass = before == after;
                println!(
                    "[fs-secret-write-denied] {} outcome={:?} 宿主内容完好={pass} stdout={}B stderr={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    r.stdout.len(),
                    stderr_head(&r)
                );
            }
            Err(e) => println!("[fs-secret-write-denied] ERR {e}"),
        }
    }
    // A3: 读只读根 → 应放行（home/config.json 非空回读）。
    if variant == "fs-ro-read-ok" {
        match run_fs(format!("cmd /c type \"{home}\\config.json\""), prod_fs()) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = !out.is_empty();
                println!(
                    "[fs-ro-read-ok] {} outcome={:?} stdout={}B head={}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    out.len(),
                    out.chars().take(120).collect::<String>()
                );
            }
            Err(e) => println!("[fs-ro-read-ok] ERR {e}"),
        }
    }
    // A4: 读 denied 秘密（两件）→ 应全拒（读面压制）。
    if variant == "fs-secret-read-denied" {
        match run_fs(
            format!("cmd /c type \"{cred}\" & type \"{authj}\""),
            prod_fs(),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout);
                let leaked = out.contains("dummy-not-a-real-key") || out.contains("\"dummy\"");
                let pass = !leaked;
                println!(
                    "[fs-secret-read-denied] {} outcome={:?} 秘密泄漏={leaked} stdout={}B stderr={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    r.stdout.len(),
                    stderr_head(&r)
                );
            }
            Err(e) => println!("[fs-secret-read-denied] ERR {e}"),
        }
    }
    // A5: 策略外越界（写 C:\Users\Public + 读 C:\Windows\win.ini）→ 应全拒。
    if variant == "fs-outside-write-denied" {
        let outside = r"C:\Users\Public\mxc-outside-probe.txt";
        let _ = std::fs::remove_file(outside);
        match run_fs(format!("cmd /c echo intruder> \"{outside}\""), prod_fs()) {
            Ok(r) => {
                let on_disk = std::path::Path::new(outside).exists();
                let pass = !on_disk;
                println!(
                    "[fs-outside-write-denied] {} outcome={:?} 宿主文件在场={on_disk} stdout={}B stderr={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    r.stdout.len(),
                    stderr_head(&r)
                );
                let _ = std::fs::remove_file(outside);
            }
            Err(e) => println!("[fs-outside-write-denied] ERR {e}"),
        }
        match run_fs("cmd /c type C:\\Windows\\win.ini".to_string(), prod_fs()) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout);
                let leaked = out.contains("[fonts]");
                let pass = !leaked;
                println!(
                    "[fs-outside-read-denied] {} outcome={:?} win.ini 泄漏={leaked} stdout={}B stderr={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    r.stdout.len(),
                    stderr_head(&r)
                );
            }
            Err(e) => println!("[fs-outside-read-denied] ERR {e}"),
        }
    }
    // A6: extra_root 额外读写根透传 → 容器写 → 宿主读回验证。
    if variant == "fs-extra-root-rw" {
        let extra_root = r"C:\AI\NemesisBot_Rust_2\target\mxc-extra-root-test";
        let _ = std::fs::create_dir_all(extra_root);
        let extra_file = format!("{extra_root}\\mxc-extra-rw.txt");
        let _ = std::fs::remove_file(&extra_file);
        let fs = mxc_sdk::v1::FilesystemPolicy {
            readwrite_paths: vec![ws_dir.to_string(), extra_root.to_string()],
            readonly_paths: vec![home.to_string()],
            denied_paths: Vec::new(),
            clear_policy_on_exit: None,
        };
        match run_fs(format!("cmd /c echo extra-root-ok> \"{extra_file}\""), fs) {
            Ok(r) => {
                let content = std::fs::read_to_string(&extra_file)
                    .map(|c| c.trim().to_string())
                    .unwrap_or_else(|e| format!("READ-FAIL({e})"));
                let pass = content == "extra-root-ok";
                println!(
                    "[fs-extra-root-rw] {} outcome={:?} 宿主读回={content}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome
                );
                let _ = std::fs::remove_file(&extra_file);
            }
            Err(e) => println!("[fs-extra-root-rw] ERR {e}"),
        }
    }

    // ── B. 网络面 ──
    // 宿主侧判定服务器：0.0.0.0:49498 恒回 200（daemon 线程随进程退）。
    static HOST_HTTP_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let ensure_host_http = || {
        if !HOST_HTTP_UP.swap(true, std::sync::atomic::Ordering::SeqCst) {
            std::thread::spawn(|| {
                let Ok(listener) = std::net::TcpListener::bind("0.0.0.0:49498") else {
                    return;
                };
                for stream in listener.incoming() {
                    let Ok(mut s) = stream else { continue };
                    let mut buf = [0u8; 4096];
                    let _ = std::io::Read::read(&mut s, &mut buf);
                    let _ = std::io::Write::write_all(
                        &mut s,
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    };
    let host_lan = "192.168.1.104";
    // locked 档策略构造（build_container_request locked 臂同构）。
    let locked_policy = |allow: &[&str]| -> mxc_sdk::v1::NetworkPolicy {
        let rules: Vec<mxc_sdk::v1::NetworkRulePolicy> = allow
            .iter()
            .map(|e| {
                let (cidr, port) = match e.rsplit_once(':') {
                    Some((c, p))
                        if !p.starts_with('+') && p.parse::<u16>().is_ok_and(|v| v >= 1) =>
                    {
                        (c.to_string(), Some(p.parse::<u16>().expect("已判数字")))
                    }
                    _ => (e.to_string(), None),
                };
                mxc_sdk::v1::NetworkRulePolicy {
                    to: Some(vec![mxc_sdk::v1::NetworkPeerPolicy { cidr, except: None }]),
                    ports: Some(vec![mxc_sdk::v1::NetworkPortPolicy {
                        protocol: Some(mxc_sdk::v1::NetworkProtocol::Tcp),
                        port,
                        end_port: None,
                    }]),
                }
            })
            .collect();
        mxc_sdk::v1::NetworkPolicy {
            egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Deny),
                allow: Some(rules),
                deny: None,
            }),
            ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Deny),
                host_loopback: Some(mxc_sdk::v1::NetworkAction::Deny),
            }),
            runtime_config: None,
        }
    };
    let net_run = |cmd: String,
                   net: mxc_sdk::v1::NetworkPolicy|
     -> Result<mxc_sdk::v1::ExecutionResult, String> {
        mxc_sdk::v1::run(
            mxc_sdk::v1::ContainerRequest {
                network: Some(net),
                filesystem: Some(prod_fs()),
                ..mxc_sdk::v1::ContainerRequest::new(cmd)
            },
            run_opt(),
        )
        .map_err(|e| e.to_string())
    };
    // 容器内 curl（System32 curl.exe 继承 PATH），-w 打 HTTP 码。
    let curl_cmd = |url: &str| {
        format!(
            "cmd /c curl -s -o NUL -w \"%{{http_code}}\" --connect-timeout 5 --max-time 10 {url}"
        )
    };
    // open 档策略（build_container_request open 臂同构：egress 全放行 +
    // ingress 恒拒）。
    let open_policy = || mxc_sdk::v1::NetworkPolicy {
        egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
            default: Some(mxc_sdk::v1::NetworkAction::Allow),
            allow: None,
            deny: None,
        }),
        ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
            default: Some(mxc_sdk::v1::NetworkAction::Deny),
            host_loopback: Some(mxc_sdk::v1::NetworkAction::Deny),
        }),
        runtime_config: None,
    };
    // B1: open 档 → egress 全放行（loopback / LAN / 公网）。
    if variant == "net-open-ok" {
        ensure_host_http();
        for (label, url) in [
            ("loopback", "http://127.0.0.1:49498/"),
            ("lan", &format!("http://{host_lan}:49498/")),
            ("公网", "http://www.baidu.com/"),
        ] {
            match net_run(curl_cmd(url), open_policy()) {
                Ok(r) => {
                    let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                    let pass = match label {
                        "公网" => !out.is_empty() && out != "000",
                        _ => out == "200",
                    };
                    println!(
                        "[net-open-{label}] {} outcome={:?} http-code={out}",
                        if pass { "PASS" } else { "FAIL" },
                        r.outcome
                    );
                }
                Err(e) => println!("[net-open-{label}] ERR {e}"),
            }
        }
    }
    // B2: locked 空白名单 → 全拒（loopback / 公网）。
    if variant == "net-locked-deny" {
        ensure_host_http();
        for (label, url) in [
            ("loopback", "http://127.0.0.1:49498/"),
            ("公网", "http://www.baidu.com/"),
        ] {
            match net_run(curl_cmd(url), locked_policy(&[])) {
                Ok(r) => {
                    let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                    let blocked = out != "200";
                    println!(
                        "[net-locked-deny-{label}] {} outcome={:?} http-code={out} stderr={}",
                        if blocked { "PASS" } else { "FAIL" },
                        r.outcome,
                        stderr_head(&r)
                    );
                }
                Err(e) => println!("[net-locked-deny-{label}] ERR {e}"),
            }
        }
    }
    // B3: locked 白名单命中（cidr:port 精确）→ 放行。
    if variant == "net-locked-allow-loopback" {
        ensure_host_http();
        match net_run(
            curl_cmd("http://127.0.0.1:49498/"),
            locked_policy(&["127.0.0.0/8:49498"]),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = out == "200";
                println!(
                    "[net-locked-allow-loopback] {} outcome={:?} http-code={out} stderr={}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    stderr_head(&r)
                );
            }
            Err(e) => println!("[net-locked-allow-loopback] ERR {e}"),
        }
    }
    if variant == "net-locked-allow-lan" {
        ensure_host_http();
        match net_run(
            curl_cmd(&format!("http://{host_lan}:49498/")),
            locked_policy(&[&format!("{host_lan}/32:49498")]),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = out == "200";
                println!(
                    "[net-locked-allow-lan] {} outcome={:?} http-code={out} stderr={}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    stderr_head(&r)
                );
            }
            Err(e) => println!("[net-locked-allow-lan] ERR {e}"),
        }
    }
    // B4: locked 白名单未命中 → 默认拒（allow 不含目标网段/端口）。
    if variant == "net-locked-allow-miss" {
        ensure_host_http();
        for (label, url) in [
            ("loopback", "http://127.0.0.1:49498/"),
            ("lan", &format!("http://{host_lan}:49498/")),
        ] {
            match net_run(curl_cmd(url), locked_policy(&["10.0.0.0/8:443"])) {
                Ok(r) => {
                    let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                    let blocked = out != "200";
                    println!(
                        "[net-locked-allow-miss-{label}] {} outcome={:?} http-code={out}",
                        if blocked { "PASS" } else { "FAIL" },
                        r.outcome
                    );
                }
                Err(e) => println!("[net-locked-allow-miss-{label}] ERR {e}"),
            }
        }
    }
    // B7: egress 白名单远程精确性（B3/B4 目标撞宿主 ingress Deny 测不出
    // egress 白名单本身——换远程目标：cidr 命中放行 / 换 IP 拒 / 换端口拒）。
    if variant == "net-locked-remote-allow" {
        // a) cidr:port 精确命中 → 放行（阿里 DNS HTTP 服务恒回 404）
        match net_run(
            curl_cmd("http://223.5.5.5/"),
            locked_policy(&["223.5.5.5/32:80"]),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = out == "404";
                println!(
                    "[net-locked-remote-hit] {} outcome={:?} http-code={out}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome
                );
            }
            Err(e) => println!("[net-locked-remote-hit] ERR {e}"),
        }
        // b) 白名单外 IP → 拒
        match net_run(
            curl_cmd("http://119.29.29.29/"),
            locked_policy(&["223.5.5.5/32:80"]),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = out == "000";
                println!(
                    "[net-locked-remote-miss-ip] {} outcome={:?} http-code={out}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome
                );
            }
            Err(e) => println!("[net-locked-remote-miss-ip] ERR {e}"),
        }
        // c) 同 IP 错端口 → 拒（端口精确性）
        match net_run(
            curl_cmd("http://223.5.5.5/"),
            locked_policy(&["223.5.5.5/32:443"]),
        ) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout).trim().to_string();
                let pass = out == "000";
                println!(
                    "[net-locked-remote-miss-port] {} outcome={:?} http-code={out}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome
                );
            }
            Err(e) => println!("[net-locked-remote-miss-port] ERR {e}"),
        }
    }

    // B6: loopback 语义诊断（B1 实录：open 档容器 curl 宿主 loopback/LAN
    // 全 exit 7、公网 200 → 容器网络舱独立 loopback 假设）。三臂：
    // a) 容器内 ipconfig（容器网络视图）；b) ingress.host_loopback=Allow
    // 再试宿主 loopback；c) ingress 全 Allow 再试 loopback+LAN。
    if variant == "net-loopback-diag" {
        ensure_host_http();
        // a) 容器网络视图
        match net_run("cmd /c ipconfig /all".to_string(), open_policy()) {
            Ok(r) => {
                let out = String::from_utf8_lossy(&r.stdout);
                println!("[net-loopback-diag-a] 容器内 ipconfig（IPv4 行）：");
                for line in out
                    .lines()
                    .filter(|l| l.contains("IPv4") || l.contains("127."))
                {
                    println!("    {line}");
                }
            }
            Err(e) => println!("[net-loopback-diag-a] ERR {e}"),
        }
        // b) host_loopback Allow
        let hl_allow = mxc_sdk::v1::NetworkPolicy {
            ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Deny),
                host_loopback: Some(mxc_sdk::v1::NetworkAction::Allow),
            }),
            ..open_policy()
        };
        match net_run(curl_cmd("http://127.0.0.1:49498/"), hl_allow) {
            Ok(r) => println!(
                "[net-loopback-diag-b host_loopback=Allow] outcome={:?} http-code={}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout).trim()
            ),
            Err(e) => println!("[net-loopback-diag-b] ERR {e}"),
        }
        // c) ingress 全 Allow
        let ing_allow = mxc_sdk::v1::NetworkPolicy {
            ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Allow),
                host_loopback: Some(mxc_sdk::v1::NetworkAction::Allow),
            }),
            ..open_policy()
        };
        for (label, url) in [
            ("loopback", "http://127.0.0.1:49498/"),
            ("lan", &format!("http://{host_lan}:49498/")),
        ] {
            match net_run(curl_cmd(url), ing_allow.clone()) {
                Ok(r) => println!(
                    "[net-loopback-diag-c ingress=Allow {label}] outcome={:?} http-code={}",
                    r.outcome,
                    String::from_utf8_lossy(&r.stdout).trim()
                ),
                Err(e) => println!("[net-loopback-diag-c {label}] ERR {e}"),
            }
        }
        // d) 容器内自连自身 loopback（对照：舱内 loopback 是否存在监听面）
        match net_run("cmd /c ping -n 1 127.0.0.1".to_string(), open_policy()) {
            Ok(r) => println!(
                "[net-loopback-diag-d ping-loopback] outcome={:?} stdout-head={}",
                r.outcome,
                String::from_utf8_lossy(&r.stdout)
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Err(e) => println!("[net-loopback-diag-d] ERR {e}"),
        }
    }

    // B5: 入站恒拒 → 容器内起监听，宿主连不上。**控制臂先行**（ingress
    // Allow → 宿主连得上 = 监听真的起来了；否则 Deny 臂的拒连是假阳性——
    // powershell 在 ui=None 默认策略下死 0xC0000142，首跑实录）。两臂都挂
    // 生产 UI 细粒度档（监听器起得来是前提）。
    if variant == "net-ingress-deny" {
        let ps_listen = r#"powershell -NoProfile -Command "$l=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any,49497);$l.Start();$c=$l.AcceptTcpClient();$c.Close();$l.Stop()""#;
        let prod_ui = || {
            Some(mxc_sdk::v1::UiPolicy {
                disable: false,
                clipboard: mxc_sdk::v1::ClipboardPolicy::None,
                allow_input_injection: false,
            })
        };
        let ingress_arm = |allow: bool| -> mxc_sdk::v1::NetworkPolicy {
            let action = if allow {
                mxc_sdk::v1::NetworkAction::Allow
            } else {
                mxc_sdk::v1::NetworkAction::Deny
            };
            mxc_sdk::v1::NetworkPolicy {
                egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
                    default: Some(mxc_sdk::v1::NetworkAction::Deny),
                    allow: None,
                    deny: None,
                }),
                ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                    default: Some(action),
                    host_loopback: Some(action),
                }),
                runtime_config: None,
            }
        };
        let run_arm = |allow: bool| -> bool {
            let req = mxc_sdk::v1::ContainerRequest {
                network: Some(ingress_arm(allow)),
                filesystem: Some(prod_fs()),
                ui: prod_ui(),
                ..mxc_sdk::v1::ContainerRequest::new(ps_listen)
            };
            let Ok(mut p) = mxc_sdk::v1::spawn_with_stdin(
                req,
                mxc_sdk::v1::SpawnOptions {
                    telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                        enabled: Some(false),
                    }),
                },
            ) else {
                println!("[net-ingress-deny] spawn ERR（{allow} 臂）");
                return false;
            };
            let _ = p.take_stdin();
            std::thread::sleep(std::time::Duration::from_secs(6));
            let host_probe = std::net::TcpStream::connect_timeout(
                &"127.0.0.1:49497".parse().expect("addr"),
                std::time::Duration::from_secs(3),
            );
            let connected = host_probe.is_ok();
            let _ = p.kill();
            connected
        };
        let control = run_arm(true);
        let denied = !run_arm(false);
        let pass = control && denied;
        println!(
            "[net-ingress-deny] {} 控制臂(Allow)宿主连上={control} 拒绝臂(Deny)宿主被拒={denied}",
            if pass { "PASS" } else { "FAIL" }
        );
    }

    // ── C. 超时面 ──
    // C1: 引擎超时（timeout_ms=3000 + ~20s stall 命令 + run() 即时进
    // wait）→ TimedOut。stall 用 curl 连 223.5.5.5:81（同机 DNS 服务的
    // 81 端口 SYN 被 DROP——宿主实测 20.08s stall；TEST-NET-3 203.0.113.1
    // 在容器内秒回 exit 7 不可用）。容器内 ICMP 被 WFP 拦（ping 秒回失败）、
    // powershell 在 ui=None 下死 0xC0000142。
    if variant == "timeout-incontainer" {
        let req = mxc_sdk::v1::ContainerRequest {
            timeout_ms: Some(3_000),
            ui: Some(mxc_sdk::v1::UiPolicy {
                disable: false,
                clipboard: mxc_sdk::v1::ClipboardPolicy::None,
                allow_input_injection: false,
            }),
            filesystem: Some(prod_fs()),
            network: Some(open_policy()),
            ..mxc_sdk::v1::ContainerRequest::new(
                "cmd /c curl -s --connect-timeout 20 --max-time 25 http://223.5.5.5:81/",
            )
        };
        let t0 = std::time::Instant::now();
        match mxc_sdk::v1::run(req, run_opt()) {
            Ok(r) => {
                let pass = r.outcome == mxc_sdk::v1::WaitResult::TimedOut;
                println!(
                    "[timeout-incontainer] {} outcome={:?} 墙钟={:?}",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome,
                    t0.elapsed()
                );
            }
            Err(e) => println!("[timeout-incontainer] ERR {e}"),
        }
    }
    // C2: 生产路径诚实化——spawn_contained + 引擎超时。**已实证的结构性
    // 事实**（2026-11-14 首跑）：spawn_contained 是 read-first 顺序（先
    // read_line 等 stdout 再 proc.wait()）——引擎 wait deadline 从进入
    // wait 才起算，子进程跑满自时长后正常退出 → wait 到场时进程已退，
    // TimedOut 不触发。判定为**行为记录**：Ok（跑满）= 实证引擎超时对已
    // 启动长命令不强制（生产兜底 = spawn_and_call_mxc 的 tokio 客户端超
    // 时，remote_executor_tool.rs:755）；Err 含「事务超时」= 引擎真强制。
    if variant == "timeout-honest" {
        let spec = nemesis_sandbox::mxc_backend::MxcSpawnSpec {
            exe: std::path::PathBuf::from(exe),
            env_pairs: vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    ws_dir.to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
            ],
            working_dir: std::path::PathBuf::from(ws_dir),
            home: std::path::PathBuf::from(home),
            extra_roots: Vec::new(),
            request_line: format!(
                "{}\n",
                serde_json::json!({
                    "tool": "exec",
                    "args": "{\"command\":\"curl -s --connect-timeout 20 --max-time 25 http://223.5.5.5:81/\"}",
                    "context": {"channel":"web","chat_id":"mxc-smoke","user":"smoke","session_key":"mxc-smoke"},
                })
            ),
            timeout: std::time::Duration::from_secs(3),
        };
        let t0 = std::time::Instant::now();
        match nemesis_sandbox::mxc_backend::spawn_contained(spec, "open", &[]) {
            Ok(out) => println!(
                "[timeout-honest] 实证（spec 超时 3s）墙钟={:?} exit={:?} response-head={} → 引擎超时未强制（read-first），生产依赖客户端兜底",
                t0.elapsed(),
                out.exit_code,
                out.response
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect::<String>()
            ),
            Err(e) => {
                let enforced = e.contains("事务超时");
                println!(
                    "[timeout-honest] 引擎超时{} 墙钟={:?} error={e}",
                    if enforced {
                        "真强制 ✓"
                    } else {
                        "触发但文案异常"
                    },
                    t0.elapsed()
                );
            }
        }
    }
    // C3: sdk wait deadline 直接验证——spawn executor 后**不写协议行**
    // （executor 阻塞在 stdin 读，进程活着），wait() 在 deadline 前进入 →
    // 3s 后应 TimedOut 且树被杀。这是 spawn_contained TimedOut 诚实化映
    // 射（审查二轮 #1）所依赖的引擎行为。
    if variant == "timeout-wait-deadline" {
        let req = mxc_sdk::v1::ContainerRequest {
            timeout_ms: Some(3_000),
            ui: Some(mxc_sdk::v1::UiPolicy {
                disable: false,
                clipboard: mxc_sdk::v1::ClipboardPolicy::None,
                allow_input_injection: false,
            }),
            filesystem: Some(prod_fs()),
            environment: Some(vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    ws_dir.to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
            ]),
            inherit_default_environment: Some(true),
            containment: mxc_sdk::v1::Containment::Process,
            working_directory: Some(ws_dir.to_string()),
            command: format!("\"{exe}\""),
            container_name: None,
            network: Some(open_policy()),
        };
        let t0 = std::time::Instant::now();
        match mxc_sdk::v1::spawn_with_stdin(
            req,
            mxc_sdk::v1::SpawnOptions {
                telemetry: Some(mxc_sdk::v1::TelemetryConfig {
                    enabled: Some(false),
                }),
            },
        ) {
            Ok(mut p) => {
                // 持有 stdin 不写协议行——executor 阻塞在 stdin 读。**不可
                // drop**：drop stdin Box = 关管道 = executor 读到 EOF 干净
                // 退出（首跑实录 Exited(0)@197ms 假象）。
                let mut _stdin = p.take_stdin().expect("stdin");
                let w = p.wait();
                drop(_stdin);
                println!(
                    "[timeout-wait-deadline] {} wait={w:?} 墙钟={:?}",
                    if matches!(w, Ok(mxc_sdk::v1::WaitResult::TimedOut)) {
                        "PASS"
                    } else {
                        "FAIL"
                    },
                    t0.elapsed()
                );
            }
            Err(e) => println!("[timeout-wait-deadline] ERR {e}"),
        }
    }

    // ── D. UI 面 ──
    // D1: 剪贴板隔离（宿主备份 → 设值 → 容器 clip 写 → 宿主应不变 → 恢复）。
    if variant == "clipboard-isolated" {
        let ps = |script: &str| -> String {
            let out = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", script])
                .output()
                .expect("powershell");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let original = ps("Get-Clipboard");
        ps("Set-Clipboard -Value 'mxc-host-clip-original'");
        let req = mxc_sdk::v1::ContainerRequest {
            ui: Some(mxc_sdk::v1::UiPolicy {
                disable: false,
                clipboard: mxc_sdk::v1::ClipboardPolicy::None,
                allow_input_injection: false,
            }),
            ..mxc_sdk::v1::ContainerRequest::new("cmd /c echo mxc-clip-intruder| clip")
        };
        match mxc_sdk::v1::run(req, run_opt()) {
            Ok(r) => {
                let after = ps("Get-Clipboard");
                let pass = after == "mxc-host-clip-original";
                println!(
                    "[clipboard-isolated] {} outcome={:?} 宿主剪贴板此刻={after:?}（原值=mxc-host-clip-original）",
                    if pass { "PASS" } else { "FAIL" },
                    r.outcome
                );
            }
            Err(e) => println!("[clipboard-isolated] ERR {e}"),
        }
        let restored = original.replace('\'', "''");
        ps(&format!("Set-Clipboard -Value '{restored}'"));
    }

    // ── E. 生产事务面（spawn_contained 全路径）──
    // E1: 全链 exec echo（env 含 NEMESISBOT_SANDBOX_BACKEND=mxc 台账标签）。
    if variant == "spawn-contained-e2e" {
        let spec = nemesis_sandbox::mxc_backend::MxcSpawnSpec {
            exe: std::path::PathBuf::from(exe),
            env_pairs: vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    ws_dir.to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
                ("NEMESISBOT_SANDBOX_BACKEND".to_string(), "mxc".to_string()),
            ],
            working_dir: std::path::PathBuf::from(ws_dir),
            home: std::path::PathBuf::from(home),
            extra_roots: Vec::new(),
            request_line: format!(
                "{}\n",
                serde_json::json!({
                    "tool": "exec",
                    "args": "{\"command\":\"echo mxc-prod-e2e-ok\"}",
                    "context": {"channel":"web","chat_id":"mxc-smoke","user":"smoke","session_key":"mxc-smoke"},
                })
            ),
            timeout: std::time::Duration::from_secs(60),
        };
        match nemesis_sandbox::mxc_backend::spawn_contained(spec, "open", &[]) {
            Ok(out) => {
                let resp = out.response.clone().unwrap_or_default();
                let pass = resp.contains("mxc-prod-e2e-ok") && out.exit_code == Some(0);
                println!(
                    "[spawn-contained-e2e] {} exit={:?} response={} stderr-tail={}",
                    if pass { "PASS" } else { "FAIL" },
                    out.exit_code,
                    resp,
                    out.stderr_tail.chars().take(200).collect::<String>()
                );
            }
            Err(e) => println!("[spawn-contained-e2e] ERR {e}"),
        }
    }
    // E2: 全链秘密读拒绝（exec type denied 文件 → 容器内失败语义回传）。
    if variant == "spawn-contained-secret-probe" {
        let spec = nemesis_sandbox::mxc_backend::MxcSpawnSpec {
            exe: std::path::PathBuf::from(exe),
            env_pairs: vec![
                ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                    ws_dir.to_string(),
                ),
                ("NEMESISBOT_EXECUTOR_HOME".to_string(), home.to_string()),
                ("NEMESISBOT_SANDBOX_BACKEND".to_string(), "mxc".to_string()),
            ],
            working_dir: std::path::PathBuf::from(ws_dir),
            home: std::path::PathBuf::from(home),
            extra_roots: Vec::new(),
            request_line: format!(
                "{}\n",
                serde_json::json!({
                    "tool": "exec",
                    "args": "{\"command\":\"type C:\\\\AI\\\\NemesisBot_Rust_2\\\\target\\\\mxc-e2e-home\\\\.nemesisbot\\\\workspace\\\\config\\\\credentials.yaml\"}",
                    "context": {"channel":"web","chat_id":"mxc-smoke","user":"smoke","session_key":"mxc-smoke"},
                })
            ),
            timeout: std::time::Duration::from_secs(60),
        };
        match nemesis_sandbox::mxc_backend::spawn_contained(spec, "open", &[]) {
            Ok(out) => {
                let resp = out.response.clone().unwrap_or_default();
                let leaked = resp.contains("dummy-not-a-real-key");
                let pass = !leaked;
                println!(
                    "[spawn-contained-secret-probe] {} exit={:?} 秘密泄漏={leaked} response-head={}",
                    if pass { "PASS" } else { "FAIL" },
                    out.exit_code,
                    resp.chars().take(400).collect::<String>()
                );
            }
            Err(e) => println!("[spawn-contained-secret-probe] ERR {e}"),
        }
    }
}

#[cfg(not(feature = "mxc"))]
fn main() {
    eprintln!(
        "本 example 需要 mxc feature：cargo run -p nemesis-sandbox --features mxc --example mxc_smoke"
    );
    std::process::exit(1);
}

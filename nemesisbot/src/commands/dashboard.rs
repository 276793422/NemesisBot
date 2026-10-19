use std::path::Path;
use std::time::Duration;

pub async fn run(local: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = crate::common::resolve_home(local);

    // 1. Read auth_token from config.json
    let config_path = home.join("config.json");
    let cfg_str = std::fs::read_to_string(&config_path).map_err(|e| {
        format!(
            "Cannot read config.json at {}: {}.\n\
             If the gateway was started with --local, run: nemesisbot --local dashboard",
            config_path.display(),
            e
        )
    })?;
    let cfg: serde_json::Value = serde_json::from_str(&cfg_str)?;
    let auth_token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // 2. Read gateway state
    let state_path = nemesis_path::resolve_gateway_state_path_in_workspace(
        &crate::common::workspace_path(&home),
    );
    let gateway_info = read_gateway_state(&state_path);

    // 3. Check if gateway is running; start if not
    let (web_host, web_port) = match gateway_info {
        Some(ref info) if info.web_port > 0 => {
            let base_url = format!("http://{}:{}", info.web_host, info.web_port);
            if check_health(&base_url).await.is_ok() {
                (info.web_host.clone(), info.web_port)
            } else {
                start_and_wait(local, &state_path).await?
            }
        }
        _ => start_and_wait(local, &state_path).await?,
    };

    // 4. Send open_dashboard command
    let base_url = format!("http://{}:{}", web_host, web_port);
    send_internal_command(&base_url, &auth_token, "open_dashboard").await?;

    println!("  OK Dashboard opening...");
    Ok(())
}

pub(crate) struct GatewayInfo {
    #[allow(dead_code)]
    pub(crate) pid: u32,
    pub(crate) web_host: String,
    pub(crate) web_port: i64,
}

pub(crate) fn read_gateway_state(path: &Path) -> Option<GatewayInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    Some(GatewayInfo {
        pid: v["pid"].as_u64()? as u32,
        web_host: v["web_host"].as_str()?.to_string(),
        web_port: v["web_port"].as_i64()?,
    })
}

pub(crate) async fn check_health(base_url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/api/health", base_url);
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("health check returned {}", resp.status()))
    }
}

async fn start_and_wait(
    local: bool,
    state_path: &Path,
) -> Result<(String, i64), Box<dyn std::error::Error>> {
    println!("  Starting gateway...");
    let exe = std::env::current_exe()?;

    let mut cmd = std::process::Command::new(&exe);
    if cfg!(test) {
        // BUG #48（2026-08-28）：测试二进制的 main 是 libtest，CLI 参数会被
        // 当成测试过滤器——`["gateway"]` 让直调单测 spawn 出一个嵌套重跑全部
        // commands::gateway 测试（217 个）的 rogue 子进程：双倍 live 负载 +
        // 幻影 result 行污染全量闸门（曾致随机 live 测试 60s 窗超时失败）。
        // 测试形态改传 libtest 空过滤器（--exact 不存在的名字）→ 子进程
        // 0 测试秒退、stdio 归零、零副作用。生产二进制走 else 分支，
        // --local/gateway argv 字节不变（cfg!(test) 编译期常量）。
        cmd.arg("--exact").arg("__dashboard_start_and_wait_stub__");
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    } else {
        if local {
            cmd.arg("--local");
        }
        cmd.arg("gateway");
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // A4 装配纪律：保留子进程句柄，等待循环经 nemesis_utils::wait_ready
    // 耦合存活断言——gateway 半路崩（坏配置/端口被占/AV 拦截）立即报退出码
    // 现场，不再傻等满 30s 只给一句笼统超时。句柄 drop = 脱管继续跑（std
    // Child drop 不杀进程），与旧行为一致：就绪后的 gateway 生命周期独立。
    let mut child = cmd.spawn()?;

    let mut poll = || async {
        match read_gateway_state(state_path) {
            Some(info) if info.web_port > 0 => {
                let base_url = format!("http://{}:{}", info.web_host, info.web_port);
                if check_health(&base_url).await.is_ok() {
                    Some((info.web_host, info.web_port))
                } else {
                    None
                }
            }
            _ => None,
        }
    };
    // 存活断言是同步体（std Child）：闭包体内算完结果，用 ready future 携带
    // 所有权值返回——async 块直接借闭包捕获变量会因 future 逃逸挂掉 FnMut。
    let mut aliveness = || {
        let death = match child.try_wait() {
            Ok(Some(status)) => Some(format!(
                "exit code: {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| status.to_string())
            )),
            Ok(None) => None,
            Err(e) => Some(format!("status check failed: {e}")),
        };
        std::future::ready(death)
    };
    match nemesis_utils::wait_ready::wait_for_ready(
        Duration::from_millis(500),
        Duration::from_secs(30),
        &mut poll,
        &mut aliveness,
    )
    .await
    {
        Ok((host, port)) => {
            println!("  Gateway started (port {port})");
            Ok((host, port))
        }
        Err(nemesis_utils::wait_ready::ReadyError::ChildDied { detail }) => {
            Err(format!("Gateway process exited during startup ({detail})").into())
        }
        Err(nemesis_utils::wait_ready::ReadyError::Timeout) => {
            Err("Gateway did not start within 30 seconds".into())
        }
    }
}

async fn send_internal_command(
    base_url: &str,
    auth_token: &str,
    cmd: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!("{}/api/internal", base_url);
    let resp = client
        .post(&url)
        .header("X-Auth-Token", auth_token)
        .json(&serde_json::json!({ "cmd": cmd }))
        .send()
        .await?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Internal command failed: {} {}", status, body).into())
    }
}

/// Like `send_internal_command` but returns the parsed JSON response body.
/// Used by `estop status` (and engage/release confirm the resulting state).
pub(crate) async fn send_internal_command_get_json(
    base_url: &str,
    auth_token: &str,
    cmd: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!("{}/api/internal", base_url);
    let resp = client
        .post(&url)
        .header("X-Auth-Token", auth_token)
        .json(&serde_json::json!({ "cmd": cmd }))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({})))
    } else {
        Err(format!("Internal command failed: {} {}", status, body).into())
    }
}

#[cfg(test)]
mod tests;

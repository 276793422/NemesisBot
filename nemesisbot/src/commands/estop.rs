//! `nemesisbot estop` — 全局急停开关 CLI。
//!
//! 跨进程触发：复用 `/api/internal` 通道（和 `dashboard` 命令同一条），把
//! `estop_engage` / `estop_release` / `estop_status` 命令 POST 给正在跑的
//! gateway。gateway 的 web handler 直接操作 `AppState.estop`（同一个 Arc，
//! agent loop 也读它），所以无需 mpsc 往返、status 也能即时返回。

pub async fn run(
    home: &std::path::Path,
    release: bool,
    status: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. 读 auth_token（/api/internal 要 X-Auth-Token，否则 401）
    let config_path = home.join("config.json");
    let cfg_str = std::fs::read_to_string(&config_path).map_err(|e| {
        format!(
            "Cannot read config.json at {}: {}.\n\
             If the gateway was started with --local, run: nemesisbot --local estop",
            config_path.display(),
            e
        )
    })?;
    let cfg: serde_json::Value = serde_json::from_str(&cfg_str)?;
    let auth_token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // 2. 读 gateway state 拿 web_host/web_port
    let state_path = home
        .join("workspace")
        .join("state")
        .join("gateway.json");
    let info = crate::commands::dashboard::read_gateway_state(&state_path)
        .ok_or_else(|| -> Box<dyn std::error::Error> {
            "Gateway 未运行（找不到 state 文件）。先用 `nemesisbot gateway` 启动它。".into()
        })?;
    if info.web_port <= 0 {
        return Err("Gateway state 无效（web_port=0）。".into());
    }
    let base_url = format!("http://{}:{}", info.web_host, info.web_port);

    // 3. 健康检查
    if crate::commands::dashboard::check_health(&base_url)
        .await
        .is_err()
    {
        return Err(format!("Gateway 在 {} 不可达。确认它正在跑。", base_url).into());
    }

    // 4. 派发命令（互斥优先级：status > release > engage）
    let cmd = if status {
        "estop_status"
    } else if release {
        "estop_release"
    } else {
        "estop_engage"
    };

    let resp =
        crate::commands::dashboard::send_internal_command_get_json(&base_url, &auth_token, cmd)
            .await?;
    let engaged = resp.get("engaged").and_then(|v| v.as_bool());

    match (status, release, engaged) {
        (true, _, Some(e)) => {
            println!(
                "E-stop 状态：{}",
                if e {
                    "⛔ ENGAGED（agent 已冻结）"
                } else {
                    "✓ released（agent 正常活动）"
                }
            );
        }
        (_, true, Some(false)) => println!("✓ E-stop 已释放 — agent 恢复活动。"),
        (_, true, _) => println!("E-stop release 指令已发送。"),
        (_, false, Some(true)) => {
            println!("⛔ E-stop 已触发 — agent 活动已冻结。");
            println!("   用 `nemesisbot estop --release` 恢复。");
        }
        (_, false, _) => println!("E-stop engage 指令已发送。"),
    }

    Ok(())
}

#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::Duration;

pub async fn run(local: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = crate::common::resolve_home(local);

    // 1. Read auth_token from config.json
    let config_path = home.join("config.json");
    let cfg_str = std::fs::read_to_string(&config_path).map_err(|e| {
        format!("Cannot read config.json: {}. Run 'nemesisbot onboard default' first.", e)
    })?;
    let cfg: serde_json::Value = serde_json::from_str(&cfg_str)?;
    let auth_token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // 2. Read gateway state
    let state_path = home.join("workspace").join("state").join("gateway.json");
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

struct GatewayInfo {
    #[allow(dead_code)]
    pid: u32,
    web_host: String,
    web_port: i64,
}

fn read_gateway_state(path: &Path) -> Option<GatewayInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    Some(GatewayInfo {
        pid: v["pid"].as_u64()? as u32,
        web_host: v["web_host"].as_str()?.to_string(),
        web_port: v["web_port"].as_i64()?,
    })
}

async fn check_health(base_url: &str) -> Result<(), String> {
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
    if local {
        cmd.arg("--local");
    }
    cmd.arg("gateway");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.spawn()?;

    // Poll for gateway state file + health (up to 30s)
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Some(info) = read_gateway_state(state_path) {
            if info.web_port > 0 {
                let base_url = format!("http://{}:{}", info.web_host, info.web_port);
                if check_health(&base_url).await.is_ok() {
                    println!("  Gateway started (port {})", info.web_port);
                    return Ok((info.web_host, info.web_port));
                }
            }
        }
    }

    Err("Gateway did not start within 30 seconds".into())
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

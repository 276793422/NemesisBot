use std::path::Path;
use std::time::Duration;

pub async fn run(local: bool) -> Result<(), Box<dyn std::error::Error>> {
    let home = crate::common::resolve_home(local);

    // 1. Read auth_token from config.json
    let config_path = home.join("config.json");
    let cfg_str = nemesis_utils::fsutil::read_config_string(&config_path).map_err(|e| {
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

#[derive(Debug)]
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
        // BUG-20261123-017：pid u64 域验域再 cast（port 防御闸同款习语；
        // dashboard 命令不依赖 ziwei feature，故内联而非 nemesis_ziwei::pid）
        // ——损坏的状态文件超域 pid 按无 pid 处理（None），不回绕成小值。
        pid: v["pid"].as_u64().filter(|p| *p <= u32::MAX as u64)? as u32,
        web_host: v["web_host"].as_str()?.to_string(),
        web_port: v["web_port"].as_i64()?,
    })
}

/// 读 gateway state 并区分「文件不存在」与「文件存在但无法解析」两种失败
/// 形态（BUG-20261125-011）。此前 estop/selfupdate/vm/ziwei 四个消费者对
/// 两种形态报同一条「找不到 state 文件」——文件明明在却被告知不存在，把
/// 排查引向错误方向（BUG-20261123-017 让损坏 state → None 后两义性显形）。
/// dashboard 命令自身走 start_and_wait 自愈路径，不需要此区分，仍用
/// `read_gateway_state`。
pub(crate) fn read_gateway_state_or_err(state_path: &Path) -> Result<GatewayInfo, String> {
    match read_gateway_state(state_path) {
        Some(info) => Ok(info),
        None if state_path.exists() => Err(format!(
            "Gateway state 文件存在但无法解析（{}）：内容损坏或缺少必要字段。删除该文件或重启 gateway 重建。",
            state_path.display()
        )),
        None => Err(format!(
            "Gateway 未运行（找不到 state 文件：{}）。先用 `nemesisbot gateway` 启动它。",
            state_path.display()
        )),
    }
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
        // 测试二进制的 main 是 libtest，CLI 参数会被
        // 当成测试过滤器——`["gateway"]` 让直调单测 spawn 出一个嵌套重跑全部
        // commands::gateway 测试（217 个）的 rogue 子进程：双倍 live 负载 +
        // 幻影 result 行污染全量闸门（曾致随机 live 测试 60s 窗超时失败）。
        // 测试形态改传 libtest 空过滤器（exact 不存在的名字）→ 子进程
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

    // 装配纪律：保留子进程句柄，等待循环经 nemesis_utils::wait_ready
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
        // BUG-20261115-045：X-Auth-Token 不在 reqwest 默认重定向剥离清单
        // （标准四头）内——gateway 侧任何 302 跨主机（反代误配/端口撞车）
        // 都会把 token 带走。同源跟随 + 跨源终止（nemesis_utils::http_redirect）。
        .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
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
        let body = nemesis_utils::http_cap::read_error_body(resp).await;
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
        // BUG-20261115-045：同 send_internal_command——X-Auth-Token 凭据面
        // 不跟随跨主机重定向。
        .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
        .build()?;
    let url = format!("{}/api/internal", base_url);
    let resp = client
        .post(&url)
        .header("X-Auth-Token", auth_token)
        .json(&serde_json::json!({ "cmd": cmd }))
        .send()
        .await?;
    let status = resp.status();
    // 双用拆分（BUG-20261113-164）：成功体是内部 API 小 JSON（estop_status
    // 等），全量读合法；错误体只喂诊断文案——有帽读取。
    let body = if status.is_success() {
        // BUG-20261120-037：2xx body 读断连不得吞成空串——旧
        // unwrap_or_default 经 from_str("") 落 json!({}) 假成功，estop
        // status 拿空对象状态失真（急停安全面）。
        resp.text()
            .await
            .map_err(|e| format!("读取 Gateway 响应体失败: {e}"))?
    } else {
        nemesis_utils::http_cap::read_error_body(resp).await
    };
    if status.is_success() {
        Ok(serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({})))
    } else {
        Err(format!("Internal command failed: {} {}", status, body).into())
    }
}

#[cfg(test)]
mod tests;

use super::display::web_bind_and_display_hosts;
use anyhow::Result;
use tracing::info;

/// `--relay` 纯中继模式（goal：反向桥与多设备汇聚，一期批次一）：只起
/// web server（状态页 `/relay` + `/bridge` 接入 + `/d/<node_id>/` 转发），
/// 不起本地 agent/board/集群/discovery——状态页即全部 UI。
/// `bridge.server.token` 空 → 拒绝启动（fail-closed，纯中继不允许裸奔）。
pub(crate) async fn run_relay(home: &std::path::Path, cfg: &nemesis_config::Config) -> Result<()> {
    let token = cfg
        .bridge
        .as_ref()
        .map(|b| b.server.token.as_str())
        .unwrap_or("");
    if token.is_empty() {
        eprintln!(
            "[Relay] --relay 启动失败：config.json 未配置 bridge.server.token（接入门令牌；\
空 = 门不开放）。请在 config.json 的 bridge.server.token 填入预共享令牌后重试。"
        );
        return Err(anyhow::anyhow!(
            "--relay requires bridge.server.token to be set"
        ));
    }
    info!("[Relay] 纯中继模式启动（--relay）：状态页 /relay + /bridge + /d/<node_id>/");

    // 与正常模式同款绑定语义，但 relay 纯中继服务端**必然**要被远端访问
    // （桥接入 /bridge + 状态页 /relay + /d/<node_id>/ 转发都在公网侧）——
    // bind_all 传 true：0.0.0.0 如实绑定所有网卡（此前传 false 使 0.0.0.0
    // 被静默回环成 127.0.0.1，VPS 真机验收暴露）。display host 不参与 relay。
    // SEC-001 豁免：纯中继不装配 /ws 与全量 /api/*（下方 relay_only 注），
    // 「控制面凭据」启动闸在这里没有对象；公网侧接入鉴权由 bridge.server
    // .token 独立把关（上方 fail-closed 校验）。
    let web_bind_host = web_bind_and_display_hosts(&cfg.channels.web.host, true).0;
    let web_port = cfg.channels.web.port;
    // relay_only（2026-09-20，用户裁决）：纯中继不暴露 hub 自身 dashboard
    // ——不传静态资源 + set_relay_only(true)，/ws、全量 /api/*、SPA 静态
    // 资源不装配（/api/* 信任边界是本机/内网，绑 0.0.0.0 公网即失守）。
    // 公网只剩 /health + /bridge 接入 + /relay 状态页 + /d/<node_id>/ 隧道。
    let web_config = nemesis_web::server::WebServerConfig {
        listen_addr: format!("{}:{}", web_bind_host, web_port),
        // P0 vault（B3）：auth_token 支持 vault:/env:/yaml: 引用（web server
        // 侧与 web channel 侧同源解析，两处比较值一致）。鉴权验证类字段：
        // 解析失败回一次性随机 token（fail-closed，绝不静默回空串关鉴权）。
        auth_token: crate::common::resolve_auth_token_or_random(
            &cfg.channels.web.auth_token,
            "channels.web.auth_token",
        ),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        home: Some(home.to_string_lossy().to_string()),
        version: crate::common::VERSION_INFO.version.to_string(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let mut web_server = nemesis_web::server::WebServer::new(web_config);
    web_server.set_relay_only(true);
    let relay_server = std::sync::Arc::new(nemesis_web::relay::RelayServer::new(
        token.to_string(),
        false,
    ));
    relay_server.ensure_maintenance();
    web_server.set_relay(relay_server);
    info!(
        "[Relay] 纯中继就绪：状态页 http://127.0.0.1:{}/relay（ws token 门；/bridge 接入与 \
/d/<node_id>/ 转发同端口复用）",
        web_port
    );

    // serve：阻塞至关停（与正常模式 web server 语义一致）。
    let _addr = web_server.start().await.map_err(|e| anyhow::anyhow!(e))?;
    info!("[Relay] 纯中继 web server 已停止");
    Ok(())
}

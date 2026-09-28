//! `nemesisbot mcp-serve` —— stdio MCP server CLI 入口（P23，能力扩展 WS10）。
//!
//! 薄壳：校验配置存在（headless 不隐式 onboard，与 `run`/`acp` 同纪律）+
//! credentials/vault 注入（与 gateway 同点）后整体委派
//! [`crate::mcp_serve::run_server`]（stdio ndjson JSON-RPC，协议与装配见
//! 该模块头文档）。

use std::path::Path;

pub async fn run(home: &Path) -> anyhow::Result<()> {
    // 配置必须存在（与 run/acp 同纪律：不隐式 onboard）。
    let config_path = crate::common::config_path(home);
    if !config_path.exists() {
        anyhow::bail!(
            "Configuration not found: {}.\nRun 'nemesisbot onboard default' first.",
            config_path.display()
        );
    }
    // U15：模型 API key 走 credentials.yaml（与 gateway/run 同一解析路径）。
    nemesis_config::credentials::set_global_credentials_path(
        nemesis_config::credentials::credentials_path_for_home(home),
    );
    // P0 vault（B1）：`vault:<alias>` 解析器同点注入。
    #[cfg(feature = "security")]
    crate::vault_runtime::install(home);

    crate::mcp_serve::run_server(home.to_path_buf())
        .await
        .map_err(anyhow::Error::msg)
}

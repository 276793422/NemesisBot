//! `nemesisbot acp` —— L7 ACP server CLI 入口。
//!
//! 薄壳：校验配置存在（headless 不隐式 onboard，与 `run` 同纪律）后
//! 整体委派 [`crate::acp::run_server`]（stdio JSON-RPC，协议与装配见
//! 该模块头文档）。session cwd 即编辑器项目目录 = workspace。

use std::path::Path;

pub async fn run(home: &Path) -> anyhow::Result<()> {
    // 配置必须存在（与 `run` 同纪律：不隐式 onboard）。
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

    crate::acp::run_server(home.to_path_buf())
        .await
        .map_err(anyhow::Error::msg)
}

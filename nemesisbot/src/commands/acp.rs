//! `nemesisbot acp` —— L7 ACP server CLI 入口。
//!
//! 薄壳：校验配置存在（headless 不隐式 onboard，与 `run` 同纪律）后
//! 整体委派 [`crate::acp::run_server`]（stdio JSON-RPC，协议与装配见
//! 该模块头文档）。session cwd 即编辑器项目目录 = workspace。

use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};

/// CLI 入口（生产 stdio transport）。
pub async fn run(home: &Path) -> anyhow::Result<()> {
    run_with_io(home, tokio::io::stdin(), tokio::io::stdout()).await
}

/// transport 注入版（测试喂 duplex；生产 [`run`] 走 stdio）。测试不得触碰
/// 进程真实 stdin——永不 EOF 的宿主管道会拖死整个测试进程（见 acp.rs
/// `run_server_with_io` 文档）。
pub async fn run_with_io<R, W>(home: &Path, reader: R, writer: W) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
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
    // P0 vault（B1）：`vault:<alias>` 解析器同点注入。
    #[cfg(feature = "security")]
    crate::vault_runtime::install(home);

    crate::acp::run_server_with_io(home.to_path_buf(), reader, writer)
        .await
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests;

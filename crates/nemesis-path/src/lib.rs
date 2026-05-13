//! Unified path management for NemesisBot.

pub mod paths;

pub use paths::{
    PathManager, resolve_home_dir, expand_home,
    detect_local, set_local_mode, is_local_mode,
    default_path_manager,
    resolve_config_path_in_workspace,
    resolve_mcp_config_path_in_workspace,
    resolve_security_config_path_in_workspace,
    resolve_cluster_config_path_in_workspace,
    resolve_skills_config_path_in_workspace,
    resolve_scanner_config_path_in_workspace,
    resolve_config_path,
    resolve_mcp_config_path,
    resolve_security_config_path,
    resolve_skills_config_path,
    resolve_scanner_config_path,
    ENV_HOME, ENV_CONFIG, ENV_MCP_CONFIG, ENV_SECURITY_CONFIG,
    ENV_SKILLS_CONFIG, ENV_SCANNER_CONFIG,
};

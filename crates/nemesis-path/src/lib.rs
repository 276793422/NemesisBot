//! Unified path management for NemesisBot.

pub mod paths;

pub use paths::{
    ENV_CONFIG, ENV_HOME, ENV_MCP_CONFIG, ENV_SCANNER_CONFIG, ENV_SECURITY_CONFIG,
    ENV_SKILLS_CONFIG, PathManager, default_path_manager, detect_local, expand_home, is_local_mode,
    resolve_cluster_config_path_in_workspace, resolve_config_path,
    resolve_config_path_in_workspace, resolve_home_dir, resolve_mcp_config_path,
    resolve_mcp_config_path_in_workspace, resolve_scanner_config_path,
    resolve_scanner_config_path_in_workspace, resolve_security_config_path,
    resolve_security_config_path_in_workspace, resolve_skills_config_path,
    resolve_skills_config_path_in_workspace, set_local_mode,
};

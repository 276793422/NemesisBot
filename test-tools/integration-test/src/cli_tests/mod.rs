//! CLI command integration tests — comprehensive coverage.
//!
//! Every CLI command, subcommand, and major flag is tested.
//! Organized into sub-modules by command group.

pub mod basic;
pub mod model;
pub mod channel;
pub mod cluster;
pub mod cors;
pub mod security;
pub mod log;
pub mod auth;
pub mod cron;
pub mod mcp;
pub mod skills;
pub mod forge;
pub mod workflow;
pub mod scanner;
pub mod agent;
pub mod misc;

// Re-export all public test functions for convenient access from main.rs
pub use basic::test_cli_version;
pub use basic::test_cli_onboard_default;
pub use basic::test_cli_status;
pub use basic::test_cli_shutdown;

pub use model::test_cli_model_add;
pub use model::test_cli_model_list;
pub use model::test_cli_model_remove;
pub use model::test_cli_model_default;

pub use channel::test_cli_channel_list;
pub use channel::test_cli_channel_enable_disable;
pub use channel::test_cli_channel_web;
pub use channel::test_cli_channel_websocket;
pub use channel::test_cli_channel_external;

pub use cluster::test_cli_cluster_init;
pub use cluster::test_cli_cluster_status;
pub use cluster::test_cli_cluster_config;
pub use cluster::test_cli_cluster_info;
pub use cluster::test_cli_cluster_enable_disable;
pub use cluster::test_cli_cluster_reset;
pub use cluster::test_cli_cluster_peers;
pub use cluster::test_cli_cluster_token;

pub use cors::test_cli_cors_full;

pub use security::test_cli_security_status;
pub use security::test_cli_security_enable_disable;
pub use security::test_cli_security_config;
pub use security::test_cli_security_audit;
pub use security::test_cli_security_test;
pub use security::test_cli_security_rules;
pub use security::test_cli_security_approve_deny;

pub use log::test_cli_log_status_config;
pub use log::test_cli_log_enable_disable;
pub use log::test_cli_log_llm;
pub use log::test_cli_log_general;
pub use log::test_cli_log_level_file_console;

pub use auth::test_cli_auth_status;

pub use cron::test_cli_cron_list;
pub use cron::test_cli_cron_crud;

pub use mcp::test_cli_mcp_crud;
pub use mcp::test_cli_mcp_inspect;

pub use skills::test_cli_skills_list;
pub use skills::test_cli_skills_list_builtin;
pub use skills::test_cli_skills_search;
pub use skills::test_cli_skills_source;
pub use skills::test_cli_skills_validate;
pub use skills::test_cli_skills_show;
pub use skills::test_cli_skills_cache;
pub use skills::test_cli_skills_install_builtin;
pub use skills::test_cli_skills_install;
pub use skills::test_cli_skills_remove;
pub use skills::test_cli_skills_install_clawhub;
pub use skills::test_cli_skills_add_source_duplicate;

pub use forge::test_cli_forge_status;
pub use forge::test_cli_forge_enable_disable;
pub use forge::test_cli_forge_reflect;
pub use forge::test_cli_forge_list;
pub use forge::test_cli_forge_evaluate;
pub use forge::test_cli_forge_export;
pub use forge::test_cli_forge_learning;

pub use workflow::test_cli_workflow_list;
pub use workflow::test_cli_workflow_run_status;
pub use workflow::test_cli_workflow_template;
pub use workflow::test_cli_workflow_validate;

pub use scanner::test_cli_scanner_list;
pub use scanner::test_cli_scanner_add_remove;
pub use scanner::test_cli_scanner_enable_disable;
pub use scanner::test_cli_scanner_check_install;
pub use scanner::test_cli_scanner_download_test_update;

pub use agent::test_cli_agent_set_llm;
pub use agent::test_cli_agent_set_concurrent;
pub use agent::test_cli_agent_message;

pub use misc::test_cli_daemon;
pub use misc::test_cli_migrate;
pub use misc::test_cli_gateway_flags;

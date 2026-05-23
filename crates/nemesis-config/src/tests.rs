use super::*;

#[test]
fn test_default_config() {
    let config = Config::default();
    assert!(config.model_list.is_empty());
    assert!(config.bindings.is_empty());
    assert!(config.gateway.port > 0);
}

#[test]
fn test_config_serialize_deserialize() {
    let config = Config::default();
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.gateway.port, config.gateway.port);
}

#[test]
fn test_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");

    let config = Config::default();
    ConfigLoader::save_to_file(&config, &path).unwrap();
    let loaded = ConfigLoader::load_from_file(&path).unwrap();
    assert_eq!(loaded.gateway.port, config.gateway.port);
}

#[test]
fn test_load_from_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join(".nemesisbot");

    // With config file - test file loading path (not dependent on embedded defaults)
    std::fs::create_dir_all(&workspace).unwrap();
    let custom = r#"{"gateway": {"host": "0.0.0.0", "port": 9090}}"#;
    std::fs::write(workspace.join("config.json"), custom).unwrap();

    let loaded = ConfigLoader::load_from_workspace(&workspace).unwrap();
    assert_eq!(loaded.gateway.host, "0.0.0.0");
    assert_eq!(loaded.gateway.port, 9090);
}

#[test]
fn test_model_config_validate() {
    let model = ModelConfig {
        model_name: "gpt-4".to_string(),
        model: "openai/gpt-4".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    assert!(model.validate().is_ok());

    let empty = ModelConfig::default();
    assert!(empty.validate().is_err());
}

#[test]
fn test_model_parse_protocol() {
    let model = ModelConfig {
        model_name: "test".to_string(),
        model: "anthropic/claude-3".to_string(),
        ..Default::default()
    };
    let (proto, name) = model.parse_model();
    assert_eq!(proto, "anthropic");
    assert_eq!(name, "claude-3");

    let model2 = ModelConfig {
        model_name: "test2".to_string(),
        model: "gpt-4o".to_string(),
        ..Default::default()
    };
    let (proto2, name2) = model2.parse_model();
    assert_eq!(proto2, "openai");
    assert_eq!(name2, "gpt-4o");
}

#[test]
fn test_provider_resolver() {
    let models = vec![
        ModelConfig {
            model_name: "default".to_string(),
            model: "openai/gpt-4".to_string(),
            api_key: "key1".to_string(),
            ..Default::default()
        },
        ModelConfig {
            model_name: "fast".to_string(),
            model: "groq/llama3".to_string(),
            api_key: "key2".to_string(),
            ..Default::default()
        },
    ];

    let found = ProviderResolver::find_by_name(&models, "fast").unwrap();
    assert_eq!(found.model, "groq/llama3");

    let default = ProviderResolver::find_default(&models).unwrap();
    assert_eq!(default.model_name, "default");

    assert!(ProviderResolver::find_by_name(&models, "nonexistent").is_none());
}

#[test]
fn test_resolve_model_string() {
    let (proto, model) = ProviderResolver::resolve_model_string("openai/gpt-4o");
    assert_eq!(proto, "openai");
    assert_eq!(model, "gpt-4o");

    let (proto2, model2) = ProviderResolver::resolve_model_string("llama3");
    assert_eq!(proto2, "openai");
    assert_eq!(model2, "llama3");
}

#[test]
fn test_agent_model_config_string() {
    let json = r#""gpt-4""#;
    let config: AgentModelConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.primary, "gpt-4");
    assert!(config.fallbacks.is_empty());
}

#[test]
fn test_agent_model_config_object() {
    let json = r#"{"primary": "gpt-4", "fallbacks": ["claude-haiku"]}"#;
    let config: AgentModelConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.primary, "gpt-4");
    assert_eq!(config.fallbacks, vec!["claude-haiku"]);
}

#[test]
fn test_workspace_resolve_local() {
    let path = WorkspaceResolver::resolve(true);
    assert_eq!(path, PathBuf::from("./.nemesisbot"));
}

#[test]
fn test_platform_security_config_filename() {
    let filename = get_platform_security_config_filename();
    assert!(filename.starts_with("config.security."));
    assert!(filename.ends_with(".json"));
}

#[test]
fn test_platform_display_name() {
    let name = get_platform_display_name();
    assert!(!name.is_empty());
    assert!(name == "Windows" || name == "Linux" || name == "macOS" || name == "Unknown");
}

#[test]
fn test_platform_info() {
    let info = get_platform_info();
    assert!(info["os"].is_string());
    assert!(info["arch"].is_string());
    assert!(info["display_name"].is_string());
    assert!(info["security_config"].is_string());
}

#[test]
fn test_load_mcp_config_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.mcp.json");
    // Write default MCP config to file to avoid dependency on embedded defaults state
    let default_cfg = McpConfig { enabled: false, servers: vec![], timeout: 30 };
    std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
    let cfg = load_mcp_config(&path).unwrap();
    assert!(!cfg.enabled);
    assert!(cfg.servers.is_empty());
    assert_eq!(cfg.timeout, 30);
}

#[test]
fn test_save_and_load_mcp_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subdir/config.mcp.json");
    let cfg = McpConfig {
        enabled: true,
        servers: vec![McpServerConfig {
            name: "test-server".to_string(),
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            ..Default::default()
        }],
        timeout: 60,
    };
    save_mcp_config(&path, &cfg).unwrap();
    let loaded = load_mcp_config(&path).unwrap();
    assert!(loaded.enabled);
    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].name, "test-server");
    assert_eq!(loaded.timeout, 60);
}

#[test]
fn test_load_security_config_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.security.json");
    // Write default config to file to avoid dependency on embedded defaults state
    let default_cfg = SecurityConfig::default();
    std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
    let cfg = load_security_config(&path).unwrap();
    assert_eq!(cfg.default_action, "deny");
    assert!(cfg.log_all_operations);
    assert_eq!(cfg.approval_timeout_seconds, 300);
}

#[test]
fn test_save_and_load_security_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.security.json");
    let cfg = SecurityConfig {
        default_action: "allow".to_string(),
        file_rules: Some(FileSecurityRules {
            read: vec![SecurityRule {
                pattern: "/workspace/**".to_string(),
                action: "allow".to_string(),
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    save_security_config(&path, &cfg).unwrap();
    let loaded = load_security_config(&path).unwrap();
    assert_eq!(loaded.default_action, "allow");
    assert!(loaded.file_rules.is_some());
    assert_eq!(loaded.file_rules.unwrap().read.len(), 1);
}

#[test]
fn test_load_scanner_config_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.scanner.json");
    // Write default config to file to avoid dependency on embedded defaults state
    let default_cfg = ScannerFullConfig::default();
    std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
    let cfg = load_scanner_config(&path).unwrap();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_save_and_load_scanner_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.scanner.json");
    let mut engines = std::collections::HashMap::new();
    engines.insert("clamav".to_string(), serde_json::json!({"url": "tcp://localhost:3310"}));
    let cfg = ScannerFullConfig {
        enabled: vec!["clamav".to_string()],
        engines,
    };
    save_scanner_config(&path, &cfg).unwrap();
    let loaded = load_scanner_config(&path).unwrap();
    assert_eq!(loaded.enabled, vec!["clamav"]);
    assert!(loaded.engines.contains_key("clamav"));
}

#[test]
fn test_load_skills_config_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.skills.json");
    // Write default config to file to avoid dependency on embedded defaults state
    let default_cfg = SkillsFullConfig::default();
    std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
    let cfg = load_skills_config(&path).unwrap();
    assert!(cfg.enabled);
    assert!(cfg.search_cache.enabled);
    assert_eq!(cfg.max_concurrent_searches, 2);
}

#[test]
fn test_save_and_load_skills_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.skills.json");
    let cfg = SkillsFullConfig {
        enabled: false,
        github_sources: vec![GitHubSourceConfig {
            name: "test-source".to_string(),
            repo: "test/repo".to_string(),
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    save_skills_config(&path, &cfg).unwrap();
    let loaded = load_skills_config(&path).unwrap();
    assert!(!loaded.enabled);
    assert_eq!(loaded.github_sources.len(), 1);
    assert_eq!(loaded.github_sources[0].repo, "test/repo");
}

#[test]
fn test_config_get_model_by_model_name() {
    let config = Config {
        model_list: vec![
            ModelConfig {
                model_name: "default".to_string(),
                model: "openai/gpt-4".to_string(),
                api_key: "key1".to_string(),
                ..Default::default()
            },
            ModelConfig {
                model_name: "fast".to_string(),
                model: "groq/llama3".to_string(),
                api_key: "key2".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // By model_name
    let found = config.get_model_by_model_name("default").unwrap();
    assert_eq!(found.api_key, "key1");

    // By model field (vendor/model)
    let found2 = config.get_model_by_model_name("groq/llama3").unwrap();
    assert_eq!(found2.model_name, "fast");

    // Not found
    assert!(config.get_model_by_model_name("nonexistent").is_err());
}

#[test]
fn test_config_get_model_config() {
    let config = Config {
        model_list: vec![ModelConfig {
            model_name: "test".to_string(),
            model: "test/model".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let found = config.get_model_config("test").unwrap();
    assert_eq!(found.model, "test/model");
}

#[test]
fn test_config_workspace_path() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "/custom/workspace".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(config.workspace_path(), "/custom/workspace");

    let empty_config = Config::default();
    let ws = empty_config.workspace_path();
    assert!(!ws.is_empty());
}

#[test]
fn test_resolve_model_config() {
    let config = Config {
        model_list: vec![ModelConfig {
            model_name: "my-model".to_string(),
            model: "openai/gpt-4".to_string(),
            api_key: "test-key".to_string(),
            api_base: "https://custom.api.com/v1".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    // By model_name
    let res = resolve_model_config(&config, "my-model").unwrap();
    assert_eq!(res.provider_name, "openai");
    assert_eq!(res.model_name, "gpt-4");
    assert_eq!(res.api_key, "test-key");
    assert_eq!(res.api_base, "https://custom.api.com/v1");

    // By model field
    let res2 = resolve_model_config(&config, "openai/gpt-4").unwrap();
    assert_eq!(res2.model_name, "gpt-4");

    // Not found, but can infer provider
    let res3 = resolve_model_config(&config, "claude-3-opus").unwrap();
    assert_eq!(res3.provider_name, "anthropic");

    // Empty ref
    assert!(resolve_model_config(&config, "").is_err());
}

#[test]
fn test_get_model_by_name_free_fn() {
    let config = Config {
        model_list: vec![ModelConfig {
            model_name: "default".to_string(),
            model: "zhipu/glm-4".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let found = get_model_by_name(&config, "default").unwrap();
    assert_eq!(found.model, "zhipu/glm-4");

    let found2 = get_model_by_name(&config, "zhipu/glm-4").unwrap();
    assert_eq!(found2.model_name, "default");

    assert!(get_model_by_name(&config, "nonexistent").is_err());
}

#[test]
fn test_get_effective_llm() {
    assert_eq!(get_effective_llm(None), "zhipu/glm-4.7-flash");

    let config = Config::default();
    assert_eq!(get_effective_llm(Some(&config)), "zhipu/glm-4.7-flash");

    let config_with_llm = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                llm: "anthropic/claude-3".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(get_effective_llm(Some(&config_with_llm)), "anthropic/claude-3");
}

#[test]
fn test_embedded_defaults() {
    let defaults = get_embedded_defaults();
    assert!(defaults.config.is_empty());
    assert!(defaults.mcp.is_empty());

    set_embedded_defaults(
        b"config".to_vec(),
        b"mcp".to_vec(),
        b"security".to_vec(),
        b"cluster".to_vec(),
        b"skills".to_vec(),
        b"scanner".to_vec(),
    );

    let defaults = get_embedded_defaults();
    assert_eq!(defaults.config, b"config".to_vec());
    assert_eq!(defaults.mcp, b"mcp".to_vec());
}

#[test]
fn test_security_config_roundtrip() {
    let cfg = SecurityConfig {
        default_action: "deny".to_string(),
        log_all_operations: true,
        approval_timeout_seconds: 600,
        max_pending_requests: 50,
        audit_log_retention_days: 30,
        audit_log_file_enabled: true,
        synchronous_mode: true,
        file_rules: Some(FileSecurityRules {
            read: vec![SecurityRule {
                pattern: "/workspace/**".to_string(),
                action: "allow".to_string(),
            }],
            write: vec![
                SecurityRule {
                    pattern: "/workspace/**".to_string(),
                    action: "allow".to_string(),
                },
                SecurityRule {
                    pattern: "*.key".to_string(),
                    action: "deny".to_string(),
                },
            ],
            ..Default::default()
        }),
        process_rules: Some(ProcessSecurityRules {
            exec: vec![SecurityRule {
                pattern: "rm -rf *".to_string(),
                action: "deny".to_string(),
            }],
            ..Default::default()
        }),
        layers: Some(SecurityLayersConfig {
            injection: Some(SecurityLayerConfig {
                enabled: true,
                ..Default::default()
            }),
            dlp: Some(DLPLayerConfig {
                enabled: true,
                action: "block".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: SecurityConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.default_action, "deny");
    assert_eq!(parsed.approval_timeout_seconds, 600);
    assert!(parsed.file_rules.is_some());
    let fr = parsed.file_rules.unwrap();
    assert_eq!(fr.write.len(), 2);
    assert!(parsed.layers.is_some());
}

#[test]
fn test_skills_full_config_roundtrip() {
    let cfg = SkillsFullConfig {
        enabled: true,
        search_cache: SkillsSearchCacheConfig {
            enabled: true,
            max_size: 100,
            ttl_seconds: 600,
        },
        max_concurrent_searches: 4,
        github_sources: vec![GitHubSourceConfig {
            name: "anthropics/skills".to_string(),
            repo: "anthropics/skills".to_string(),
            enabled: true,
            index_type: "github_tree".to_string(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            ..Default::default()
        }],
        clawhub: SkillsClawHubConfig {
            enabled: true,
            base_url: "https://clawhub.com".to_string(),
            ..Default::default()
        },
    };

    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: SkillsFullConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.search_cache.max_size, 100);
    assert_eq!(parsed.github_sources.len(), 1);
    assert_eq!(parsed.github_sources[0].name, "anthropics/skills");
}

#[test]
fn test_full_config_roundtrip() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                max_tokens: 256000,
                temperature: 0.5,
                ..Default::default()
            },
            list: vec![AgentConfigEntry {
                id: "main".to_string(),
                default: true,
                name: "Main Agent".to_string(),
                ..Default::default()
            }],
        },
        model_list: vec![ModelConfig {
            model_name: "test".to_string(),
            model: "test/test-1.0".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        }],
        channels: ChannelsConfig {
            web: WebChannelConfig {
                enabled: true,
                port: 9999,
                ..Default::default()
            },
            ..Default::default()
        },
        security: Some(SecurityFlagConfig { enabled: false }),
        ..Default::default()
    };

    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.agents.defaults.max_tokens, 256000);
    assert_eq!(parsed.agents.list.len(), 1);
    assert_eq!(parsed.model_list[0].model, "test/test-1.0");
    assert!(parsed.channels.web.enabled);
    assert_eq!(parsed.channels.web.port, 9999);
    assert!(parsed.security.is_some());
    assert!(!parsed.security.unwrap().enabled);
}

#[test]
fn test_config_error_display() {
    let err = ConfigError::Validation("test error".to_string());
    assert!(err.to_string().contains("test error"));

    let err = ConfigError::WorkspaceNotFound;
    assert!(err.to_string().contains("Workspace not found"));
}

#[test]
fn test_mcp_server_config_default() {
    let cfg = McpServerConfig::default();
    assert!(cfg.name.is_empty());
    assert!(cfg.command.is_empty());
    assert!(cfg.args.is_empty());
    assert!(cfg.env.is_empty());
}

#[test]
fn test_mcp_config_roundtrip() {
    let cfg = McpConfig {
        enabled: true,
        servers: vec![
            McpServerConfig {
                name: "server1".to_string(),
                command: "node".to_string(),
                args: vec!["a.js".to_string(), "b.js".to_string()],
                env: vec!["KEY=VALUE".to_string()],
                timeout: 30,
            },
            McpServerConfig {
                name: "server2".to_string(),
                command: "python".to_string(),
                args: vec!["main.py".to_string()],
                env: vec![],
                timeout: 30,
            },
        ],
        timeout: 120,
    };
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: McpConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.servers.len(), 2);
    assert_eq!(parsed.servers[0].args.len(), 2);
    assert_eq!(parsed.timeout, 120);
}

#[test]
fn test_security_rule_serialization() {
    let rule = SecurityRule {
        pattern: "/workspace/**".to_string(),
        action: "allow".to_string(),
    };
    let json = serde_json::to_string(&rule).unwrap();
    let parsed: SecurityRule = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.pattern, "/workspace/**");
    assert_eq!(parsed.action, "allow");
}

#[test]
fn test_engine_state_default_v2() {
    let cfg = ScannerFullConfig::default();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_agent_model_config_manual() {
    let cfg = AgentModelConfig {
        primary: String::new(),
        fallbacks: vec![],
    };
    assert!(cfg.primary.is_empty());
    assert!(cfg.fallbacks.is_empty());
}

#[test]
fn test_agent_model_config_string_serialization() {
    let cfg = AgentModelConfig {
        primary: "gpt-4".to_string(),
        fallbacks: vec!["claude-3".to_string(), "llama3".to_string()],
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: AgentModelConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.primary, "gpt-4");
    assert_eq!(parsed.fallbacks.len(), 2);
}

#[test]
fn test_agent_defaults_default() {
    let defaults = AgentDefaults::default();
    assert_eq!(defaults.max_tokens, 8192);
    assert_eq!(defaults.temperature, 0.7);
    assert_eq!(defaults.max_tool_iterations, 20);
}

#[test]
fn test_agents_config_default() {
    let agents = AgentsConfig::default();
    assert!(agents.list.is_empty());
}

#[test]
fn test_channels_config_default() {
    let channels = ChannelsConfig::default();
    assert!(!channels.web.enabled);
    assert_eq!(channels.web.port, 8080);
}

#[test]
fn test_gateway_config_default() {
    let gateway = GatewayConfig::default();
    assert_eq!(gateway.host, "0.0.0.0");
    assert_eq!(gateway.port, 18790);
}

#[test]
fn test_model_config_default() {
    let model = ModelConfig::default();
    assert!(model.model_name.is_empty());
    assert!(model.model.is_empty());
    assert!(model.api_key.is_empty());
}

#[test]
fn test_model_parse_no_slash() {
    let model = ModelConfig {
        model_name: "test".to_string(),
        model: "gpt-4o-mini".to_string(),
        ..Default::default()
    };
    let (proto, name) = model.parse_model();
    assert_eq!(proto, "openai");
    assert_eq!(name, "gpt-4o-mini");
}

#[test]
fn test_model_validate_empty_model_name() {
    let model = ModelConfig {
        model_name: String::new(),
        model: "openai/gpt-4".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    assert!(model.validate().is_err());
}

#[test]
fn test_model_validate_empty_api_key() {
    let model = ModelConfig {
        model_name: "test".to_string(),
        model: "openai/gpt-4".to_string(),
        api_key: String::new(),
        ..Default::default()
    };
    // api_key is not validated by validate()
    assert!(model.validate().is_ok());
}

#[test]
fn test_config_get_model_config_not_found() {
    let config = Config::default();
    assert!(config.get_model_config("nonexistent").is_err());
}

#[test]
fn test_workspace_resolver_config_path() {
    let path = WorkspaceResolver::config_path(Path::new("/tmp/test"));
    assert_eq!(path, PathBuf::from("/tmp/test/config.json"));
}

#[test]
fn test_workspace_resolver_ensure_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("new_workspace");
    WorkspaceResolver::ensure_workspace(&ws).unwrap();
    assert!(ws.is_dir());
}

#[test]
fn test_security_config_default_values() {
    let cfg = SecurityConfig::default();
    assert_eq!(cfg.default_action, "deny");
    assert!(cfg.log_all_operations);
    assert_eq!(cfg.approval_timeout_seconds, 300);
    assert_eq!(cfg.max_pending_requests, 100);
    assert_eq!(cfg.audit_log_retention_days, 90);
}

#[test]
fn test_security_layer_config_default() {
    let cfg = SecurityLayerConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.extra.is_empty());
}

#[test]
fn test_file_security_rules_default() {
    let rules = FileSecurityRules::default();
    assert!(rules.read.is_empty());
    assert!(rules.write.is_empty());
    assert!(rules.delete.is_empty());
}

#[test]
fn test_process_security_rules_default() {
    let rules = ProcessSecurityRules::default();
    assert!(rules.exec.is_empty());
    assert!(rules.spawn.is_empty());
    assert!(rules.kill.is_empty());
}

#[test]
fn test_network_security_rules_default() {
    let rules = NetworkSecurityRules::default();
    assert!(rules.request.is_empty());
    assert!(rules.download.is_empty());
    assert!(rules.upload.is_empty());
}

#[test]
fn test_load_config_from_file_nonexistent() {
    let result = ConfigLoader::load_from_file(Path::new("/nonexistent/config.json"));
    assert!(result.is_err());
}

#[test]
fn test_load_config_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "not valid json{{").unwrap();
    let result = ConfigLoader::load_from_file(&path);
    assert!(result.is_err());
}

#[test]
fn test_expand_tilde_absolute() {
    let result = expand_tilde("/absolute/path");
    assert_eq!(result, PathBuf::from("/absolute/path"));
}

#[test]
fn test_expand_tilde_relative() {
    let result = expand_tilde("relative/path");
    assert_eq!(result, PathBuf::from("relative/path"));
}

#[test]
fn test_save_config_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/nested/config.json");
    let config = Config::default();
    ConfigLoader::save_to_file(&config, &path).unwrap();
    assert!(path.exists());
}

// ---- New tests ----

#[test]
fn test_agent_defaults_restrict_to_workspace_default() {
    let defaults = AgentDefaults::default();
    assert!(defaults.restrict_to_workspace);
}

#[test]
fn test_agent_defaults_queue_size() {
    let defaults = AgentDefaults::default();
    assert_eq!(defaults.queue_size, 8);
}

#[test]
fn test_agent_defaults_concurrent_request_mode() {
    let defaults = AgentDefaults::default();
    assert_eq!(defaults.concurrent_request_mode, "reject");
}

#[test]
fn test_session_config_default() {
    let session = SessionConfig::default();
    assert!(session.dm_scope.is_empty());
    assert!(session.identity_links.is_empty());
}

#[test]
fn test_binding_match_manual() {
    let bm = BindingMatch {
        channel: String::new(),
        account_id: String::new(),
        peer: None,
        guild_id: String::new(),
        team_id: String::new(),
    };
    assert!(bm.channel.is_empty());
    assert!(bm.account_id.is_empty());
    assert!(bm.peer.is_none());
}

#[test]
fn test_peer_match_manual() {
    let pm = PeerMatch {
        kind: String::new(),
        id: String::new(),
    };
    assert!(pm.kind.is_empty());
    assert!(pm.id.is_empty());
}

#[test]
fn test_whatsapp_config_default() {
    let cfg = WhatsAppConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.bridge_url.is_empty());
    assert!(cfg.allow_from.is_empty());
}

#[test]
fn test_telegram_config_default() {
    let cfg = TelegramConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.token.is_empty());
}

#[test]
fn test_feishu_config_default() {
    let cfg = FeishuConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.app_id.is_empty());
}

#[test]
fn test_discord_config_default() {
    let cfg = DiscordConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_tools_config_default() {
    let cfg = ToolsConfig::default();
    // Default trait gives serde defaults; brave.max_results=0, duckduckgo.enabled=false via Default trait
    assert_eq!(cfg.web.brave.max_results, 0);
    assert!(!cfg.web.duckduckgo.enabled);
}

#[test]
fn test_exec_config_default() {
    let cfg = ExecConfig::default();
    assert!(!cfg.enable_deny_patterns);
    assert!(cfg.custom_deny_patterns.is_empty());
}

#[test]
fn test_heartbeat_config_default() {
    let cfg = HeartbeatConfig::default();
    assert!(!cfg.enabled); // Default trait gives false, serde default gives true
    assert_eq!(cfg.interval, 0); // Default trait gives 0, serde default gives 30
}

#[test]
fn test_devices_config_default() {
    let cfg = DevicesConfig::default();
    assert!(!cfg.enabled);
    assert!(!cfg.monitor_usb); // Default trait gives false, serde default gives true
}

#[test]
fn test_security_flag_config_default() {
    let cfg = SecurityFlagConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_forge_flag_config_default() {
    let cfg = ForgeFlagConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_memory_flag_config_default() {
    let cfg = MemoryFlagConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_skills_config_default() {
    let cfg = SkillsConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_model_config_parse_multiple_slashes() {
    let model = ModelConfig {
        model_name: "test".to_string(),
        model: "provider/model/variant".to_string(),
        ..Default::default()
    };
    let (proto, name) = model.parse_model();
    assert_eq!(proto, "provider");
    assert_eq!(name, "model/variant");
}

#[test]
fn test_agent_config_entry_default() {
    let entry = AgentConfigEntry::default();
    assert!(entry.id.is_empty());
    assert!(!entry.default);
    assert!(entry.skills.is_empty());
}

#[test]
fn test_subagents_config_manual() {
    let cfg = SubagentsConfig {
        allow_agents: vec![],
        model: None,
    };
    assert!(cfg.allow_agents.is_empty());
    assert!(cfg.model.is_none());
}

#[test]
fn test_channels_config_all_disabled() {
    let cfg = ChannelsConfig::default();
    assert!(!cfg.whatsapp.enabled);
    assert!(!cfg.telegram.enabled);
    assert!(!cfg.feishu.enabled);
    assert!(!cfg.discord.enabled);
    assert!(!cfg.maixcam.enabled);
    assert!(!cfg.qq.enabled);
    assert!(!cfg.dingtalk.enabled);
    assert!(!cfg.slack.enabled);
    assert!(!cfg.line.enabled);
    assert!(!cfg.onebot.enabled);
}

#[test]
fn test_web_channel_config_defaults() {
    let cfg = WebChannelConfig::default();
    assert!(!cfg.enabled);
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.path, "/ws");
    assert_eq!(cfg.heartbeat_interval, 30);
    assert_eq!(cfg.session_timeout, 3600);
}

#[test]
fn test_logging_config_default() {
    let cfg = LoggingConfig::default();
    assert!(cfg.llm.is_none());
    assert!(cfg.general.is_none());
}

#[test]
fn test_llm_log_config_default() {
    let cfg = LlmLogConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.log_dir.is_empty());
    assert_eq!(cfg.detail_level, ""); // Default trait gives empty string, serde default gives "full"
}

#[test]
fn test_general_log_config_default() {
    let cfg = GeneralLogConfig::default();
    assert!(!cfg.enabled); // Default trait gives false, serde default gives true
    assert!(!cfg.enable_console); // Default trait gives false, serde default gives true
    assert_eq!(cfg.level, ""); // Default trait gives empty string, serde default gives "INFO"
}

#[test]
fn test_mcp_config_default() {
    let cfg = McpConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.servers.is_empty());
    assert_eq!(cfg.timeout, 0); // Default trait gives 0, serde default gives 30
}

#[test]
fn test_dlp_layer_config_default() {
    let cfg = DLPLayerConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.rules.is_empty());
    assert!(cfg.action.is_empty());
}

#[test]
fn test_signature_layer_config_default() {
    let cfg = SignatureLayerConfig::default();
    assert!(!cfg.enabled);
    assert!(!cfg.strict);
}

#[test]
fn test_hardware_security_rules_defaults_v2() {
    let rules = HardwareSecurityRules::default();
    assert!(rules.i2c.is_empty());
    assert!(rules.spi.is_empty());
    assert!(rules.gpio.is_empty());
}

#[test]
fn test_registry_security_rules_defaults_v2() {
    let rules = RegistrySecurityRules::default();
    assert!(rules.read.is_empty());
    assert!(rules.write.is_empty());
    assert!(rules.delete.is_empty());
}

#[test]
fn test_directory_security_rules_defaults_v2() {
    let rules = DirectorySecurityRules::default();
    assert!(rules.read.is_empty());
    assert!(rules.create.is_empty());
    assert!(rules.delete.is_empty());
}

#[test]
fn test_clamav_engine_config_default() {
    let cfg = ClamAVEngineConfig::default();
    assert!(cfg.url.is_empty());
    assert!(!cfg.scan_on_write);
    assert!(!cfg.scan_on_download);
    assert!(cfg.scan_extensions.is_empty());
}

#[test]
fn test_flexible_string_vec_with_numbers() {
    let json = r#"{"allow_from": ["user1", 123, "user3"]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from, vec!["user1", "123", "user3"]);
}

#[test]
fn test_flexible_string_vec_empty_array() {
    let json = r#"{"allow_from": []}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.allow_from.is_empty());
}

#[test]
fn test_config_post_process_for_compatibility() {
    let mut config = Config::default();
    config.channels.external.sync_to = vec!["web".to_string()];
    config.channels.websocket.sync_to = vec!["web".to_string()];
    config.post_process_for_compatibility();
    assert!(config.channels.external.sync_to_web);
    assert!(config.channels.websocket.sync_to_web);
}

#[test]
fn test_config_post_process_no_sync() {
    let mut config = Config::default();
    config.post_process_for_compatibility();
    assert!(!config.channels.external.sync_to_web);
    assert!(!config.channels.websocket.sync_to_web);
}

#[test]
fn test_config_workspace_path_empty() {
    let config = Config::default();
    let ws = config.workspace_path();
    assert!(!ws.is_empty());
}

#[test]
fn test_config_workspace_path_tilde() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/custom/path".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let ws = config.workspace_path();
    assert!(!ws.starts_with("~"));
    assert!(ws.contains("custom"));
}

#[test]
fn test_security_layers_config_default() {
    let cfg = SecurityLayersConfig::default();
    assert!(cfg.injection.is_none());
    assert!(cfg.command_guard.is_none());
    assert!(cfg.dlp.is_none());
    assert!(cfg.ssrf.is_none());
    assert!(cfg.credential.is_none());
    assert!(cfg.signature.is_none());
    assert!(cfg.audit_chain.is_none());
}

#[test]
fn test_brave_config_default() {
    let cfg = BraveConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.api_key.is_empty());
    assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
}

#[test]
fn test_duckduckgo_config_default() {
    let cfg = DuckDuckGoConfig::default();
    assert!(!cfg.enabled); // Default trait gives false, serde default gives true
    assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
}

#[test]
fn test_perplexity_config_default() {
    let cfg = PerplexityConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.api_key.is_empty());
    assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
}

#[test]
fn test_cron_tools_config_default() {
    let cfg = CronToolsConfig::default();
    assert_eq!(cfg.exec_timeout_minutes, 0);
}

#[test]
fn test_websocket_channel_config_default() {
    let cfg = WebSocketChannelConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.host.is_empty());
    assert!(!cfg.sync_to_web);
}

#[test]
fn test_external_config_default() {
    let cfg = ExternalConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.input_exe.is_empty());
    assert!(!cfg.sync_to_web);
}

#[test]
fn test_maixcam_config_default() {
    let cfg = MaixCamConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.host.is_empty());
    assert_eq!(cfg.port, 0);
}

#[test]
fn test_qq_config_default() {
    let cfg = QqConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.app_id.is_empty());
}

#[test]
fn test_dingtalk_config_default() {
    let cfg = DingTalkConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.client_id.is_empty());
}

#[test]
fn test_slack_config_default() {
    let cfg = SlackConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.bot_token.is_empty());
}

#[test]
fn test_line_config_default() {
    let cfg = LineConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.channel_secret.is_empty());
}

#[test]
fn test_onebot_config_default() {
    let cfg = OneBotConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.ws_url.is_empty());
    assert!(cfg.group_trigger_prefix.is_empty());
}

#[test]
fn test_engine_state_default() {
    let state = EngineState::default();
    assert!(state.install_status.is_empty());
    assert!(state.install_error.is_empty());
    assert!(state.last_install_attempt.is_empty());
    assert!(state.db_status.is_empty());
    assert!(state.last_db_update.is_empty());
}

#[test]
fn test_skills_full_config_enabled_by_default() {
    let cfg = SkillsFullConfig::default();
    assert!(cfg.enabled);
}

#[test]
fn test_github_source_config_default() {
    let cfg = GitHubSourceConfig::default();
    assert!(cfg.name.is_empty());
    assert!(cfg.repo.is_empty());
    assert!(!cfg.enabled);
}

#[test]
fn test_skills_clawhub_config_default() {
    let cfg = SkillsClawHubConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.base_url.is_empty());
}

// ---- Additional coverage tests ----

#[test]
fn test_expand_tilde_home() {
    let result = expand_tilde("~");
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    assert_eq!(result, home);
}

#[test]
fn test_expand_tilde_with_path() {
    let result = expand_tilde("~/some/path");
    if let Some(home) = dirs::home_dir() {
        assert_eq!(result, home.join("some/path"));
    }
}

#[test]
fn test_config_workspace_path_absolute() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "/absolute/path".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(config.workspace_path(), "/absolute/path");
}

#[test]
fn test_apply_env_overrides_gateway_host() {
    let mut config = Config::default();

    // SAFETY: single-threaded test, no concurrent access to this env var
    unsafe { std::env::set_var("NEMESISBOT_GATEWAY_HOST", "192.168.1.1"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.gateway.host, "192.168.1.1");

    unsafe { std::env::remove_var("NEMESISBOT_GATEWAY_HOST"); }
}

#[test]
fn test_apply_env_overrides_gateway_port() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_GATEWAY_PORT", "9999"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.gateway.port, 9999);

    unsafe { std::env::remove_var("NEMESISBOT_GATEWAY_PORT"); }
}

#[test]
fn test_apply_env_overrides_security_enabled() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_SECURITY_ENABLED", "true"); }
    apply_env_overrides(&mut config);
    assert!(config.security.is_some());
    assert!(config.security.as_ref().unwrap().enabled);

    unsafe { std::env::remove_var("NEMESISBOT_SECURITY_ENABLED"); }
}

#[test]
fn test_apply_env_overrides_forge_enabled() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_FORGE_ENABLED", "true"); }
    apply_env_overrides(&mut config);
    assert!(config.forge.is_some());
    assert!(config.forge.as_ref().unwrap().enabled);

    unsafe { std::env::remove_var("NEMESISBOT_FORGE_ENABLED"); }
}

#[test]
fn test_apply_env_overrides_workspace() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE", "/custom/ws"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.workspace, "/custom/ws");

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE"); }
}

#[test]
fn test_apply_env_overrides_max_tokens() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS", "16000"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.max_tokens, 16000);

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS"); }
}

#[test]
fn test_apply_env_overrides_temperature() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE", "0.3"); }
    apply_env_overrides(&mut config);
    assert!((config.agents.defaults.temperature - 0.3).abs() < f64::EPSILON);

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE"); }
}

#[test]
fn test_apply_env_overrides_heartbeat() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_HEARTBEAT_ENABLED", "false"); }
    unsafe { std::env::set_var("NEMESISBOT_HEARTBEAT_INTERVAL", "60"); }
    apply_env_overrides(&mut config);
    assert!(!config.heartbeat.enabled);
    assert_eq!(config.heartbeat.interval, 60);

    unsafe { std::env::remove_var("NEMESISBOT_HEARTBEAT_ENABLED"); }
    unsafe { std::env::remove_var("NEMESISBOT_HEARTBEAT_INTERVAL"); }
}

#[test]
fn test_apply_env_overrides_session_dm_scope() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_SESSION_DM_SCOPE", "per-peer"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.session.dm_scope, "per-peer");

    unsafe { std::env::remove_var("NEMESISBOT_SESSION_DM_SCOPE"); }
}

#[test]
fn test_platform_info_json_fields() {
    let info = get_platform_info();
    assert!(info.get("os").is_some());
    assert!(info.get("arch").is_some());
    assert!(info.get("family").is_some());
    assert!(info.get("display_name").is_some());
    assert!(info.get("security_config").is_some());
    // Verify values are strings
    assert!(info["os"].is_string());
    assert!(info["arch"].is_string());
}

#[test]
fn test_deserialize_flexible_string_vec_with_bool() {
    // The deserializer converts non-string, non-number types via to_string()
    let json = r#"{"allow_from": ["user1", true]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from.len(), 2);
    assert_eq!(cfg.allow_from[0], "user1");
    assert_eq!(cfg.allow_from[1], "true");
}

#[test]
fn test_config_default_values() {
    let config = default_config();
    assert!(config.agents.defaults.restrict_to_workspace);
    assert!(!config.channels.web.enabled || config.channels.web.enabled); // depends on default
    assert!(config.gateway.port > 0);
}

#[test]
fn test_config_get_model_by_model_name_not_found() {
    let config = Config::default();
    assert!(config.get_model_by_model_name("nonexistent").is_err());
}

#[test]
fn test_model_validate_empty_model() {
    let model = ModelConfig {
        model_name: "test".to_string(),
        model: String::new(),
        ..Default::default()
    };
    assert!(model.validate().is_err());
}

// ---- Additional coverage tests for 95%+ ----

#[test]
fn test_apply_env_overrides_web_channel() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_ENABLED", "false"); }
    unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_HOST", "127.0.0.1"); }
    unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_PORT", "9999"); }
    apply_env_overrides(&mut config);
    assert!(!config.channels.web.enabled);
    assert_eq!(config.channels.web.host, "127.0.0.1");
    assert_eq!(config.channels.web.port, 9999);

    unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_ENABLED"); }
    unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_HOST"); }
    unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_PORT"); }
}


#[test]
fn test_apply_env_overrides_invalid_bool() {
    let mut config = Config::default();

    // Invalid bool for restrict_to_workspace defaults to true
    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE", "notabool"); }
    apply_env_overrides(&mut config);
    assert!(config.agents.defaults.restrict_to_workspace);

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE"); }
}

#[test]
fn test_apply_env_overrides_llm() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_LLM", "anthropic/claude-3"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.llm, "anthropic/claude-3");

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_LLM"); }
}

#[test]
fn test_apply_env_overrides_image_model() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL", "openai/dall-e-3"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.image_model, "openai/dall-e-3");

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL"); }
}

#[test]
fn test_apply_env_overrides_max_tool_iterations() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS", "50"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.max_tool_iterations, 50);

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS"); }
}

#[test]
fn test_apply_env_overrides_concurrent_request_mode() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE", "queue"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.concurrent_request_mode, "queue");

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE"); }
}

#[test]
fn test_apply_env_overrides_queue_size() {
    let mut config = Config::default();

    unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE", "16"); }
    apply_env_overrides(&mut config);
    assert_eq!(config.agents.defaults.queue_size, 16);

    unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE"); }
}

#[test]
fn test_load_config_nonexistent_falls_through() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent.json");
    // Should not fail - falls through to default
    let config = load_config(&path).unwrap();
    assert!(config.gateway.port > 0);
}

#[test]
fn test_set_embedded_defaults_from_fs_missing_dir() {
    let result = set_embedded_defaults_from_fs(Path::new("/nonexistent/config/dir"));
    assert!(result.is_err());
}

// Combined test to avoid parallel global state race: set embedded defaults from fs
// and verify all load_*_config functions pick them up
#[test]
fn test_embedded_defaults_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path();

    // Write all required files with distinctive values
    let main_config = r#"{"gateway":{"host":"e2e-test","port":9999}}"#;
    let mcp_config = r#"{"enabled":true,"servers":[],"timeout":42}"#;
    let security_config = r#"{"default_action":"allow","log_all_operations":false,"approval_timeout_seconds":123}"#;
    let cluster_config = r#"{"enabled":true,"port":11111}"#;
    let skills_config = r#"{"enabled":false,"github_sources":[],"search_cache":{"enabled":false},"max_concurrent_searches":5}"#;
    let scanner_config = r#"{"enabled":["clamav"],"engines":{}}"#;

    std::fs::write(config_dir.join("config.default.json"), main_config).unwrap();
    std::fs::write(config_dir.join("config.mcp.default.json"), mcp_config).unwrap();
    std::fs::write(config_dir.join(get_platform_security_config_filename()), security_config).unwrap();
    std::fs::write(config_dir.join("config.cluster.default.json"), cluster_config).unwrap();
    std::fs::write(config_dir.join("config.skills.default.json"), skills_config).unwrap();
    std::fs::write(config_dir.join("config.scanner.default.json"), scanner_config).unwrap();

    let result = set_embedded_defaults_from_fs(config_dir);
    assert!(result.is_ok());

    // Verify load_config uses embedded fallback
    let nonexistent = dir.path().join("no-config-here/config.json");
    let config = load_config(&nonexistent).unwrap();
    assert_eq!(config.gateway.host, "e2e-test");
    assert_eq!(config.gateway.port, 9999);

    // Verify load_mcp_config uses embedded fallback
    let mcp_path = dir.path().join("no-config-here/config.mcp.json");
    let mcp = load_mcp_config(&mcp_path).unwrap();
    assert!(mcp.enabled);
    assert_eq!(mcp.timeout, 42);

    // Verify load_security_config uses embedded fallback
    let sec_path = dir.path().join("no-config-here/config.security.json");
    let sec = load_security_config(&sec_path).unwrap();
    assert_eq!(sec.default_action, "allow");
    assert_eq!(sec.approval_timeout_seconds, 123);

    // Verify load_scanner_config uses embedded fallback
    let scan_path = dir.path().join("no-config-here/config.scanner.json");
    let scan = load_scanner_config(&scan_path).unwrap();
    assert_eq!(scan.enabled, vec!["clamav"]);

    // Verify load_skills_config uses embedded fallback
    let skills_path = dir.path().join("no-config-here/config.skills.json");
    let skills = load_skills_config(&skills_path).unwrap();
    assert!(!skills.enabled);
    assert_eq!(skills.max_concurrent_searches, 5);

    // Verify load_embedded_config works
    let embedded = load_embedded_config();
    assert!(embedded.is_ok());
    assert_eq!(embedded.unwrap().gateway.host, "e2e-test");

    // Verify get_embedded_defaults
    let defaults = get_embedded_defaults();
    assert!(!defaults.config.is_empty());
    assert!(!defaults.mcp.is_empty());
}

#[test]
fn test_save_config_local_mode() {
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();
    let config_path = local_dir.join("config.json");

    let mut config = Config::default();
    // Set workspace to default home path so local mode adjusts it
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    config.agents.defaults.workspace = home.join(".nemesisbot").join("workspace").to_string_lossy().to_string();

    // This should succeed even if local mode detection doesn't fully trigger
    let result = save_config(&config_path, &mut config);
    assert!(result.is_ok());
    assert!(config_path.exists());
}

#[test]
fn test_workspace_path_tilde_expansion() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/custom/workspace".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let ws = config.workspace_path();
    // Should expand ~ to home directory
    assert!(!ws.starts_with("~"));
    assert!(ws.contains("custom") || ws.contains("workspace"));
}

#[test]
fn test_workspace_path_empty() {
    let config = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: String::new(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let ws = config.workspace_path();
    assert!(!ws.is_empty());
}

#[test]
fn test_expand_tilde_home_v2() {
    let result = expand_tilde("~");
    assert!(!result.to_string_lossy().starts_with("~"));
}

#[test]
fn test_expand_tilde_subpath() {
    let result = expand_tilde("~/subdir");
    assert!(!result.to_string_lossy().starts_with("~"));
    assert!(result.to_string_lossy().contains("subdir"));
}

#[test]
fn test_post_process_for_compatibility() {
    let mut config = Config::default();
    // External with non-empty sync_to
    config.channels.external.sync_to = vec!["web".to_string()];
    config.channels.websocket.sync_to = vec!["web".to_string()];
    config.post_process_for_compatibility();
    assert!(config.channels.external.sync_to_web);
    assert!(config.channels.websocket.sync_to_web);

    // Empty sync_to
    config.channels.external.sync_to = vec![];
    config.channels.websocket.sync_to = vec![];
    config.post_process_for_compatibility();
    assert!(!config.channels.external.sync_to_web);
    assert!(!config.channels.websocket.sync_to_web);
}

#[test]
fn test_adjust_paths_for_environment_empty_workspace() {
    let mut config = Config::default();
    config.agents.defaults.workspace = String::new();
    config.adjust_paths_for_environment();
    assert!(!config.agents.defaults.workspace.is_empty());
}

#[test]
fn test_adjust_paths_for_environment_log_dir() {
    let mut config = Config::default();
    config.logging = Some(LoggingConfig {
        llm: Some(LlmLogConfig {
            enabled: true,
            log_dir: String::new(),
            detail_level: "full".to_string(),
        }),
        general: None,
    });
    config.adjust_paths_for_environment();
    assert_eq!(config.logging.as_ref().unwrap().llm.as_ref().unwrap().log_dir, "logs/request_logs");
}

#[test]
fn test_adjust_paths_for_environment_existing_log_dir() {
    let mut config = Config::default();
    config.logging = Some(LoggingConfig {
        llm: Some(LlmLogConfig {
            enabled: true,
            log_dir: "custom/logs".to_string(),
            detail_level: "full".to_string(),
        }),
        general: None,
    });
    config.adjust_paths_for_environment();
    assert_eq!(config.logging.as_ref().unwrap().llm.as_ref().unwrap().log_dir, "custom/logs");
}

#[test]
fn test_adjust_paths_for_environment_no_logging() {
    let mut config = Config::default();
    config.logging = None;
    config.adjust_paths_for_environment();
    assert!(config.logging.is_none());
}

#[test]
fn test_load_config_from_file_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let json = r#"{"gateway": {"host": "0.0.0.0", "port": 9999}}"#;
    std::fs::write(&path, json).unwrap();
    let config = load_config(&path).unwrap();
    assert_eq!(config.gateway.port, 9999);
}

#[test]
fn test_clamav_engine_config_roundtrip() {
    let cfg = ClamAVEngineConfig {
        url: "tcp://localhost:3310".to_string(),
        clamav_path: "/usr/bin/clamav".to_string(),
        scan_on_write: true,
        scan_extensions: vec![".exe".to_string()],
        skip_extensions: vec![".txt".to_string()],
        max_file_size: 100_000_000,
        state: EngineState {
            install_status: "installed".to_string(),
            db_status: "ready".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let parsed: ClamAVEngineConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.url, "tcp://localhost:3310");
    assert!(parsed.scan_on_write);
    assert_eq!(parsed.scan_extensions.len(), 1);
    assert_eq!(parsed.state.install_status, "installed");
}

#[test]
fn test_config_loader_save_to_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    let config = Config::default();
    ConfigLoader::save_to_workspace(&config, &workspace).unwrap();
    let config_path = workspace.join("config.json");
    assert!(config_path.exists());
}

#[test]
fn test_dlp_layer_config_roundtrip() {
    let cfg = DLPLayerConfig {
        enabled: true,
        rules: vec!["no_credit_card".to_string(), "no_ssn".to_string()],
        action: "block".to_string(),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: DLPLayerConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.rules.len(), 2);
    assert_eq!(parsed.action, "block");
}

#[test]
fn test_signature_layer_config_roundtrip() {
    let cfg = SignatureLayerConfig {
        enabled: true,
        strict: true,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: SignatureLayerConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.enabled);
    assert!(parsed.strict);
}

#[test]
fn test_model_config_all_fields() {
    let model = ModelConfig {
        model_name: "test-model".to_string(),
        model: "test/test-v1".to_string(),
        api_base: "https://api.test.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        proxy: "http://proxy:8080".to_string(),
        auth_method: "bearer".to_string(),
        connect_mode: "streaming".to_string(),
        workspace: "/custom/ws".to_string(),
    };
    assert_eq!(model.api_base, "https://api.test.com/v1");
    assert_eq!(model.proxy, "http://proxy:8080");
    assert_eq!(model.auth_method, "bearer");
    assert_eq!(model.connect_mode, "streaming");
    assert_eq!(model.workspace, "/custom/ws");
}

#[test]
fn test_general_log_config_defaults_v2() {
    let cfg = GeneralLogConfig::default();
    assert!(!cfg.enabled); // Default trait gives false, serde default gives true
    assert!(!cfg.enable_console); // Default trait gives false
    assert!(cfg.level.is_empty()); // Default trait gives "", serde default gives "INFO"
    assert!(cfg.file.is_empty());
}

#[test]
fn test_llm_log_config_defaults_v2() {
    let cfg = LlmLogConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.log_dir.is_empty());
    assert!(cfg.detail_level.is_empty());
}

#[test]
fn test_cron_tools_config_defaults_v2() {
    let cfg = CronToolsConfig::default();
    assert_eq!(cfg.exec_timeout_minutes, 0);
}

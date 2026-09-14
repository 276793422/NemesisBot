//! ldap 模块纯逻辑测试（不需要真实目录服务）：转义 + DN 解析 + 角色归一。
//! 端到端协议测试（需 OpenLDAP 容器）在本文件底部，`#[ignore]` 门控。

use super::*;

#[test]
fn dn_escape_special_chars() {
    assert_eq!(escape_dn_value("plain"), "plain");
    assert_eq!(escape_dn_value("a,b"), "a\\,b");
    assert_eq!(escape_dn_value("a+b"), "a\\+b");
    assert_eq!(escape_dn_value("a\"b"), "a\\\"b");
    assert_eq!(escape_dn_value("a<b>c;d"), "a\\<b\\>c\\;d");
    assert_eq!(escape_dn_value("back\\slash"), "back\\\\slash");
}

#[test]
fn dn_escape_leading_trailing() {
    assert_eq!(escape_dn_value("#admin"), "\\#admin");
    assert_eq!(escape_dn_value(" leading"), "\\ leading");
    assert_eq!(escape_dn_value("trailing "), "trailing\\ ");
    assert_eq!(escape_dn_value("mid dle"), "mid dle", "中间空格不转义");
}

/// DN 注入攻击样例：用户名带逗号试图改变 RDN 结构。
#[test]
fn dn_injection_neutralized() {
    // 攻击者输入：carl,ou=groups —— 试图把自己挪进 groups 树
    let escaped = escape_dn_value("carl,ou=groups");
    let dn = format!("uid={},ou=people,dc=example,dc=org", escaped);
    // 注：`=` 在属性值内按 RFC 4514 无需转义（DN 解析按未转义逗号切 RDN、
    // 每 RDN 只认第一个 `=`，值内后续 `=` 不改变结构）——关键是逗号已转义，
    // 攻击者仍在 people 子树下且只是一个普通 uid。
    assert_eq!(dn, "uid=carl\\,ou=groups,ou=people,dc=example,dc=org");
    // 解析回 CN 抽取也一致（无 CN 段）
    assert_eq!(extract_cn_from_dn(&dn), None);
}

#[test]
fn filter_escape_control_chars() {
    assert_eq!(escape_filter_value("plain"), "plain");
    assert_eq!(escape_filter_value("a*b"), "a\\2ab");
    assert_eq!(escape_filter_value("(x)"), "\\28x\\29");
    assert_eq!(escape_filter_value("back\\slash"), "back\\5cslash");
}

/// filter 注入攻击样例：用户名带通配符试图扩大组匹配。
#[test]
fn filter_injection_neutralized() {
    let evil_dn = "uid=*,ou=people,dc=example,dc=org";
    let filter = format!(
        "(&(objectClass=groupOfNames)(member={}))",
        escape_filter_value(evil_dn)
    );
    assert!(filter.contains("uid=\\2a,ou"), "通配符必须被转义：{filter}");
}

#[test]
fn extract_cn_basic_and_escapes() {
    assert_eq!(
        extract_cn_from_dn("cn=operators,ou=groups,dc=example,dc=org"),
        Some("operators".to_string())
    );
    assert_eq!(
        extract_cn_from_dn("CN=Admins,OU=groups,DC=example,DC=org"),
        Some("Admins".to_string())
    );
    assert_eq!(
        extract_cn_from_dn("uid=carl,ou=people,dc=example,dc=org"),
        None,
        "无 CN 段应返回 None"
    );
    assert_eq!(extract_cn_from_dn("cn=a\\,b,ou=g"), Some("a,b".to_string()));
}

#[test]
fn normalize_roles_from_member_of_dns() {
    let raw = vec![
        "cn=operators,ou=groups,dc=example,dc=org".to_string(),
        "cn=viewers,ou=groups,dc=example,dc=org".to_string(),
        "cn=operators,ou=groups,dc=example,dc=org".to_string(), // 重复应去重
    ];
    assert_eq!(normalize_roles(&raw), vec!["operators", "viewers"]);
    assert!(normalize_roles(&[]).is_empty());
}

// ---------------- 端到端协议测试（需要 docker compose up 的 OpenLDAP） ----------------
// 运行：cargo test -p authn-core -- --ignored

fn e2e_config() -> LdapConfig {
    let mut config = LdapConfig::new(
        "ldap://localhost:1389",
        "uid={},ou=people,dc=example,dc=org",
    );
    config.base_dn = "dc=example,dc=org".to_string();
    config.group_extraction = GroupExtraction::MemberFilter;
    config
}

#[tokio::test]
#[ignore = "需要 OpenLDAP 容器（docker/docker-compose.yml，端口 1389）"]
async fn e2e_bind_and_group_filter() {
    let result = authenticate(&e2e_config(), "carl", "carl123")
        .await
        .expect("carl/carl123 应 bind 成功");
    assert_eq!(result.user_dn, "uid=carl,ou=people,dc=example,dc=org");
    assert_eq!(result.identity.roles, vec!["operators"]);
    assert_eq!(
        result.identity.source,
        AuthnSource::Ldap {
            server: "ldap://localhost:1389".into(),
            user_dn: "uid=carl,ou=people,dc=example,dc=org".into(),
        }
    );
}

#[tokio::test]
#[ignore = "需要 OpenLDAP 容器"]
async fn e2e_bad_password_rejected() {
    let err = authenticate(&e2e_config(), "carl", "wrong")
        .await
        .expect_err("错误密码应被拒绝");
    assert!(matches!(err, LdapError::InvalidCredentials));
}

#[tokio::test]
#[ignore = "需要 OpenLDAP 容器"]
async fn e2e_unknown_user_rejected() {
    let err = authenticate(&e2e_config(), "ghost", "whatever")
        .await
        .expect_err("不存在用户应被拒绝");
    assert!(matches!(err, LdapError::InvalidCredentials));
}

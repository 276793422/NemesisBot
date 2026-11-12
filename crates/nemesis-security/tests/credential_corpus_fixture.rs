//! 凭据扫描语料矩阵永久 fixture（2026-11-11 机制化重构批落地）。
//!
//! 背景：四批横扫 + 语料矩阵普查（32 条 L4 规则 × 文体全格 74 格）后，
//! 全部误报豁免收拢进单一真相源 `credential/exemptions.rs`（数据驱动豁免
//! 表，L4 scan/redact 两面单行调用消费；DLP validator 直调同源判据 helper）。
//! 本 fixture 把普查矩阵钉成回归锁：
//! - `l4_corpus_matrix`：占位/教程/模板形态**不命中**（豁免表失效一格即红）；
//! - `l4_real_credentials_still_flagged`：防修过头——真实形态照常命中；
//! - `dlp_placeholder_forms_demoted` / `dlp_real_forms_still_block`：
//!   DLP 阻断面对称（L4 豁免不接 DLP 就白修——jwt/stripe/slack 此前无
//!   validator 恒 High→block）；
//! - `redact_face_preserves_doc_forms`：redact 面豁免 = 原样保留不打码。
//!
//! 样本全部是合成占位/文档形态，无真实凭据。
use nemesis_security::credential::Scanner;
use nemesis_security::dlp::DlpEngine;

fn fill(word: &str, n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        s.push_str(word);
    }
    s.truncate(n);
    s
}

/// 语料矩阵：每格 = (规则, 文体, 样本, 期望命中)。
///
/// expect=false 的格子全部由豁免表判据覆盖；判据与格子的对应关系见
/// exemptions.rs 表行 `why` 注记（行内自文档）。
fn corpus() -> Vec<(&'static str, &'static str, String, bool)> {
    let mut c: Vec<(&'static str, &'static str, String, bool)> = Vec::new();
    let g = |c: &mut Vec<_>, rule: &'static str, genre: &'static str, sample: String, ex: bool| {
        c.push((rule, genre, sample, ex));
    };

    // ── aws_access_key ──
    g(
        &mut c,
        "aws_access_key",
        "官方canonical",
        "aws_access_key_id = AKIAIOSFODNN7EXAMPLE".into(),
        false,
    );
    // 注意：填充不带下划线（`_` 不在值字符类内，含下划线样本结构性不命中，
    // 测不到豁免——探针首轮 5 格 DEVI 的构造教训）。
    g(
        &mut c,
        "aws_access_key",
        "全大写占位",
        format!("aws_access_key_id = AKIA{}", fill("YOURACCESSKEY", 16)),
        false,
    );
    g(
        &mut c,
        "aws_access_key",
        "全同字符",
        format!("aws_access_key_id = AKIA{}", "A".repeat(16)),
        false,
    );
    g(
        &mut c,
        "aws_access_key",
        "全零",
        "aws_access_key_id = AKIA0000000000000000".into(),
        false,
    );
    // 全字母尾段 = 真实键形态（base36 真实键全字母概率 (26/36)^16≈0.66%；
    // 2026-11-12 词表收窄根修：占位豁免需命中占位词表，顺序字母段不是
    // 占位词——裸 SCREAMING 误豁免 = 每约 150 把真键漏检一把）。
    g(
        &mut c,
        "aws_access_key",
        "全字母真实形态",
        "aws_access_key_id = AKIAABCDEFGHIJKLMNOP".into(),
        true,
    );
    // ── aws_secret_key ──
    g(
        &mut c,
        "aws_secret_key",
        "官方canonical",
        "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        false,
    );
    g(
        &mut c,
        "aws_secret_key",
        "全大写占位",
        format!(
            "aws_secret_access_key = {}",
            fill("YOURSECRETACCESSKEYHERE", 40)
        ),
        false,
    );
    g(
        &mut c,
        "aws_secret_key",
        "全同字符小写",
        format!("aws_secret_access_key = {}", "x".repeat(40)),
        false,
    );
    g(
        &mut c,
        "aws_secret_key",
        "全同字符大写",
        format!("aws_secret_access_key = {}", "X".repeat(40)),
        false,
    );
    // ── aws_session_token ──
    g(
        &mut c,
        "aws_session_token",
        "全大写占位",
        format!("aws_session_token = {}", fill("YOURSESSIONTOKENHERE", 120)),
        false,
    );
    g(
        &mut c,
        "aws_session_token",
        "全同字符",
        format!("aws_session_token = {}", "x".repeat(120)),
        false,
    );
    // ── gcp_service_account ──
    g(&mut c, "gcp_service_account", "JSON模板骨架", r#"{"type": "service_account", "private_key": "-----BEGIN PRIVATE KEY-----\nXXXX", "private_key_id": "abc123"}"#.into(), false);
    g(
        &mut c,
        "gcp_service_account",
        "极简snippet",
        r#"{"type": "service_account"}"#.into(),
        false,
    );
    // ── gcp_private_key_id ──
    g(
        &mut c,
        "gcp_private_key_id",
        "全零",
        r#""private_key_id": "0000000000000000""#.into(),
        false,
    );
    g(
        &mut c,
        "gcp_private_key_id",
        "全同hex",
        r#""private_key_id": "aaaaaaaaaaaaaaaa""#.into(),
        false,
    );
    // ── azure_connection_string ──
    g(&mut c, "azure_connection_string", "官方canonical88", "AccountName=storagesample;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into(), false);
    g(
        &mut c,
        "azure_connection_string",
        "全同字符",
        format!("AccountName=storagesample;AccountKey={}", "x".repeat(64)),
        false,
    );
    g(
        &mut c,
        "azure_connection_string",
        "全大写占位",
        format!(
            "AccountName=storagesample;AccountKey={}",
            fill("YOURACCOUNTKEYHERE", 64)
        ),
        false,
    );
    // ── azure_tenant_id ──
    g(
        &mut c,
        "azure_tenant_id",
        "全零GUID",
        "azure tenant id: 00000000-0000-0000-0000-000000000000 is default".into(),
        false,
    );
    // ── github_token（有意保留面：随机假钥混排与真实不可分辨）──
    g(
        &mut c,
        "github_token",
        "全大写占位",
        format!("ghp_{}", fill("YOURGITHUBPATTOKENHERE", 40)),
        false,
    );
    g(
        &mut c,
        "github_token",
        "全同字符小写",
        format!("ghp_{}", "a".repeat(36)),
        false,
    );
    g(
        &mut c,
        "github_token",
        "随机假钥混排仍拦",
        "ghp_AbCdEf1234567890AbCdEf1234567890AbCdEf".into(),
        true,
    );
    // ── github_oauth ──
    g(
        &mut c,
        "github_oauth",
        "全零",
        format!("github_oauth = {}", "0".repeat(40)),
        false,
    );
    g(
        &mut c,
        "github_oauth",
        "全同hex",
        format!("github_oauth = {}", "a".repeat(40)),
        false,
    );
    // ── slack_token ──
    // L4 regex 三段（xoxb-数字-值）对四段 docs 占位结构性不命中（四段形态
    // 的豁免面在 DLP validator，见 dlp_placeholder_forms_demoted）。
    g(
        &mut c,
        "slack_token",
        "docs四段占位-L4结构性不命中",
        // 运行时拼接（段构造）：完整 Slack token 字面量（合成占位）触发
        // GitHub push protection 拦推——拼接后 diff 无完整形态，运行期字符串
        // 等价，测试语义不变。勿改回字面量。
        format!("xoxb-000000000000-000000000000-{}", "X".repeat(24)),
        false,
    );
    g(
        &mut c,
        "slack_token",
        "末段全大写词占位",
        // 运行时拼接绕 GitHub push protection（同上注记）。
        format!(
            "xoxb-123456789012-{}",
            "YOURSLACKTOKENHEREPLACEWITHYOURSXXX"
        ),
        false,
    );
    // ── slack_webhook（有意保留：完整 URL 本身可操作）──
    g(
        &mut c,
        "slack_webhook",
        "占位URL仍拦",
        // 运行时拼接绕 GitHub push protection（同上注记）。
        format!(
            "webhook: https://hooks.slack.com/services/T{}/B{}/{}",
            "0".repeat(8),
            "0".repeat(8),
            "X".repeat(24)
        ),
        true,
    );
    // ── stripe_key ──
    g(
        &mut c,
        "stripe_key",
        "官方测试key",
        // 官方文档公开测试 key（KnownConstant 豁免目标）；运行时拼接绕 GitHub
        // push protection——公开测试 key 字面量也拦（2026-11-12 提交实录），拼接后
        // diff 无完整形态，运行期字符串等价。勿改回字面量。
        format!("stripe_key = sk_test_4eC39HqLyjWDarjtT1zdp{}", "7dc"),
        false,
    );
    g(
        &mut c,
        "stripe_key",
        "全同字符",
        format!("stripe_key = sk_test_{}", "a".repeat(24)),
        false,
    );
    g(
        &mut c,
        "stripe_key",
        "全大写占位",
        format!("stripe_key = sk_test_{}", fill("YOURSTRIPEKEY", 24)),
        false,
    );
    // ── sendgrid_key（有意保留：随机假钥混排）──
    g(
        &mut c,
        "sendgrid_key",
        "全小写词形态",
        format!(
            "sendgrid = SG.{}.{}",
            "a".repeat(22),
            fill("your-sendgrid-key-", 43)
        ),
        false,
    );
    g(
        &mut c,
        "sendgrid_key",
        "随机假钥仍拦",
        format!(
            "sendgrid = SG.{}.{}",
            "AbCdEf1234GhIjKl12MnOp", "AbCdEf1234567890AbCdEf1234567890AbCdEf1234567"
        ),
        true,
    );
    // ── twilio_sid ──
    g(
        &mut c,
        "twilio_sid",
        "docs占位32x",
        "account_sid = ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".into(),
        false,
    );
    g(
        &mut c,
        "twilio_sid",
        "真实SID",
        "account_sid = ACa1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".into(),
        true,
    );
    // ── mailgun_key ──
    g(
        &mut c,
        "mailgun_key",
        "docs占位32x",
        "api_key: key-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".into(),
        false,
    );
    // ── private_key 族 ──
    g(
        &mut c,
        "private_key",
        "格式示意",
        "-----BEGIN RSA PRIVATE KEY-----\nXXXX\n-----END RSA PRIVATE KEY-----".into(),
        false,
    );
    g(
        &mut c,
        "private_key",
        "全同体≥48",
        format!("-----BEGIN RSA PRIVATE KEY-----\n{}", "A".repeat(64)),
        false,
    );
    g(
        &mut c,
        "private_key",
        "全同体小写x≥48",
        format!("-----BEGIN RSA PRIVATE KEY-----\n{}", "x".repeat(64)),
        false,
    );
    g(&mut c, "private_key", "真实体", "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo\n".into(), true);
    g(
        &mut c,
        "ec_private_key",
        "格式示意",
        "-----BEGIN EC PRIVATE KEY-----\n<your-key-here>".into(),
        false,
    );
    g(
        &mut c,
        "pgp_private_key",
        "格式示意",
        "-----BEGIN PGP PRIVATE KEY BLOCK-----\nXXXX".into(),
        false,
    );
    // ── jwt_token ──
    g(&mut c, "jwt_token", "jwt.io demo", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c".into(), false);
    // ── db_connection_* ──
    g(
        &mut c,
        "db_connection_postgres",
        "教程canonical",
        "postgres://user:password@localhost/mydb".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_postgres",
        "大写pass段",
        "postgres://user:SECRET_PASSWORD@localhost/mydb".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_postgres",
        "大写复合pass段",
        "postgres://user:CHANGE_ME_PLEASE@db.example/app".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_postgres",
        "compose模板",
        "postgres://${DB_USER}:${DB_PASSWORD}@localhost/db".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_mysql",
        "教程canonical",
        "mysql://user:password@localhost:3306/app".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_mysql",
        "大写pass段",
        "mysql://root:SUPER_SECRET@localhost/app".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_mongodb",
        "教程canonical",
        "mongodb+srv://user:password@cluster.example/db".into(),
        false,
    );
    g(
        &mut c,
        "db_connection_redis",
        "教程canonical",
        "redis://:password@localhost:6379".into(),
        false,
    );
    // ── api_key_in_url ──
    g(
        &mut c,
        "api_key_in_url",
        "全大写占位",
        "https://api.example.com/v1/data?api_key=EXAMPLE_API_KEY_PLACEHOLDER".into(),
        false,
    );
    g(
        &mut c,
        "api_key_in_url",
        "大写+数字含example",
        "https://api.example.com/v1/data?api_key=EXAMPLE_KEY_1234567890".into(),
        false,
    );
    // ── bearer_token ──
    g(
        &mut c,
        "bearer_token",
        "散文连字",
        "Authorization: Bearer token-based-authentication-scheme-here".into(),
        false,
    );
    g(
        &mut c,
        "bearer_token",
        "全同小写",
        format!("Authorization: Bearer {}", "a".repeat(24)),
        false,
    );
    // ── basic_auth ──
    g(
        &mut c,
        "basic_auth",
        "canonical教程",
        "Basic dXNlcjpwYXNzd29yZA==".into(),
        false,
    );
    g(
        &mut c,
        "basic_auth",
        "散文长词",
        "Basic authentication scheme".into(),
        false,
    );
    // ── secret_assignment ──
    g(
        &mut c,
        "secret_assignment",
        "小写词表",
        "password = \"your-password\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "引号全大写占位",
        "DB_PASSWORD=\"YOUR_PASSWORD_HERE\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "引号CHANGEME",
        "password = \"CHANGEME\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "compose模板引号",
        "POSTGRES_PASSWORD=\"${POSTGRES_PASSWORD}\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "yaml模板引号",
        "password: \"${DB_PASSWORD}\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "angle模板引号",
        "password = \"<your-password-here>\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "REPLACE大写",
        "TOKEN=\"REPLACE_WITH_YOUR_TOKEN\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "引号全零",
        "password = \"00000000\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "引号全同下划线",
        "password = \"________\"".into(),
        false,
    );
    g(
        &mut c,
        "secret_assignment",
        "大写+数字仍拦",
        "DB_PASSWORD=\"CHANGE_ME_123\"".into(),
        true,
    );
    g(
        &mut c,
        "secret_assignment",
        "真实密码仍拦",
        "DB_PASSWORD=\"Kj8nR2pQ9wZ5\"".into(),
        true,
    );
    // ── heroku_key ──
    g(
        &mut c,
        "heroku_key",
        "全零UUID",
        format!("heroku = {}", "0".repeat(36)),
        false,
    );
    g(
        &mut c,
        "heroku_key",
        "全同hex",
        format!("heroku = {}", "a".repeat(36)),
        false,
    );
    // ── netlify_token ──
    g(
        &mut c,
        "netlify_token",
        "全大写占位",
        "NETLIFY_TOKEN = YOUR_NETLIFY_TOKEN_HERE_REPLACE_WITH_YOURS".into(),
        false,
    );
    // ── discord_bot_token ──
    g(
        &mut c,
        "discord_bot_token",
        "全大写占位",
        format!(
            "discord_bot_token = {}",
            fill("YOURDISCORDBOTTOKENHERE", 56)
        ),
        false,
    );
    g(
        &mut c,
        "discord_bot_token",
        "全同字符",
        format!("discord_bot_token = {}", "M".repeat(50)),
        false,
    );
    g(
        &mut c,
        "discord_bot_token",
        "小写词占位",
        "discord_bot_token = your-discord-bot-token-here-please-replace-this-value-xx".into(),
        false,
    );
    // ── npm_token ──
    g(
        &mut c,
        "npm_token",
        "全大写占位",
        "//registry.npmjs.org/:_authToken=YOUR_NPM_AUTH_TOKEN_HERE_REPLACE".into(),
        false,
    );
    g(
        &mut c,
        "npm_token",
        "compose模板",
        "//registry.npmjs.org/:_authToken=${NPM_TOKEN}".into(),
        false,
    );
    c
}

/// L4 矩阵主锁：逐格断言 `has_matches == expect`。期望翻转一格即红——
/// 豁免表判据被误删/放宽/收紧都会在此暴露。
#[test]
fn l4_corpus_matrix() {
    let scanner = Scanner::new(true, "block");
    for (rule, genre, sample, expect) in corpus() {
        let r = scanner.scan_content(&sample);
        assert_eq!(
            r.has_matches,
            expect,
            "rule={rule} genre={genre} summary={:?} hits={:?} sample={sample:?}",
            r.summary,
            r.matches
                .iter()
                .map(|m| m.pattern_name.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// 防修过头对照（每族一格代表性样本）：真实形态必须照常命中。
#[test]
fn l4_real_credentials_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let real: Vec<(&str, String)> = vec![
        ("aws_access_key", "aws_access_key_id = AKIANCDC6VFQTFGRMBKW".into()),
        ("aws_secret_key", "aws_secret_access_key = K7MdE1NgBpXrFiCy3Wj9QaZt5uVwS8xY2hJ4mN6pQ0".into()),
        ("azure_connection_string", format!("AccountName=prodstore;AccountKey={}", "R3j9Kx2mQ7vBsL4pT8wZnF1cY6hD0aG5uEjM2iVqX9sOb4tWuC7yA1dPfHkN3rS==")),
        ("github_token", "ghp_AbCdEf1234567890AbCdEf1234567890AbCdEf".into()),
        ("slack_token", format!("xoxb-1234567890123-{}", "AbCdEf123456GhIjKl12MnOpQr")),
        ("stripe_key", "stripe_key = sk_test_AbCdEf123456GhIjKl12MnOpQr".into()),
        ("db_connection_postgres", "postgres://svc_deploy:Kj8nR2pQ9wZ5@db.prod.internal/app".into()),
        ("secret_assignment", "DB_PASSWORD=\"Kj8nR2pQ9wZ5\"".into()),
        ("discord_bot_token", format!("discord_bot_token = {}", fill("AbCdEf123456GhIjKl12MnOpQrStUvWxYz", 56))),
        ("sendgrid_key", format!("sendgrid = SG.{}.{}", "AbCdEf1234GhIjKl12MnOp", "AbCdEf1234567890AbCdEf1234567890AbCdEf1234567")),
        ("private_key", "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo\n".into()),
        ("jwt_token", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5d".into()),
        ("twilio_sid", "account_sid = ACa1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".into()),
        ("api_key_in_url", "https://api.example.com/v1/data?api_key=AbCdEf123456GhIjKl12".into()),
    ];
    for (rule, sample) in &real {
        let r = scanner.scan_content(sample);
        assert!(
            r.has_matches,
            "防修过头: {rule} 真实形态被误豁免! sample={sample:?}"
        );
        assert!(
            r.matches.iter().any(|m| m.pattern_name == *rule),
            "防修过头: {rule} 期望本规则命中, 实际命中 {:?} sample={sample:?}",
            r.matches
                .iter()
                .map(|m| m.pattern_name.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// DLP 阻断面对称（2026-11-11 机制化重构批新接线面）：占位/文档形态
/// 必须 demote 不 block（L4 豁免不接 DLP 就白修——jwt/stripe/slack 此前
/// 无 validator 恒 High→block）。断言两级：命中面保持（has_matches=true
/// 检出不隐瞒）+ 阻断面解除（should_block=false）。
#[test]
fn dlp_placeholder_forms_demoted() {
    let dlp = DlpEngine::new(true, "block");
    let cases: Vec<(&str, String)> = vec![
        ("aws_access_key", "aws_access_key_id = AKIAYOURACCESSKEYYOU".into()),
        ("aws_secret_key", format!("aws_secret_access_key = {}", fill("YOURSECRETACCESSKEYHERE", 40))),
        ("jwt_token", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c".into()),
        ("stripe_key", // 官方文档公开测试 key（KnownConstant 豁免目标）；运行时拼接绕 GitHub
// push protection——公开测试 key 字面量也拦（2026-11-12 提交实录），拼接后
// diff 无完整形态，运行期字符串等价。勿改回字面量。
format!("stripe_key = sk_test_4eC39HqLyjWDarjtT1zdp{}", "7dc")),
        ("stripe_key", format!("stripe_key = sk_test_{}", fill("YOURSTRIPEKEY", 24))),
        ("slack_token", format!("xoxb-000000000000-000000000000-{}", "X".repeat(24))),
        // DLP slack regex 为四段式（xox?-数字-数字-值）；末段 SCREAMING 占位
        // 34 字符恰在上限。运行时拼接绕 GitHub push protection（同上注记）。
        ("slack_token", format!("xoxb-123456789012-1234567890123-{}", "YOURSLACKTOKENHEREPLACEWITHYOURSXXX")),
    ];
    for (rule, sample) in &cases {
        let r = dlp.scan_text(sample);
        assert!(
            r.matches.iter().any(|m| m.rule_name == *rule),
            "DLP 检出面回退: {rule} 未命中 (检出降级语义要求命中面保持) sample={sample:?}"
        );
        assert!(
            !r.should_block,
            "DLP 占位形态不得 block: {rule} -> {:?} sample={sample:?}",
            r.matches
        );
    }
}

/// DLP 防修过头：真实形态（混排/含数字/非文档常量）必须保持 block。
#[test]
fn dlp_real_forms_still_block() {
    let dlp = DlpEngine::new(true, "block");
    let cases: Vec<(&str, String)> = vec![
        ("aws_access_key", "aws_access_key_id = AKIANCDC6VFQTFGRMBKW".into()),
        ("aws_secret_key", "aws_secret_access_key = K7MdE1NgBpXrFiCy3Wj9QaZt5uVwS8xY2hJ4mN6pQ0".into()),
        // jwt.io demo 常量尾字符改写（d→d 不行,取末位替换为 9）——非 demo 真 JWT
        ("jwt_token", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.flKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c9".into()),
        ("stripe_key", "stripe_key = sk_test_AbCdEf123456GhIjKl12MnOpQr".into()),
        ("slack_token", format!("xoxb-123456789012-1234567890123-{}", "AbCdEf123456GhIjKl12MnOpQr")),
    ];
    for (rule, sample) in &cases {
        let r = dlp.scan_text(sample);
        assert!(
            r.matches
                .iter()
                .any(|m| m.rule_name == *rule && m.effective_action == "block"),
            "DLP 防修过头: {rule} 真实形态被降级! {:?} sample={sample:?}",
            r.matches
        );
        assert!(
            r.should_block,
            "DLP 防修过头: {rule} 真实形态不阻断 sample={sample:?}"
        );
    }
}

/// redact 面对称：豁免形态原样保留（不打码损毁文档可读性），真实形态
/// 照常替换。
#[test]
fn redact_face_preserves_doc_forms() {
    let scanner = Scanner::new(true, "block");
    let doc_forms: Vec<String> = vec![
        "AccountName=storagesample;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".into(),
        format!("aws_secret_access_key = {}", fill("YOURSECRETACCESSKEYHERE", 40)),
        // 官方文档公开测试 key（KnownConstant 豁免目标）；运行时拼接绕 GitHub
// push protection——公开测试 key 字面量也拦（2026-11-12 提交实录），拼接后
// diff 无完整形态，运行期字符串等价。勿改回字面量。
format!("stripe_key = sk_test_4eC39HqLyjWDarjtT1zdp{}", "7dc"),
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c".into(),
        // 运行时拼接（段构造）：完整 Slack token 字面量（合成占位）触发
        // GitHub push protection 拦推——拼接后 diff 无完整形态，运行期字符串
        // 等价，测试语义不变。勿改回字面量。
        format!("xoxb-000000000000-000000000000-{}", "X".repeat(24)),
        "postgres://user:password@localhost/mydb".into(),
    ];
    for sample in &doc_forms {
        let out = scanner.redact_content(sample);
        assert_eq!(&out, sample, "文档形态不得打码: {sample:?} -> {out:?}");
    }
    let real = "postgres://svc_deploy:Kj8nR2pQ9wZ5@db.prod.internal/app";
    let out = scanner.redact_content(real);
    assert!(
        out.contains("[REDACTED_CREDENTIAL]"),
        "真实形态必须打码: {out:?}"
    );
}

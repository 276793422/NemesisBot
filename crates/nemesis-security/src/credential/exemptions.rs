//! 豁免表（exemption table）——凭据扫描 Layer 4 全部误报豁免的单一真相源。
//!
//! ## 为什么有这个模块
//!
//! 2026-11-11 五批横扫后用户拍板机制化重构（原话要点：「一次都查全」+
//! 「在同一个位置做好整批的豁免字符串定义」+「机制上能针对特定字符串豁免」
//! +「不要四处贴膏药」）。此前四批修复每批在 scan_content / redact_content
//! 两条 if 链各贴一组钩子（共 12 组 × 2 面），DLP validator 第三面手工对齐
//! ——判定逻辑虽然抽了 helper（单一真相源），但**接线面**散在五处，新增
//! 一类豁免要动五处代码，且没有任何机制回答「全部豁免有哪些」。
//!
//! 本模块把接线收拢为一张**数据表**：每行 = `规则 × 值段提取 × 判据`，
//! `scan_content` / `redact_content` 各剩一行 `is_exempt(...)` 调用；DLP
//! validator 消费同一批判据 helper（语义差异只在处置：L4 跳过 / redact
//! 保留 / DLP demote Low）。新增豁免 = 表加一行 + 语料 fixture 加格，
//! 不再碰三面钩子链。
//!
//! ## 表的读法
//!
//! - `rules`：适用的 L4 规则名（空表 = 全规则，仅 vault 引用行使用）。
//! - `extract`：值段提取——判定必须落在值本体而不是含键名的整段。
//! - `pred`：判据谓词；同一规则多行 = 任一行判 true 即豁免（行间独立，
//!   与旧 if 链逐条 `continue` 语义等价）。
//! - 消费入口：[`is_exempt`]（Layer 4 两面共用）+ 判据 helper（DLP 面
//!   validator 直接调用）。
//!
//! ## 语料矩阵（2026-11-11 五批普查结论）
//!
//! 32 规则 × 文体全格矩阵（74 格）真 scanner 实跑：全部命中面收敛为
//! **六类原理判据**（UniformChar / ScreamingPlaceholder / ProseToken /
//! TemplatePair / KnownConstant / 既有判据），三枚知名文档常量
//!（jwt.io demo 签名 / Azure 官方示例 AccountKey / Stripe 官方测试 key）
//! 以 `KnownConstant` 精确豁免——这正是用户要的「针对特定字符串的机制性
//! 豁免」。结构性不可分辨类（随机形假钥、slack_webhook 占位 URL）有意
//! 不豁免，见 fixture 注记与报告 §11。

use base64::Engine;
use regex::Regex;

use nemesis_types::utils;

// ─────────────────────────────────────────────────────────────────────────
// 知名文档常量（KnownConstant 谓词的精确豁免目标）
// ─────────────────────────────────────────────────────────────────────────

/// Azure 官方文档 canonical 示例 AccountKey（88 字符，每篇 Azure 存储教程
/// 都有）。§7.3 曾记「与真实 key 同长度不可分辨」——KnownConstant 机制化
/// 后精确豁免成为可能：真实密钥与该常量逐字节相等的概率为零。
pub(crate) const AZURE_DOC_ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

/// jwt.io（jwt 调试器事实标准）demo token 的固定签名段。整枚 demo token
/// 是全世界 JWT 教程的 canonical 示例；签名段 64 字符逐字节固定，真实
/// token 携带该签名的概率为零。按签名段匹配（而非整 token）：header/
/// payload 教程自订变体（同签名）同样豁免。
pub(crate) const JWT_IO_DEMO_SIGNATURE: &str = "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

/// Stripe 官方文档测试 key 值段（`sk_test_`/`pk_test_` 后随部分，每篇
/// Stripe 教程都有）。§7.3 曾记「结构前缀规则固有」——常量精确豁免后
/// 只放行该固定值，其余 `sk_test_` 值照旧拦截。
pub(crate) const STRIPE_DOCS_TEST_KEY: &str = "4eC39HqLyjWDarjtT1zdp7dc";

// ─────────────────────────────────────────────────────────────────────────
// 判据谓词（单一真相源；L4 表与 DLP validator 共用）
// ─────────────────────────────────────────────────────────────────────────

/// 散文/占位符 token 值判据（2026-11-11 三次横扫批）：值段**无数字**且
/// **非大小写混排**（全小写、全大写或无字母）→ 散文。消费规则：bearer /
/// api_key_in_url / npm_token / github_token / netlify / slack / stripe /
/// sendgrid / discord 的值段（分层提取分隔符尾段后传入）。
///
/// 机理：随机凭据（base62）的大小写近乎均匀——真实 token 同时「无数字
/// 且单一大小写」的概率 ≈ 2×(26/62)^20 ≈ 1.4e-5（≥20 字符段），零实际
/// 漏检面；而文档占位符（`EXAMPLE_API_KEY_PLACEHOLDER`、`token-based-
/// authentication-scheme-here`、`YOUR_NPM_AUTH_TOKEN_HERE`）几乎恒满足。
/// digit-only 闸被否：纯字母随机 token 占 ~3%（(52/62)^20），误杀面过大。
///
/// 纯标点段（`Bearer ---------------` 类，无字母无数字）按散文豁免——
/// 无信息量，不构成可操作凭据。
pub(crate) fn is_prose_token_value(value: &str) -> bool {
    let has_digit = value.chars().any(|c| c.is_ascii_digit());
    let has_lower = value.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = value.chars().any(|c| c.is_ascii_uppercase());
    !(has_digit || (has_lower && has_upper))
}

/// AWS 官方文档 canonical 示例判据（2026-11-11 三次横扫批）：匹配段含
/// "example"（大小写不敏感）→ 文档示例。消费规则：aws 三条 + api_key_in_url
/// （五批矩阵：`?api_key=EXAMPLE_KEY_1234567890` 含 example 即占位）。
///
/// AWS 官方文档示例（`AKIAIOSFODNN7EXAMPLE`、
/// `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`）出现在**每一篇** AWS
/// 教程——含 EXAMPLE 七连字。真实 40 字符 base62 秘钥恰含该七连字的
/// 概率 ~1e-9，零实际漏检面。
pub(crate) fn is_canonical_doc_example(matched: &str) -> bool {
    matched.to_lowercase().contains("example")
}

/// 全同字符占位符判据（2026-11-11 四次横扫批）：值段非空且所有字符相同
/// ——教程占位符惯例（Twilio/Mailgun 文档 canonical 32 连 x、Azure tenant
/// 全零 GUID、五批矩阵扩展面：aws 全零/全同 ×40、`"private_key_id":
/// "0000000000000000"`、`"________"` 引号值、`"00000000"` 引号值）。
/// 真实凭据全部字符相同的概率 (1/36)^31 ≈ 5.7e-49 量级，零实际漏检面。
pub(crate) fn is_uniform_char_value(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => chars.all(|c| c == first),
        None => false,
    }
}

/// SCREAMING_SNAKE 占位符判据（2026-11-11 五批，语料矩阵收敛的新原理
/// 判据）：值段全由大写字母 + 分隔符（`-`/`_`/`.`）构成且至少一个大写
/// ——shell/env 占位符惯例（`YOUR_PASSWORD_HERE`、`CHANGEME`、
/// `REPLACE_WITH_YOUR_TOKEN`、`SECRET_PASSWORD`、`CHANGE_ME_PLEASE`、
/// `YOURACCESSKEY`）。矩阵一次性打中整个大写占位符族。
///
/// 含数字不豁免（`CHANGE_ME_123` 保守拦截）；小写/混排不豁免。漏检界：
/// 真实口令全大写且无数字的概率（人类口令习惯加数字/混排；随机 base62
/// ≥20 字符段 (26/62)^20 ≈ 2.4e-5）——与 prose 判据同量级，零实际漏检面。
pub(crate) fn is_screaming_placeholder(value: &str) -> bool {
    !value.is_empty()
        && value.chars().any(|c| c.is_ascii_uppercase())
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || matches!(c, '-' | '_' | '.'))
}

/// 模板语法对判据（2026-11-11 五批）：值段含完整 `${...}` 或 `<...>` 对
/// ——compose/YAML 模板值（`"${POSTGRES_PASSWORD}"`、`"${DB_PASSWORD}"`）
/// 与尖括号占位（`"<your-password-here>"`）。刻意用**对**而非裸模板字符：
/// `pa$$word` 类真实口令含 `$` 但无 `${`…`}` 对，不受影响。
pub(crate) fn is_template_placeholder_pair(value: &str) -> bool {
    (value.contains("${") && value.contains('}')) || (value.contains('<') && value.contains('>'))
}

/// 教程复合词表（secret_assignment 引号值段豁免用，2026-11-11 三次横扫
/// 批）：规范教程词（is_canonical_placeholder_pass 词表）+ 连接词/骨架词
/// ——`your-password` / `my-secret-here` 类分解后每段都落表。
/// `correct-horse-battery` 类真实口令模式不在表内，照旧拦截。
const TUTORIAL_COMPOUND_WORDS: [&str; 27] = [
    "password",
    "pass",
    "pwd",
    "secret",
    "example",
    "changeme",
    "mypassword",
    "yourpassword",
    "placeholder",
    "postgres",
    "root",
    "my",
    "your",
    "here",
    "token",
    "key",
    "api",
    "sample",
    "test",
    "dummy",
    "default",
    "admin",
    "user",
    "replace",
    "insert",
    "enter",
    "with",
];

/// 教程复合词值判据：值段全由小写字母 + 分隔符（`-`/`_`/`.`）构成，且
/// 分隔分解后**每段**都属教程复合词表。含数字或大小写混排不豁免（真实
/// 口令形态；大写占位族由 is_screaming_placeholder 承接）。
pub(crate) fn is_tutorial_compound_value(value: &str) -> bool {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || matches!(c, '-' | '_' | '.'))
    {
        return false;
    }
    value
        .split(['-', '_', '.'])
        .all(|p| TUTORIAL_COMPOUND_WORDS.contains(&p))
}

/// 规范教程词表（db 连接串与 basic_auth 两族共用，2026-11-11 二次横扫批
/// 抽出）：密码段归一（剔 `-`/`_`/`.` + 小写）后比对。
fn is_canonical_placeholder_pass(pass: &str) -> bool {
    let normalized: String = pass
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect::<String>()
        .to_lowercase();
    matches!(
        normalized.as_str(),
        "password"
            | "pass"
            | "pwd"
            | "secret"
            | "example"
            | "changeme"
            | "mypassword"
            | "yourpassword"
            | "placeholder"
            | "postgres"
            | "root"
    )
}

/// db_connection 族（Layer 4 四规则 + DLP `database_connection_string`）的
/// 占位符豁免单一真相源（2026-11-11 横扫批；五批加 SCREAMING 臂）：凭据
/// 段是教程/模板占位符形态时不构成泄漏。三类形态：
/// 1. 模板语法**对**（user 或 pass 任一段，is_template_placeholder_pair）：
///    compose / k8s / envsubst 清单的 `${DB_USER}:${DB_PASSWORD}`、文档
///    尖括号 `<user>:<password>`。BUG-20261113-124 修正：此处原用裸模板
///    字符（任含 `$<>{}{` 之一即豁免），与同模块 is_template_placeholder_pair
///    的既定原理（「刻意用对而非裸模板字符：`pa$$word` 类真实口令含 `$`
///    但无 `${…}` 对，不受影响」）自相矛盾——`postgres://admin:Pa$$w0rd@host`
///    类真实口令（`$` 是高频密码字符）被整串豁免，L4 拦截与 DLP demote
///    双面漏检。改对判据后裸 `$`/`{`（无 `${…}` 配对）照旧拦截；教程
///    模板形态全部带对，豁免面不变。
/// 2. 密码段是 SCREAMING_SNAKE 占位（五批，is_screaming_placeholder）：
///    `SECRET_PASSWORD` / `CHANGE_ME_PLEASE` / `SUPER_SECRET`——矩阵实测
///    大写复合占位此前全拦；
/// 3. 密码段是规范教程词（大小写不敏感，`-`/`_`/`.` 归一后比对）：
///    所有 DB 教程的 canonical 示例 `postgres://user:password@localhost/db`
///    及本地 dev 默认词（password/pass/pwd/secret/example/changeme/
///    mypassword/yourpassword/placeholder/postgres/root）。空密码段
///    （`user:@host`）同样豁免——无凭据可言。
///
/// 真实凭据（非常见词、无模板语法）照旧拦截。语义分工：Layer 4 豁免 =
/// 跳过（与 vault 引用豁免同型），DLP = validator 判 false → demote Low
/// 只 log 不 block（与 luhn 同型）——两层都保留可观测性，不静默。
pub(crate) fn is_placeholder_db_credential(matched: &str) -> bool {
    let rest = match matched.split_once("://") {
        Some((_, r)) => r,
        None => return false,
    };
    let cred = match rest.rsplit_once('@') {
        Some((c, _)) => c,
        None => return false,
    };
    let (user, pass) = match cred.split_once(':') {
        Some((u, p)) => (u, p),
        None => (cred, ""),
    };
    if is_template_placeholder_pair(user) || is_template_placeholder_pair(pass) {
        return true;
    }
    if pass.is_empty() {
        return true;
    }
    is_screaming_placeholder(pass) || is_canonical_placeholder_pass(pass)
}

/// basic_auth 占位符/散文豁免单一真相源（2026-11-11 二次横扫批，目标设备
/// d96357f1 实录：`{12,}` 下限修复后仍有 1 credential [basic_auth] 命中）。
/// 两类豁免：
/// 1. **散文长词**：`Basic authentication` / `Basic authorization`——
///    authentication(14)/authorization(13) 超过 {12,} 下限，长度闸挡不住；
///    判据 = RFC 7617 结构：Basic 凭据协议上恒为 base64(user:pass)，解码
///    失败或解码后无 `:` 分隔即非凭据形态（散文词 base64 化是乱码、无
///    冒号）——零真实漏检面（协议保证真实凭据必有 `:`）。
/// 2. **canonical 教程示例**：解码后 user:pass 的 pass 段属规范教程词
///    （`dXNlcjpwYXNzd29yZA==` = base64("user:password")，所有认证教程的
///    示例头）——词表与 db 连接串豁免共用（is_canonical_placeholder_pass）。
///
/// 输入为规则完整匹配（`Basic <token>`）；bearer_token 不适用（opaque
/// token 非 base64，无解码判据，靠 {20,} 下限——散文最长常见词 14 < 20）。
pub(crate) fn is_placeholder_basic_auth(matched: &str) -> bool {
    let mut parts = matched.split_whitespace();
    if !parts
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case("basic"))
    {
        return false;
    }
    let Some(token) = parts.next() else {
        return false;
    };
    let core = token.trim_end_matches('=');
    if core.is_empty() {
        return true;
    }
    let mut padded = core.to_string();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let decoded = match base64::engine::general_purpose::STANDARD.decode(padded.as_bytes()) {
        Ok(d) => d,
        Err(_) => return true, // 非法 base64 = 散文词
    };
    let Ok(text) = String::from_utf8(decoded) else {
        return false; // 解出非 UTF-8：保守起见不豁免（按疑似凭据保留）
    };
    match text.split_once(':') {
        Some((_, pass)) => is_canonical_placeholder_pass(pass),
        None => true, // 无 user:pass 结构 = 散文词乱码
    }
}

/// content 中是否存在 ≥min_len 的连续标准 base64 字符段
///（`[A-Za-z0-9+/=]`）且该段**非全同字符**（2026-11-11 五批收窄：全同
/// 段——`"A".repeat(64)` 类——是占位示意不是真实钥材，真实 PEM 体行内
/// 字符分布均匀，全同概率天文级小）。真实 PEM 体每行 64 字符 base64；
/// 模板/示意占位是 `XXXX` / `<your-key>` / 全同填充类短段或退化段。
pub(crate) fn has_real_base64_run(text: &str, min_len: usize) -> bool {
    let mut run = 0usize;
    let mut all_same = true;
    let mut first = 0u8;
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' {
            if run == 0 {
                first = b;
                all_same = true;
            } else if b != first {
                all_same = false;
            }
            run += 1;
            if run >= min_len && !all_same {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// private_key 族 PEM 格式示意判据（2026-11-11 三次横扫批；五批换
/// has_real_base64_run）：marker 后 300 字符窗口内无 ≥48 字符连续**非全同**
/// base64 段 = 格式示意文档，不构成泄漏。阈值 48：真实 PEM 体行 64 字符
/// （容忍截断/缩进行），示意段（`XXXX`、省略号、短占位、全同填充）远低
/// 于此或退化。JSON 内嵌形态（SA JSON 的 private_key 字段值，`\n` 转义为
/// 两字符）同样每行 64 ≥ 48，照常判定为真实体。
pub(crate) fn is_pem_illustration(content: &str, match_end: usize) -> bool {
    let end = (match_end + 300).min(content.len());
    let end = utils::floor_char_boundary(content, end);
    let window = content.get(match_end..end).unwrap_or("");
    !has_real_base64_run(window, 48)
}

/// gcp_service_account 模板骨架判据（2026-11-11 三次横扫批）：SA JSON
/// 的泄漏实质恒在 private_key 字段——content 无 PEM marker，或 marker
/// 后是示意段（is_pem_illustration 同一判据）= 模板骨架（键材占位
/// `XXXX`/`<your-key>` 类），`"type": "service_account"` 标记行不构成
/// 泄漏 → 豁免。真实 SA JSON 的 private_key 恒为完整 PEM 体（~1600 字符
/// base64）→ 照常拦截。
pub(crate) fn is_template_service_account_json(content: &str) -> bool {
    match content.find("-----BEGIN") {
        None => true,
        Some(pos) => is_pem_illustration(content, pos),
    }
}

/// 末对引号内值段提取（`"private_key_id": "0000…"` / `password = "x"`
/// 共用）：定位末次出现的引号字符与其前一个同种引号之间的本体。单对
/// 引号（secret_assignment）时即首对；键名带引号的 JSON 形态取值段。
/// 引号不成对返回 None。
pub(crate) fn extract_quoted_tail(matched: &str) -> Option<&str> {
    let q = matched.chars().rfind(|c| *c == '"' || *c == '\'')?;
    let end = matched.rfind(q)?;
    let start = matched[..end].rfind(q)?;
    matched.get(start + q.len_utf8()..end)
}

/// secret_assignment 引号值段复合豁免单一真相源（2026-11-11 三次横扫批
/// 建立基线；五批扩四臂）：引号内值属任一占位形态 → 豁免。
/// 1. PEM 占位值臂：值含 `-----BEGIN` 且无 ≥48 非全同 base64 体——SA
///    JSON 模板骨架（`"private_key": "-----BEGIN…\nXXXX…"`）；
/// 2. 模板语法对（`${…}` / `<…>`）；
/// 3. SCREAMING_SNAKE 占位（`YOUR_PASSWORD_HERE` / `CHANGEME`）；
/// 4. 全同字符（`"00000000"` / `"________"`）；
/// 5. 小写教程复合词表（`"your-password"`，既有词表）。
///
/// 真实口令（混排/含数字/非词表）照旧拦截——防修过头对照见 fixture。
pub(crate) fn is_placeholder_secret_value(value: &str) -> bool {
    if value.contains("-----BEGIN") && !has_real_base64_run(value, 48) {
        return true;
    }
    is_template_placeholder_pair(value)
        || is_screaming_placeholder(value)
        || is_uniform_char_value(value)
        || is_tutorial_compound_value(value)
}

// ─────────────────────────────────────────────────────────────────────────
// 数据表：规则 × 值段提取 × 判据
// ─────────────────────────────────────────────────────────────────────────

/// 值段提取方式：判定必须落在值本体，不能落在含键名/前缀的整段。
#[derive(Clone, Copy)]
enum Extract {
    /// 匹配段整段（判定谓词自身处理结构，如 db/basic_auth/example/常量）。
    WholeMatch,
    /// 空白尾段（`split_whitespace().last()`）。
    WhitespaceTail,
    /// 空白尾段剔 `-`（GUID 形态全同判定：`00000000-0000-…` 剔连字符后
    /// 判全同）。
    WhitespaceTailStripDash,
    /// 分隔符尾段（`rsplit(sep).next()`；sep 必须不出现在值字符类内）。
    TailAfter(&'static [char]),
    /// 首个字面量后段（`split_once(lit).1`；前缀由 regex 锚定在段首）。
    AfterLiteral(&'static str),
    /// 末对引号内值（JSON 键值对 / 赋值引号值共用）。
    QuotedTail,
    /// 不提取值段——PEM 窗口闸（marker 后 300 字符）。
    PemWindow,
    /// 不提取值段——整体内容闸（gcp 上下文）。
    WholeContent,
}

/// 豁免判据谓词。
enum Pred {
    /// vault 引用重扫臂（特殊：需要规则 regex，strip 后重扫同规则）。
    VaultRescan,
    /// 全同字符占位符。
    UniformChar,
    /// 全同字符占位符（剔连字符后判——GUID 形态全零/全同）。
    UniformCharIgnoringDashes,
    /// SCREAMING_SNAKE 占位符。
    ScreamingPlaceholder,
    /// 散文值（无数字且非混排）。
    ProseToken,
    /// 匹配段含 "example"（ci）。
    ContainsExample,
    /// 精确知名文档常量（子串匹配；真实凭据与常量逐字节相等的概率为零）。
    KnownConstant(&'static str),
    /// db 连接串占位符（SCREAMING + 教程词 + 模板字符 + 空段）。
    DbPlaceholder,
    /// Basic RFC 7617 结构闸（解码失败/无冒号/教程 pass）。
    Rfc7617Basic,
    /// secret 引号值复合豁免（PEM 臂 + 模板对 + SCREAMING + 全同 + 词表）。
    TutorialSecretValue,
    /// PEM 格式示意（窗口无真实 base64 体）。
    PemIllustration,
    /// SA JSON 模板骨架（整体内容上下文闸）。
    TemplateServiceAccount,
}

/// 一行豁免：`规则 × 提取 × 判据`。`rules` 空 = 全规则（仅 vault 行）。
struct Exemption {
    rules: &'static [&'static str],
    extract: Extract,
    pred: Pred,
    /// 注记（表内自文档：哪批横扫、机理一句话）。仅文档用途，运行期
    /// 不读——豁免行为完全由 rules/extract/pred 三元组决定。
    #[allow(dead_code)]
    why: &'static str,
}

/// 全部 L4 豁免（单一真相源）。行间独立，任一行判 true 即豁免——与旧
/// if 链逐条 continue 语义等价；消费入口 [`is_exempt`]。
static EXEMPTIONS: &[Exemption] = &[
    // ── vault 引用（全规则；strip 后对同一规则重扫，仍命中才算真命中）──
    Exemption {
        rules: &[],
        extract: Extract::WholeMatch,
        pred: Pred::VaultRescan,
        why: "vault 引用原文 ≠ 明文秘密（2026-11-10 B4 批）",
    },
    // ── db 连接串占位符 ──
    Exemption {
        rules: &[
            "db_connection_mysql",
            "db_connection_postgres",
            "db_connection_mongodb",
            "db_connection_redis",
        ],
        extract: Extract::WholeMatch,
        pred: Pred::DbPlaceholder,
        why: "教程 canonical / 模板 / SCREAMING / dev 默认词（2026-11-11 横扫批）",
    },
    // ── basic_auth RFC 7617 结构闸 ──
    Exemption {
        rules: &["basic_auth"],
        extract: Extract::WholeMatch,
        pred: Pred::Rfc7617Basic,
        why: "散文长词 / base64(user:password) canonical（2026-11-11 二次横扫批）",
    },
    // ── 散文值族 ──
    Exemption {
        rules: &["bearer_token"],
        extract: Extract::WhitespaceTail,
        pred: Pred::ProseToken,
        why: "token-based-authentication-scheme-here 类散文（三次横扫批）",
    },
    Exemption {
        rules: &["api_key_in_url", "npm_token"],
        extract: Extract::TailAfter(&['=']),
        pred: Pred::ProseToken,
        why: "URL/npm 值段散文占位（三次横扫批）",
    },
    Exemption {
        rules: &["github_token"],
        extract: Extract::AfterLiteral("_"),
        pred: Pred::ProseToken,
        why: "ghp_ 后段散文占位（三次横扫批；首个 `_` = split_once 语义）",
    },
    Exemption {
        rules: &["netlify_token"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::ProseToken,
        why: "全大写散文占位（四次横扫批）",
    },
    Exemption {
        rules: &["slack_token"],
        extract: Extract::TailAfter(&['-']),
        pred: Pred::ProseToken,
        why: "末段全大写词占位（五批矩阵：xoxb-…-YOURSLACKTOKEN…）",
    },
    Exemption {
        rules: &["stripe_key"],
        extract: Extract::TailAfter(&['_']),
        pred: Pred::ProseToken,
        why: "小写词形占位（五批矩阵）",
    },
    Exemption {
        rules: &["sendgrid_key"],
        extract: Extract::TailAfter(&['.']),
        pred: Pred::ProseToken,
        why: "末段小写词形占位（五批矩阵）",
    },
    Exemption {
        rules: &["discord_bot_token"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::ProseToken,
        why: "值段散文占位（五批矩阵：your-discord-bot-token-… / 全大写词）",
    },
    // ── example 文档示例族 ──
    Exemption {
        rules: &["aws_access_key", "aws_secret_key", "aws_session_token"],
        extract: Extract::WholeMatch,
        pred: Pred::ContainsExample,
        why: "AWS 官方 canonical 含 EXAMPLE（三次横扫批）",
    },
    Exemption {
        rules: &["api_key_in_url"],
        extract: Extract::WholeMatch,
        pred: Pred::ContainsExample,
        why: "EXAMPLE_KEY_1234567890 含 example 即占位（五批矩阵）",
    },
    // ── 全同字符族 ──
    Exemption {
        rules: &["twilio_sid"],
        extract: Extract::AfterLiteral("AC"),
        pred: Pred::UniformChar,
        why: "文档 canonical 32 连 x（四次横扫批）",
    },
    Exemption {
        rules: &["mailgun_key"],
        extract: Extract::AfterLiteral("key-"),
        pred: Pred::UniformChar,
        why: "文档 canonical 32 连 x（四次横扫批）",
    },
    Exemption {
        rules: &["azure_tenant_id"],
        extract: Extract::WhitespaceTailStripDash,
        pred: Pred::UniformChar,
        why: "Microsoft 文档 canonical 全零 GUID（四次横扫批）",
    },
    Exemption {
        rules: &["aws_access_key"],
        extract: Extract::AfterLiteral("AKIA"),
        pred: Pred::UniformChar,
        why: "AKIA+全零/全同 ×16（五批矩阵）",
    },
    Exemption {
        rules: &["aws_secret_key", "aws_session_token"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::UniformChar,
        why: "值段全零/全同 ×40/×120（五批矩阵）",
    },
    Exemption {
        rules: &["gcp_private_key_id"],
        extract: Extract::QuotedTail,
        pred: Pred::UniformChar,
        why: "全零/全同 hex ×16+（五批矩阵）",
    },
    Exemption {
        rules: &["azure_connection_string"],
        extract: Extract::AfterLiteral("AccountKey="),
        pred: Pred::UniformChar,
        why: "AccountKey 全零/全同 ×64（五批矩阵）",
    },
    Exemption {
        rules: &["github_oauth"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::UniformChar,
        why: "全零/全同 hex ×40（五批矩阵）",
    },
    Exemption {
        rules: &["heroku_key"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::UniformCharIgnoringDashes,
        why: "全零/全同 UUID ×36（五批矩阵；GUID 连字符不参与全同判定）",
    },
    Exemption {
        rules: &["stripe_key"],
        extract: Extract::TailAfter(&['_']),
        pred: Pred::UniformChar,
        why: "sk_test_+全同 ×24（五批矩阵）",
    },
    Exemption {
        rules: &["discord_bot_token"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::UniformChar,
        why: "值段全同 ×50（五批矩阵）",
    },
    // ── SCREAMING_SNAKE 族（五批矩阵收敛）──
    Exemption {
        rules: &["aws_access_key"],
        extract: Extract::AfterLiteral("AKIA"),
        pred: Pred::ScreamingPlaceholder,
        why: "AKIA+YOURACCESSKEY 类大写词占位（五批矩阵）",
    },
    Exemption {
        rules: &["aws_secret_key", "aws_session_token"],
        extract: Extract::TailAfter(&['=', ':']),
        pred: Pred::ScreamingPlaceholder,
        why: "值段 YOUR_SECRET… 类大写词占位（五批矩阵）",
    },
    Exemption {
        rules: &["azure_connection_string"],
        extract: Extract::AfterLiteral("AccountKey="),
        pred: Pred::ScreamingPlaceholder,
        why: "AccountKey 大写词占位（五批矩阵）",
    },
    Exemption {
        rules: &["stripe_key"],
        extract: Extract::TailAfter(&['_']),
        pred: Pred::ScreamingPlaceholder,
        why: "sk_test_+YOURSTRIPEKEY 类大写占位（五批矩阵）",
    },
    // ── secret_assignment 引号值复合 ──
    Exemption {
        rules: &["secret_assignment"],
        extract: Extract::QuotedTail,
        pred: Pred::TutorialSecretValue,
        why: "引号值占位五臂：PEM 示意 / 模板对 / SCREAMING / 全同 / 词表（三次横扫批基线 + 五批扩臂）",
    },
    // ── gcp / PEM 上下文闸 ──
    Exemption {
        rules: &["gcp_service_account"],
        extract: Extract::WholeContent,
        pred: Pred::TemplateServiceAccount,
        why: "SA JSON 模板骨架（键材占位；三次横扫批）",
    },
    Exemption {
        rules: &[
            "private_key",
            "ec_private_key",
            "dsa_private_key",
            "pgp_private_key",
        ],
        extract: Extract::PemWindow,
        pred: Pred::PemIllustration,
        why: "marker 后无真实（非全同）base64 体 = 格式示意（三次横扫批；五批收窄非全同）",
    },
    // ── 知名文档常量精确豁免（五批；用户拍板的「针对特定字符串豁免」）──
    Exemption {
        rules: &["azure_connection_string"],
        extract: Extract::WholeMatch,
        pred: Pred::KnownConstant(AZURE_DOC_ACCOUNT_KEY),
        why: "Azure 官方文档示例 AccountKey（§7.3 挂账 → 机制化精确豁免）",
    },
    Exemption {
        rules: &["stripe_key"],
        extract: Extract::WholeMatch,
        pred: Pred::KnownConstant(STRIPE_DOCS_TEST_KEY),
        why: "Stripe 官方测试 key（§7.3 挂账 → 机制化精确豁免）",
    },
    Exemption {
        rules: &["jwt_token"],
        extract: Extract::WholeMatch,
        pred: Pred::KnownConstant(JWT_IO_DEMO_SIGNATURE),
        why: "jwt.io demo token 签名段（全球 JWT 教程 canonical）",
    },
];

/// 谓词额外判据：全同（忽略连字符）——GUID 形态全零/全同。
fn is_uniform_char_ignoring_dashes(value: &str) -> bool {
    is_uniform_char_value(&value.replace('-', ""))
}

/// 豁免求值上下文：`full` = 规则完整匹配；`window` = PEM 窗口闸源
///（scan = content / redact = 当前累积串，match_end 偏移与被扫串对齐）；
/// `whole` = 整体内容闸源（gcp 上下文闸恒用原始 content，不随前序规则
/// 打码漂移——与旧钩子链语义逐点对齐）。
pub(crate) struct ExemptionCtx<'a> {
    pub rule: &'a str,
    pub full: &'a str,
    pub window: &'a str,
    pub whole: &'a str,
    pub match_end: usize,
    pub re: &'a Regex,
}

/// 豁免求值单一入口（Layer 4 scan / redact 两面共用）：遍历豁免表，
/// 规则匹配的行任一判 true 即豁免。vault 行需要规则 regex 做 strip 重扫。
pub(crate) fn is_exempt(ctx: &ExemptionCtx) -> bool {
    for e in EXEMPTIONS {
        if !e.rules.is_empty() && !e.rules.contains(&ctx.rule) {
            continue;
        }
        if evaluate(e, ctx) {
            return true;
        }
    }
    false
}

fn evaluate(e: &Exemption, ctx: &ExemptionCtx) -> bool {
    use std::borrow::Cow;
    // 值段提取（PemWindow/WholeContent/VaultRescan 不提取）。
    let value: Option<Cow<'_, str>> = match e.extract {
        Extract::WholeMatch => Some(Cow::Borrowed(ctx.full)),
        Extract::WhitespaceTail => ctx.full.split_whitespace().last().map(Cow::Borrowed),
        Extract::WhitespaceTailStripDash => ctx
            .full
            .split_whitespace()
            .last()
            .map(|s| Cow::Owned(s.replace('-', ""))),
        // `key = VALUE` 形态分隔符后带空格——trim 后再判（空格不在任何
        // 值判据字符集内，不 trim 则 SCREAMING/prose 判定恒 false）。
        Extract::TailAfter(sep) => Some(Cow::Borrowed(
            ctx.full.rsplit(sep).next().unwrap_or("").trim(),
        )),
        Extract::AfterLiteral(lit) => ctx
            .full
            .split_once(lit)
            .map(|(_, tail)| Cow::Borrowed(tail)),
        Extract::QuotedTail => extract_quoted_tail(ctx.full).map(Cow::Borrowed),
        Extract::PemWindow | Extract::WholeContent => None,
    };
    match e.pred {
        Pred::VaultRescan => {
            ctx.full.contains(nemesis_config::vault_ref::VAULT_PREFIX)
                && !ctx
                    .re
                    .is_match(&nemesis_config::vault_ref::strip_reference_forms(ctx.full))
        }
        Pred::UniformChar => value.is_some_and(|v| is_uniform_char_value(&v)),
        Pred::UniformCharIgnoringDashes => {
            value.is_some_and(|v| is_uniform_char_ignoring_dashes(&v))
        }
        Pred::ScreamingPlaceholder => value.is_some_and(|v| is_screaming_placeholder(&v)),
        Pred::ProseToken => value.is_some_and(|v| is_prose_token_value(&v)),
        Pred::ContainsExample => is_canonical_doc_example(ctx.full),
        Pred::KnownConstant(c) => ctx.full.contains(c),
        Pred::DbPlaceholder => is_placeholder_db_credential(ctx.full),
        Pred::Rfc7617Basic => is_placeholder_basic_auth(ctx.full),
        Pred::TutorialSecretValue => value.is_some_and(|v| is_placeholder_secret_value(&v)),
        Pred::PemIllustration => is_pem_illustration(ctx.window, ctx.match_end),
        Pred::TemplateServiceAccount => is_template_service_account_json(ctx.whole),
    }
}

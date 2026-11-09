//! `vault:<alias>` 引用解析钩子（vault 计划，2026-09-22 计划）。
//!
//! 依赖环约束：vault 的加密实现住在 nemesis-security，而 nemesis-security
//! 依赖本 crate（nemesis-config）——所以本 crate **不能**反向依赖它。约定：
//!
//! - 本模块只持有前缀常量 + 全局解析器槽位（`set_global_vault_resolver`）；
//! - 上层装配（nemesisbot 启动路径，security feature）把"别名 → secret"
//!   的解析闭包注入进来；
//! - 所有 `vault:` 消费点（provider api_key、channel token、cluster token、
//!   MCP env/headers）经 [`resolve_vault_reference`] / [`resolve_secret_field`]
//!   走同一槽位。
//!
//! 语义与 `env:` / `yaml:` 一致：引用解析**每次现查、无缓存**（vault 轮换
//! 下次解析即生效）；解析失败 fail loud 带补救指引，绝不静默降级为空值。

use crate::{ConfigError, Result};
use parking_lot::RwLock;
use std::sync::Arc;

/// `vault:` 引用前缀。配置中写作 `vault:<alias>`（如 `vault:openai-main`）。
pub const VAULT_PREFIX: &str = "vault:";

/// vault 别名解析函数：alias -> secret，或人类可读错误（别名不存在 /
/// vault 锁定 / 打不开）。由上层装配注入；实现须无阻塞交互（无 TTY 提示）。
pub type VaultResolver = Arc<dyn Fn(&str) -> std::result::Result<String, String> + Send + Sync>;

static GLOBAL_VAULT_RESOLVER: RwLock<Option<VaultResolver>> = RwLock::new(None);

/// 安装全局 vault 解析器（幂等：后装覆盖先装）。测试用
/// [`clear_global_vault_resolver`] 复位。
pub fn set_global_vault_resolver(resolver: VaultResolver) {
    *GLOBAL_VAULT_RESOLVER.write() = Some(resolver);
}

/// Reset the global resolver (test hygiene, mirrors credentials module).
pub fn clear_global_vault_resolver() {
    *GLOBAL_VAULT_RESOLVER.write() = None;
}

/// 当前全局解析器（诊断/测试用）。
pub fn global_vault_resolver() -> Option<VaultResolver> {
    GLOBAL_VAULT_RESOLVER.read().clone()
}

/// vault 别名写入函数：`(裸别名, 值)` -> Ok / 人类可读错误。裸别名 =
/// 不带 `vault:` 前缀（`agent/<会话>/<名>` 形态）。与解析器同约束：
/// 实现须无阻塞交互（无 TTY 提示）；**值不落日志、不落错误串**是实现的
/// 纪律（本 crate 的错误透传实现方给的文案，不二次加工）。
pub type VaultWriter = Arc<dyn Fn(&str, &str) -> std::result::Result<(), String> + Send + Sync>;

static GLOBAL_VAULT_WRITER: RwLock<Option<VaultWriter>> = RwLock::new(None);

/// 安装全局 vault 写入器（幂等：后装覆盖先装）。B4 会话凭据收取
///（question 凭据形态）用——nemesis-agent 无法依赖 vault 实现层
///（依赖环同解析器），走同一「全局槽 + 上层装配注入」模式。
/// 测试用 [`clear_global_vault_writer`] 复位。
pub fn set_global_vault_writer(writer: VaultWriter) {
    *GLOBAL_VAULT_WRITER.write() = Some(writer);
}

/// Reset the global writer (test hygiene, mirrors the resolver slot).
pub fn clear_global_vault_writer() {
    *GLOBAL_VAULT_WRITER.write() = None;
}

/// 当前全局写入器（诊断/测试用）。
pub fn global_vault_writer() -> Option<VaultWriter> {
    GLOBAL_VAULT_WRITER.read().clone()
}

/// 写入裸别名凭据（question 凭据形态消费点）。`Err` = 未装写入器（进程
/// 未装配 vault 写面）或写入失败——调用方必须向上传播，不得静默。
/// 值只进 vault，不落日志。
pub fn write_vault_secret(bare_alias: &str, value: &str) -> std::result::Result<(), String> {
    let writer = GLOBAL_VAULT_WRITER.read().clone();
    match writer {
        Some(w) => w(bare_alias, value),
        None => Err(format!(
            "本进程未安装 vault 写入通道，无法接收凭据 \
            （headless/通道形态不支持凭据索要；请在有 Dashboard 的 gateway 会话中收取，\
             或用 `nemesisbot vault set {bare_alias}` 预存）"
        )),
    }
}

/// 若 `value` 是 `vault:` 引用则解析之。
///
/// 返回 `None` = 不是 vault 引用（前缀不相交，调用方继续自己的链路）；
/// `Some(Err)` = 是引用但解析失败（解析器未安装 / 别名不存在 / vault
/// 锁定）——调用方必须把错误向上传播，不得静默当字面量处理。
pub fn resolve_vault_reference(value: &str) -> Option<std::result::Result<String, String>> {
    let alias = value.strip_prefix(VAULT_PREFIX)?;
    if alias.is_empty() {
        return Some(Err(
            "vault: 引用别名为空——写作 vault:<alias>（如 vault:openai-main），\
             或用 `nemesisbot vault set <alias>` 先写入"
                .to_string(),
        ));
    }
    let resolver = global_vault_resolver();
    match resolver {
        Some(r) => Some(r(alias)),
        None => Some(Err(format!(
            "检测到 vault:{alias} 引用，但本进程未安装 vault 解析器\
             （security feature 未启用，或非完整 nemesisbot 构建）"
        ))),
    }
}

/// `value` 是否为整值秘密引用（`env:` / `yaml:` / `vault:` 前缀）。
///
/// 前缀清单的唯一判定点：新增前缀时只改这里与 [`resolve_secret_field`]，
/// 消费方（如工作流模板层， 计划类 B）以本函数做整值识别，
/// 不自行维护前缀列表。
pub fn is_secret_reference(value: &str) -> bool {
    value.starts_with("env:") || value.starts_with("yaml:") || value.starts_with(VAULT_PREFIX)
}

/// 会话凭据模板起始标记（`{{vault:<alias>}}`）。单一真相源：安全层
/// （nemesis-security 凭据扫描 / DLP 的模板豁免）与 agent 侧展开
///（nemesis-agent `session_secrets`）共用本常量，防两套语法漂移。
pub const TEMPLATE_HEAD: &str = "{{vault:";
/// 会话凭据模板闭合标记。
pub const TEMPLATE_TAIL: &str = "}}";

/// 剔除字符串中全部**完整合法**的 `{{vault:<alias>}}` 模板子串。
///
/// [`strip_reference_forms`] 的模板半边（完整模板判定/剔除单一真相源）。
/// 消费方式见 [`strip_reference_forms`] 文档。
///
/// 从严判定（安全层不查库不触真值，只认语法）：
/// - 未闭合（`{{vault:x` 无 `}}`）→ 保留 `{{vault:` 原文继续扫——与
///   agent 侧展开语义一致（按字面量保留，非合法模板形态）；
/// - 别名 trim 后为空（`{{vault:   }}`）→ 保留原文（非法形态不豁免）；
/// - 别名段嵌套/畸形（如 `{{vault:{{vault:x}}` 内层提前闭合）→ 按最近
///   `}}` 闭合判定——该形态展开层会越界报错 fail loud，安全层放行的
///   是纯字面文本（无真值），无泄漏窗口。
pub fn strip_templates(s: &str) -> String {
    if !s.contains(TEMPLATE_HEAD) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let Some(head) = rest.find(TEMPLATE_HEAD) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..head]);
        let after = &rest[head + TEMPLATE_HEAD.len()..];
        let Some(close) = after.find(TEMPLATE_TAIL) else {
            // 未闭合：保留 HEAD 原文，从 HEAD 之后继续找（后续可能有合法模板）。
            out.push_str(TEMPLATE_HEAD);
            rest = after;
            continue;
        };
        if after[..close].trim().is_empty() {
            // 空别名：非法模板形态，原文保留（含 HEAD），从 HEAD 之后继续。
            out.push_str(TEMPLATE_HEAD);
            rest = after;
            continue;
        }
        // 完整合法模板：剔除（不进输出）。
        rest = &after[close + TEMPLATE_TAIL.len()..];
    }
    out
}

/// 裸引用 token 的别名字符判定（`vault:` 之后的连续段）。
///
/// 正 charset：字母数字（含 Unicode——会话名经 sanitize_path_segment 可
/// 含 CJK）+ `_` `.` `/` `-`（别名段分隔与惯用连接符）。**遇空格/引号/
/// 花括号/其他标点即断段**：断点后的残余参与重扫（真秘密的标点残余
/// 照旧命中）；`vault:` 后无别名字符（空值 / `{{vault:}}` 闭合位）→
/// 非引用形态，原文保留（从严）。
fn is_alias_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
}

/// 剔除字符串中全部 vault **引用形态**——完整模板 `{{vault:<alias>}}` +
/// 裸引用 `vault:<alias>`——返回「假如这些引用都不存在」的文本。
///
/// ══ 为什么存在（B4 误拦修复 2026-11-10；裸形态用户拍板选 1）══
///
/// 安全层的明文秘密检测（nemesis-security 凭据扫描 `secret_assignment`
/// / DLP `secret_password_assignment`）按「关键词 + =/: + 8+ 字符值」的
/// 通用形态命中，而 vault 引用恰好长这样：
///
/// ```text
/// password = "{{vault:agent/s1/pw}}"   （会话模板——B4 误拦实锤）
/// password = "vault:zope/main"         （裸引用——配置文件合法写法）
/// password = vault:zope/main           （裸引用无引号——DLP 也命中）
/// ```
///
/// 别名串（无空格无引号 8+ 字符）在通用形态眼里与真密码不可区分。
/// 不豁免 = 写 vault 引用配置（vault 自身的文档化用法）、模型回显引用
/// 示例、插件日志引用原文，全被误拦/误打码。**不要删掉本豁免回到
/// 「命中即拦」**：那是误报回归不是安全加固——引用文本无真值，拦它
/// 防不了任何泄漏，只会打断合法流程。
///
/// ══ 已接受的代价（同一拍板）══
///
/// 裸形态豁免标准是「看起来像引用」而非「确定是引用」：真值形如
/// `vault:` + 别名字符的明文秘密（如 `vault:abc123456`）会绕过上述两条
/// **通用形态**规则。收窄措施：只认**独立词元**（`vault:` 前不能是别名
/// 字符——`myvault:x` 不豁免）；剥离后残余仍满足规则照旧拦
///（`vault:abc$%^12345` 剥到 `$%^12345` 仍命中）。固定结构规则（AKIA /
/// GitHub token / JWT 等 30+ 条）不受影响。残留风险仍受输出掩码 /
/// 审计链 / guardian 二审兜底。
///
/// ══ 未来出路（回查锚点）══
///
/// 当「数据式凭据通道」落地（deferred 问题三：凭据经 stdin 载荷/受控
/// 环境注入，不再以文本形态进工具参数），引用形态从源码面消失，本函数
/// 与全部豁免点可退役。届时按 `strip_reference_forms` 或「模板豁免」
/// 检索即达全部位置：nemesis-security `credential.rs`
///（scan_content / redact_content）+ `dlp.rs`（scan_text / redact_content
/// 静态+动态臂）。
///
/// 消费方式（安全层唯一姿势）：对命中的匹配文本 strip 后按**同一规则**
/// 重扫——仍命中才算真命中。模板半边的从严判定见 [`strip_templates`]。
pub fn strip_reference_forms(s: &str) -> String {
    let stripped = strip_templates(s);
    if !stripped.contains(VAULT_PREFIX) {
        return stripped;
    }
    let mut out = String::with_capacity(stripped.len());
    let mut rest = stripped.as_str();
    loop {
        let Some(pos) = rest.find(VAULT_PREFIX) else {
            out.push_str(rest);
            break;
        };
        // 独立词元闸：`vault:` 前是别名字符（如 `myvault:`）→ 不是引用
        // 形态，是普通单词的一部分——前缀与前置字符原样保留。
        let standalone = pos == 0
            || rest[..pos]
                .chars()
                .next_back()
                .is_none_or(|c| !is_alias_char(c));
        out.push_str(&rest[..pos]);
        let after = &rest[pos + VAULT_PREFIX.len()..];
        let alias_len: usize = after
            .chars()
            .take_while(|c| is_alias_char(*c))
            .map(char::len_utf8)
            .sum();
        if !standalone || alias_len == 0 {
            out.push_str(VAULT_PREFIX);
            rest = after;
        } else {
            // 独立 `vault:<alias>`：前缀与别名段一起剔除（别名段就落在
            // after 开头，跳 alias_len 字节）。
            rest = &after[alias_len..];
        }
    }
    out
}

/// 通用秘密字段解析：channel token / cluster token / MCP
/// env、headers 等字段共用。前缀链与 `resolve_api_key_value` 对齐：
/// `env:VAR` > `yaml:<alias>` > `vault:<alias>` > 字面量（向后兼容）。
///
/// `field_for_error` 是字段上下文（如 "channels.telegram.token"），仅进
/// 错误消息，不参与解析。
pub fn resolve_secret_field(raw: &str, field_for_error: &str) -> Result<String> {
    if let Some(var) = raw.strip_prefix("env:") {
        if var.is_empty() {
            return Err(ConfigError::Validation(format!(
                "{field_for_error}: env: 引用变量名为空——写作 env:VAR_NAME 或直接填字面量"
            )));
        }
        return match std::env::var(var) {
            Ok(v) if !v.is_empty() => Ok(v),
            Ok(_) => Err(ConfigError::Validation(format!(
                "{field_for_error}: 环境变量 '{var}' 已设置但为空"
            ))),
            Err(_) => Err(ConfigError::Validation(format!(
                "{field_for_error}: 环境变量 '{var}' 未设置"
            ))),
        };
    }
    if let Some(alias) = raw.strip_prefix("yaml:") {
        return crate::credentials::resolve_yaml_reference(alias, field_for_error);
    }
    if let Some(resolved) = resolve_vault_reference(raw) {
        return resolved
            .map_err(|msg| ConfigError::Validation(format!("{field_for_error}: {msg}")));
    }
    // 字面量原样通过（向后兼容：现有明文配置继续工作）。
    Ok(raw.to_string())
}

#[cfg(test)]
mod tests;

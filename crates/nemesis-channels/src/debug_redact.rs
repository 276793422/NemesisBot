//! SEC-002 Debug 脱敏共享件（BUG-20261111-068 横向排查）。
//!
//! 通道侧配置结构在 gateway 装配期持有**已解析**的明文凭据（config.json
//! 的 vault:/env:/yaml: 引用在本 crate 装配点解引用后的明文
//! token/password/private_key），derive(Debug) 会把明文凭据带进
//! Debug/panic/错误上下文日志（nemesis-config 侧 WebChannelConfig 的
//! SEC-002 审查注释明示的威胁模型；通道侧 telegram `redact_token` 的
//! reqwest 错误脱敏是同族先例，Debug 面此前漏做）。
//!
//! [`impl_redacted_debug!`] 生成手写 Debug：`secret` 列表输出
//! `<empty>/<redacted>`，`opt_secret` 列表处理 `Option<String>` 形态，
//! `plain` 列表原样输出（保留诊断价值）。输出字段顺序 = plain 先、
//! secret 后（无消费方依赖这些结构的 Debug 输出顺序）。
//!
//! 纪律：新增带凭据字段的通道配置结构必须走本宏（或手写 Debug——
//! wecom `webhooks` 路由表等 map 形态宏罩不住），不得 derive(Debug)；
//! 与 nemesis-config 侧同族宏同形独立（该侧 helper 是 pub(crate)，
//! 跨 crate 不可复用）。

/// 凭据字段的 Debug 形态：空值 `<empty>`、非空 `<redacted>`（诊断信息
/// 以 config.json / Dashboard 回显为准，日志不落明文凭据）。
pub(crate) fn debug_secret_value(v: &str) -> &'static str {
    if v.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

/// `Option<String>` 凭据字段：None / Some("") = 未配置 `<empty>`，
/// Some(非空) = `<redacted>`。
///
/// cfg 门：`opt_secret` 宏臂目前仅 email/irc/onebot 三个 feature 门模块
/// 使用（nemesisbot 默认通道集不含它们），default workspace build 下宏臂
/// 零展开、本函数只剩 `#[cfg(test)]` 引用——lib 目标视为 dead_code，
/// clippy `-D warnings` 门禁红（BUG-20261111-073）。新增 opt_secret 用户
/// 时必须把对应 feature 并入本 cfg 列表（漏并会在 feature-on 构建
/// 编译期报函数缺失，属可见失败，不会静默假成功）。
#[cfg(any(feature = "email", feature = "irc", feature = "onebot", test))]
pub(crate) fn debug_secret_option(v: Option<&str>) -> &'static str {
    match v {
        Some(s) if !s.is_empty() => "<redacted>",
        _ => "<empty>",
    }
}

macro_rules! impl_redacted_debug {
    ($ty:ident {
        plain { $($p:ident),* $(,)? }
        secret { $($s:ident),* $(,)? }
        $(opt_secret { $($o:ident),* $(,)? })?
    }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut st = f.debug_struct(stringify!($ty));
                $(
                    st.field(stringify!($p), &self.$p);
                )*
                $(
                    st.field(
                        stringify!($s),
                        &$crate::debug_redact::debug_secret_value(&self.$s),
                    );
                )*
                $(
                    $(
                        st.field(
                            stringify!($o),
                            &$crate::debug_redact::debug_secret_option(self.$o.as_deref()),
                        );
                    )*
                )?
                st.finish()
            }
        }
    };
}

pub(crate) use impl_redacted_debug;

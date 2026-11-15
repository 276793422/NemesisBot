//! SEC-002 Debug 脱敏共享件（BUG-20261111-068 家族 providers 侧补漏）。
//!
//! 本 crate 的 provider 配置结构在 gateway 装配点持有**已解析**的明文凭据
//! （config.json `model_list[].api_key` / `proxy` 经 FactoryConfig 透传到
//! 各 lane Config 的明文形态）——derive(Debug) 会把明文凭据带进
//! Debug/panic/错误上下文日志（与 nemesis-config / nemesis-channels 侧
//! SEC-002 审查注释明示的威胁模型同源；ModelConfig.api_key 侧已修，本侧
//! 是其下游接收面，BUG-20261111-068 横向排查当时的 40 结构未含 providers）。
//!
//! [`impl_redacted_debug!`] 生成手写 Debug：`secret` 列表输出
//! `<empty>/<redacted>`，`opt_secret` 列表处理 `Option<String>` 形态，
//! `secret_map` 列表处理 `HashMap<String, String>` 形态（空表原样无秘密、
//! 非空整体 `<redacted>`——headers 可带 Authorization 类私有凭据头，与
//! wecom `webhooks` 路由表同口径），`plain` 列表原样输出（保留诊断价值）。
//! 输出字段顺序 = plain 先、secret 后（无消费方依赖 Debug 输出顺序，
//! BUG-20261111-068 已确立该先例）。
//!
//! 纪律：新增带凭据字段的 provider 配置结构必须走本宏（或手写 Debug），
//! 不得 derive(Debug)；与 nemesis-config / nemesis-channels 侧同族宏同形
//! 独立（两侧 helper 均 pub(crate)，跨 crate 不可复用）。
//!
//! 本 crate 全结构恒编译（无 feature 门差异），helper 不需要 cfg 门。

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
pub(crate) fn debug_secret_option(v: Option<&str>) -> &'static str {
    match v {
        Some(s) if !s.is_empty() => "<redacted>",
        _ => "<empty>",
    }
}

/// `HashMap<String, String>` 凭据面字段（headers 等任意 KV——可携带
/// Authorization 类私有凭据头）：空表原样无秘密 `<empty>`，非空整体
/// `<redacted>`（不逐键判断——键名可预见而值不可见，整体脱敏保诊断下限）。
pub(crate) fn debug_secret_map(m: &std::collections::HashMap<String, String>) -> &'static str {
    if m.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

macro_rules! impl_redacted_debug {
    ($ty:ident {
        plain { $($p:ident),* $(,)? }
        secret { $($s:ident),* $(,)? }
        $(opt_secret { $($o:ident),* $(,)? })?
        $(secret_map { $($m:ident),* $(,)? })?
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
                $(
                    $(
                        st.field(
                            stringify!($m),
                            &$crate::debug_redact::debug_secret_map(&self.$m),
                        );
                    )*
                )?
                st.finish()
            }
        }
    };
}

pub(crate) use impl_redacted_debug;

//! 架构依赖矩阵门禁（扩展计划 T3）：把 crate 间依赖方向的分层纪律
//! 测试化——违规 = 测试失败 = CI 红。
//!
//! 用 toml 解析 workspace 各 crate 的 Cargo.toml，断言 crate 间依赖方向符合
//! 既定分层。规则集编码的是**当前真实分层**（先核实现状再立规）；
//! 新增依赖若违反规则，要么改代码要么显式修订规则并说明理由。
//!
//! 只看 `[dependencies]` 与 `[target.*.dependencies]`；dev-dependencies /
//! build-dependencies 不在架构分层约束内（测试/构建期反向引用是合法的）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 一条分层规则。
pub enum Rule {
    /// 零内部依赖（最底层 crate）。
    ZeroInternal,
    /// 内部依赖白名单（只允许列表内的 nemesis-* crate）。
    Only(&'static [&'static str]),
    /// 内部依赖黑名单（禁止出现列表内的 nemesis-* crate）。
    Forbidden(&'static [&'static str]),
}

pub fn rules() -> Vec<(&'static str, Rule)> {
    vec![
        // R1: 类型层是根，不依赖任何内部 crate。
        ("nemesis-types", Rule::ZeroInternal),
        // R2: 消息总线只认类型。
        ("nemesis-bus", Rule::Only(&["nemesis-types"])),
        // R3: agent 是引擎，不得反向依赖呈现层/通道层/产品层。
        (
            "nemesis-agent",
            Rule::Forbidden(&[
                "nemesis-web",
                "nemesis-channels",
                "nemesis-desktop",
                "nemesis-board",
            ]),
        ),
        // R4: provider 层只认类型 + 叶子工具 crate。T2a（扩展计划 D4，
        // 2026-09-24）：错误分类纯逻辑下沉 nemesis-utils（agent 的
        // nemesis-providers 是 dev-dep，生产代码不可引用——单一真相源
        // 只能落公共叶子），providers 保留薄富化委托。
        (
            "nemesis-providers",
            Rule::Only(&["nemesis-types", "nemesis-utils"]),
        ),
        // R5: 安全层不得依赖引擎/呈现/通道。
        (
            "nemesis-security",
            Rule::Forbidden(&["nemesis-agent", "nemesis-web", "nemesis-channels"]),
        ),
        // R6: 沙盒层不得依赖引擎/呈现。
        (
            "nemesis-sandbox",
            Rule::Forbidden(&["nemesis-agent", "nemesis-web"]),
        ),
        // R7: 通道层不得依赖引擎/呈现（出站靠 bus 消息，不走直接调用）。
        (
            "nemesis-channels",
            Rule::Forbidden(&["nemesis-agent", "nemesis-web"]),
        ),
    ]
}

/// 读一个 crate 的 `[dependencies]` + `[target.*.dependencies]` 里的内部
/// 依赖（nemesis-*）。键名即依赖名；重命名依赖取 `package` 字段。
pub fn internal_deps(crate_dir: &Path) -> Vec<String> {
    let raw = match std::fs::read_to_string(crate_dir.join("Cargo.toml")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let value: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    let mut collect = |table: &toml::Value| {
        if let Some(deps) = table.as_table() {
            for (key, val) in deps {
                let real = val
                    .get("package")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| key.clone());
                if real.starts_with("nemesis-") {
                    out.push(real);
                }
            }
        }
    };

    if let Some(deps) = value.get("dependencies") {
        collect(deps);
    }
    if let Some(targets) = value.get("target").and_then(|t| t.as_table()) {
        for target in targets.values() {
            if let Some(deps) = target.get("dependencies") {
                collect(deps);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 解析 workspace 成员清单（根 Cargo.toml 的 [workspace].members）。
pub fn workspace_members(root: &Path) -> Vec<String> {
    let raw = match std::fs::read_to_string(root.join("Cargo.toml")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let value: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    value
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// 规则评审：返回 (crate, 规则名, 违规明细) 列表；空 = 全绿。
pub fn evaluate(root: &Path) -> Vec<String> {
    let members = workspace_members(root);
    // member 路径 → crate 目录（成员路径即相对 root 的目录）。
    let mut dirs: BTreeMap<String, PathBuf> = BTreeMap::new();
    for m in &members {
        let dir = root.join(m);
        if dir.join("Cargo.toml").is_file() {
            let name = std::fs::read_to_string(dir.join("Cargo.toml"))
                .ok()
                .and_then(|raw| {
                    toml::from_str::<toml::Value>(&raw).ok().and_then(|v| {
                        v.get("package")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                });
            if let Some(name) = name {
                dirs.insert(name, dir);
            }
        }
    }

    let mut violations = Vec::new();
    for (crate_name, rule) in rules() {
        let Some(dir) = dirs.get(crate_name) else {
            violations.push(format!(
                "[R?] crate '{crate_name}' 不在 workspace（规则引用了不存在的 crate）"
            ));
            continue;
        };
        let deps = internal_deps(dir);
        match rule {
            Rule::ZeroInternal => {
                if !deps.is_empty() {
                    violations.push(format!("[R1] {crate_name} 必须零内部依赖，实际：{deps:?}"));
                }
            }
            Rule::Only(allowed) => {
                for d in &deps {
                    if !allowed.contains(&d.as_str()) {
                        violations.push(format!(
                            "[R] {crate_name} 依赖了白名单外的内部 crate '{d}'（白名单：{allowed:?}）"
                        ));
                    }
                }
            }
            Rule::Forbidden(banned) => {
                for d in &deps {
                    if banned.contains(&d.as_str()) {
                        violations.push(format!(
                            "[R] {crate_name} 依赖了被禁止的内部 crate '{d}'（黑名单：{banned:?}）"
                        ));
                    }
                }
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests;

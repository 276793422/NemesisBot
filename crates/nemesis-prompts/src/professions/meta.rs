//! 职能目录：slug 语法、内置目录、min_tier 元数据（集群专业职能框架）。
//!
//! 本模块是纯函数与静态文本（crate 零依赖纪律）：slug 校验、内置目录
//! 查询、tier 常量。tier 口径与 [`crate::subagents::role_tier`] 同源同值
//! （mini/normal/big 字符串），消费方负责与 ModelTier 的映射。
//!
//! slug 语法：`family[:spec]`，段字符集 `[a-z0-9_-]+`，如 `product`、
//! `dev:cpp`。裸 family 只来自内置目录；用户扩展职能必须是
//! `family:spec` 两段形态（工作区两级目录映射，文件名避开冒号）。
//! 匹配语义：**精确匹配，无继承**（`dev` 宣告不命中 `dev:cpp` 任务，
//! 反向亦然）。

/// 内置职能条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfessionMeta {
    /// slug（`family` 或 `family:spec`，小写）。
    pub slug: &'static str,
    /// 中文显示名（前端徽标/日志用）。
    pub name: &'static str,
    /// family 段（`dev:cpp` → `dev`）。
    pub family: &'static str,
    /// spec 段（裸 family 为 `None`）。
    pub spec: Option<&'static str>,
    /// 最低节点档（role_tier 口径）；用户自定义职能无此元数据。
    pub min_tier: &'static str,
    /// 典型工件路径（planner 方法论/未来评审感知的素材）。
    pub artifacts: &'static [&'static str],
    /// 职能契约正文（编译期嵌入）。
    pub contract: &'static str,
}

/// 内置职能目录（6 职能 + dev:cpp 样板）。
///
/// 单一真相源：planner 提示词的值域清单以字面文本写入
/// `crate::board::PLANNER_SYSTEM_PROMPT`（const 不能插值），由
/// nemesis-board 的 sync 测试逐 slug 钉住与本目录一致。
pub const CATALOG: &[ProfessionMeta] = &[
    ProfessionMeta {
        slug: "product",
        name: "产品经理",
        family: "product",
        spec: None,
        min_tier: "normal",
        artifacts: &["docs/prd.md"],
        contract: include_str!("contracts/product.md"),
    },
    ProfessionMeta {
        slug: "ui-design",
        name: "UI 设计",
        family: "ui-design",
        spec: None,
        min_tier: "normal",
        artifacts: &["design/ui-spec.md", "design/prototype.html"],
        contract: include_str!("contracts/ui_design.md"),
    },
    ProfessionMeta {
        slug: "architecture",
        name: "架构师",
        family: "architecture",
        spec: None,
        min_tier: "big",
        artifacts: &["docs/architecture.md"],
        contract: include_str!("contracts/architecture.md"),
    },
    ProfessionMeta {
        slug: "dev",
        name: "开发工程师",
        family: "dev",
        spec: None,
        min_tier: "normal",
        artifacts: &[],
        contract: include_str!("contracts/dev.md"),
    },
    ProfessionMeta {
        slug: "dev:cpp",
        // 显示名不带括号嵌套：spec 形态的执行定位行是
        // 「# 执行职能：{name}（{slug}）」（见 render），name 含括号会
        // 叠出双括号（开发工程师（C/C++）（dev:cpp）），也让机器核验
        // （首个（slug）括组）必须跳过 name 内层括号。
        name: "开发工程师·C/C++",
        family: "dev",
        spec: Some("cpp"),
        min_tier: "big",
        artifacts: &[],
        contract: include_str!("contracts/dev.md"),
    },
    ProfessionMeta {
        slug: "test-whitebox",
        name: "白盒测试开发",
        family: "test-whitebox",
        spec: None,
        min_tier: "normal",
        artifacts: &["docs/test-report-whitebox.md"],
        contract: include_str!("contracts/test_whitebox.md"),
    },
    ProfessionMeta {
        slug: "test-blackbox",
        name: "黑盒测试",
        family: "test-blackbox",
        spec: None,
        min_tier: "normal",
        artifacts: &["docs/test-report-blackbox.md"],
        contract: include_str!("contracts/test_blackbox.md"),
    },
];

/// slug 格式错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlugError {
    /// 空串。
    Empty,
    /// 超长（slug 整体 > 64 字符）。
    TooLong,
    /// 形态非法：段字符集 `[a-z0-9_-]+`、至多一个冒号、段非空。
    Malformed,
}

impl core::fmt::Display for SlugError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "职能 slug 为空"),
            Self::TooLong => write!(f, "职能 slug 超过 64 字符"),
            Self::Malformed => write!(
                f,
                "职能 slug 形态非法（要求 family[:spec]，小写字母/数字/下划线/连字符）"
            ),
        }
    }
}

/// slug 段合法性：`[a-z0-9_-]+`。
fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// 校验 slug 语法（宣告侧/解析侧/渲染侧三端共用）。
///
/// 合法形态：`family` 或 `family:spec`；段字符集 `[a-z0-9_-]+`；整体
/// ≤64 字符。不查目录（用户自定义职能合法但不内置）。
pub fn validate_slug(slug: &str) -> Result<(), SlugError> {
    if slug.is_empty() {
        return Err(SlugError::Empty);
    }
    if slug.len() > 64 {
        return Err(SlugError::TooLong);
    }
    match slug.split_once(':') {
        None => {
            if valid_segment(slug) {
                Ok(())
            } else {
                Err(SlugError::Malformed)
            }
        }
        Some((family, spec)) => {
            if valid_segment(family) && valid_segment(spec) {
                Ok(())
            } else {
                Err(SlugError::Malformed)
            }
        }
    }
}

/// 拆 slug 为 (family, spec)；语法不合法返回 `None`。
pub fn split_slug(slug: &str) -> Option<(&str, Option<&str>)> {
    validate_slug(slug).ok()?;
    Some(match slug.split_once(':') {
        Some((f, s)) => (f, Some(s)),
        None => (slug, None),
    })
}

/// 大小写归一：slug 统一小写比较（宣告侧手写 `Dev:CPP` 也能命中）。
pub fn normalize_slug(slug: &str) -> String {
    slug.trim().to_ascii_lowercase()
}

/// 查内置目录（大小写不敏感）。
pub fn find_builtin(slug: &str) -> Option<&'static ProfessionMeta> {
    let norm = normalize_slug(slug);
    CATALOG.iter().find(|m| m.slug == norm)
}

/// tier 门槛判定（matcher 硬条件 4 的纯函数内核）。
///
/// 节点 tier 已知（`Some`）且低于职能 `min_tier` → `false`（排除）；
/// 节点 tier 未知（`None`，含旧节点）→ fail-open 放行（D3）。节点输入
/// trim + 归一小写（peers.toml 手写 `BIG` 也能比对；min_tier 是内置常量
/// 恒小写）。
pub fn tier_allows(node_tier: Option<&str>, min_tier: &str) -> bool {
    let Some(node) = node_tier else {
        return true;
    };
    let node = crate::subagents::role_tier::rank(node.trim().to_ascii_lowercase().as_str());
    node >= crate::subagents::role_tier::rank(min_tier)
}

/// 一组职能的最低满足档位：各成员 `min_tier` 取 rank 最大值，映射回
/// 档位字符串（mini/normal/big）。空集 → `None`（无职能 = 无门槛推断）。
///
/// `cluster init` 未显式 `--tier` 时用它从声明职能推断节点档位默认值。
pub fn min_tier_for_set(slugs: &[String]) -> Option<String> {
    let max_rank = slugs
        .iter()
        .filter_map(|s| find_builtin(s))
        .map(|m| crate::subagents::role_tier::rank(m.min_tier))
        .max()?;
    Some(
        match max_rank {
            0 => "mini",
            1 => "normal",
            _ => "big",
        }
        .to_string(),
    )
}

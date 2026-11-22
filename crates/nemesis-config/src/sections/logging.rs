// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub llm: Option<LlmLogConfig>,
    pub general: Option<GeneralLogConfig>,
}

/// LLM 日志详细档位——值域单一真相源（typed 枚举根治 stringly 值域失配：
/// 出厂模板曾带 `"summary"`、皮肤编辑器 schema 曾只给 `['summary','full']`，
/// 值域在类型系统里只有两个合法值后，此类错值面从根上消失）。
/// `rename_all = "lowercase"`：Serialize 输出规范小写字面（`"full"` /
/// `"truncated"`），typed 保存回写与 CLI 值域一致（缺省派生会输出帕斯卡
/// 变体名，round-trip 会撞进别名/非法臂——单测钉住）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LlmDetailLevel {
    #[default]
    Full,
    Truncated,
}

impl LlmDetailLevel {
    /// 规范小写字面（Serialize 与 CLI 展示共用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            LlmDetailLevel::Full => "full",
            LlmDetailLevel::Truncated => "truncated",
        }
    }

    /// 校验+解析同一入口（CLI `log config --detail-level` 走这里，手写
    /// valid 表退役）。非法值 = Err（CLI loud 报错退出）；`"summary"`
    /// 在这里是非法值——别名只做加载期迁移（见 Deserialize），不是可设值。
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "full" => Ok(LlmDetailLevel::Full),
            "truncated" => Ok(LlmDetailLevel::Truncated),
            other => Err(format!(
                "Error: Invalid detail level '{}'. Valid: [\"full\", \"truncated\"]",
                other
            )),
        }
    }
}

// 自定义 Deserialize（值域闸门）：合法二值直解；`"summary"` 是出厂模板
// 曾承诺过、从未单独实现的档位——运行时一直是「非 truncated 全落 Full」，
// 兑现承诺 = 迁移到 Truncated（加载期 WARN 可见不静默）；其余非法值按
// 旧行为归 Full，同样从静默变 WARN。Serialize 恒写规范字面——typed 保存
// 全量重序列化时存量 `"summary"` 自愈回写 `"truncated"`，不留永久别名。
impl<'de> Deserialize<'de> for LlmDetailLevel {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "full" => Ok(LlmDetailLevel::Full),
            "truncated" => Ok(LlmDetailLevel::Truncated),
            "summary" => {
                tracing::warn!(
                    "logging.llm.detail_level=\"summary\" 是历史遗留档位（从未单独实现），已按 \"truncated\"（精简档）生效；下次保存配置时自动回写为 \"truncated\""
                );
                Ok(LlmDetailLevel::Truncated)
            }
            other => {
                tracing::warn!(
                    "logging.llm.detail_level=\"{}\" 非法（值域 full/truncated），按 \"full\"（全量档）生效",
                    other
                );
                Ok(LlmDetailLevel::Full)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub log_dir: String,
    #[serde(default = "default_detail_level")]
    pub detail_level: LlmDetailLevel,
    #[serde(default)]
    pub save_raw: bool,
}

// 手写 Default（不 derive）：`detail_level` 的 serde 缺省是 Full，
// derive 给空串（同 FsWatcherConfig/AgentsConfig 的坑——当前两 struct 均
// Option 包裹、反序列化路径暂不可达 derive，防御性收敛防未来加
// `#[serde(default)]` 时复发，BUG-20261107-014 同批）。
impl Default for LlmLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            log_dir: String::new(),
            detail_level: default_detail_level(),
            save_raw: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralLogConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub enable_console: bool,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: String,
}

// 手写 Default（不 derive）：`enabled`/`enable_console`/`level` 的 serde
// 缺省是 true/true/"INFO"，derive 给 false/false/""（同上，防御性收敛）。
impl Default for GeneralLogConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            enable_console: default_true(),
            level: default_log_level(),
            file: String::new(),
        }
    }
}

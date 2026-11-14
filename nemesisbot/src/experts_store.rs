//! 专家子代理资源层（experts）——目录即注册表。
//!
//! 布局（workspace 锚定）：
//! - `<workspace>/experts/<slug>/EXPERT.md`：TOML frontmatter（name/summary/
//!   tags/model/enabled/source/hired_from）+ 正文 = 专家系统提示词全文；
//! - `<workspace>/experts/_default.meta.json`：默认子代理显示名（仅名字，
//!   执行形态零改变——spawn 不点名时的兜底，出厂 `{"name": "默认子代理"}`
//!   由 WSAPI 首次改名落盘；文件缺席 = 回落 [`ExpertsStore::FALLBACK_DEFAULT_NAME`]）。
//!
//! 单一真相源 = 磁盘：每次读现扫（专家数量个位数，扫描成本可忽略），无
//! 内存缓存——WSAPI 编辑与 spawn 名册天然一致，无失效窗口。slug = 目录名
//! （[`nemesis_utils::sanitize::sanitize_path_segment`] 归一 + 纯符号名退化
//! 时间戳形态）；改名只动 frontmatter、目录名稳定，模型侧旧 slug 引用不失效。

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 专家完整条目（资源层形态：frontmatter + 提示词正文）。
#[derive(Debug, Clone)]
pub struct ExpertFull {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// 模型别名覆盖（model_list 条目；空 = 继承主 Agent）。
    pub model: String,
    pub enabled: bool,
    /// `manual`（Dashboard 手工创建）| `shop`（人格超市雇佣）。
    pub source: String,
    /// shop 来源的人格 id（manual 恒空）。
    pub hired_from: String,
    /// 专家系统提示词（整段替换主人格，需自足）。
    pub prompt: String,
}

/// 创建/更新入参（WSAPI 面；全部 Option = 部分更新语义）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ExpertUpsert {
    pub name: Option<String>,
    pub summary: Option<String>,
    pub tags: Option<Vec<String>>,
    pub model: Option<String>,
    pub enabled: Option<bool>,
    pub prompt: Option<String>,
}

/// EXPERT.md frontmatter（读侧）。
#[derive(Default, serde::Deserialize)]
struct ExpertFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    model: String,
    #[serde(default = "fm_true")]
    enabled: bool,
    #[serde(default = "fm_manual")]
    source: String,
    #[serde(default)]
    hired_from: String,
}

fn fm_true() -> bool {
    true
}
fn fm_manual() -> String {
    "manual".to_string()
}

/// frontmatter（写侧，字段顺序 = 声明序，toml 序列化保序）。
#[derive(serde::Serialize)]
struct FmOut<'a> {
    name: &'a str,
    summary: &'a str,
    tags: &'a [String],
    model: &'a str,
    enabled: bool,
    source: &'a str,
    hired_from: &'a str,
}

pub struct ExpertsStore {
    root: PathBuf,
    /// config.json 路径（2026-11-13 专家模式策略现读源；None = 策略面
    /// 缺席——handle() 的 policy 闭包回落 `ExpertsPolicy::default()`，
    /// 即模式关 + 降级开：无配置读取面时走保守诚实拒绝姿态）。
    config_path: Option<PathBuf>,
}

impl ExpertsStore {
    /// 默认子代理元数据文件（experts 根下的保留文件名；扫描跳过 `_` 前缀）。
    pub const DEFAULT_META_FILE: &'static str = "_default.meta.json";
    /// `_default.meta.json` 缺席/损坏时的默认子代理显示名。
    pub const FALLBACK_DEFAULT_NAME: &'static str = "默认子代理";
    /// 专家定义文件名（目录内唯一载荷）。
    pub const EXPERT_FILE: &'static str = "EXPERT.md";

    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.join("experts"),
            config_path: None,
        }
    }

    /// 注入 config.json 路径（专家模式策略 `agents.experts` 的现读源）。
    /// 装配点（gateway agent_build / headless run / acp）各带自家 home 的
    /// config 路径；不注入 = 策略走 Default（模式关——诚实拒绝姿态）。
    pub fn with_config_path(mut self, config_path: PathBuf) -> Self {
        self.config_path = Some(config_path);
        self
    }

    /// slug 守卫：slug 是目录名，防御直达——拒空串/保留名/路径形态。
    /// `_default` 不是专家（默认子代理走 default_name/set_default_name 面）。
    fn slug_guard(slug: &str) -> Result<(), String> {
        if slug.is_empty() {
            return Err("专家 slug 不能为空".to_string());
        }
        if slug == Self::DEFAULT_META_FILE || slug.starts_with('_') {
            return Err(format!("'{slug}' 是保留名，不是专家 slug"));
        }
        if slug.contains('/') || slug.contains('\\') || slug == "." || slug == ".." {
            return Err(format!("非法专家 slug '{slug}'"));
        }
        Ok(())
    }

    /// 名字 → slug。ASCII 可辨名直通 sanitize；中文名/纯符号名 sanitize
    /// 成下划线串（不可辨且易撞）——退化为 `expert-<millis>` 时间戳形态。
    fn slug_for_name(name: &str) -> String {
        let base = nemesis_utils::sanitize::sanitize_path_segment(name);
        let has_alnum = base.chars().any(|c| c.is_ascii_alphanumeric());
        if has_alnum && base != "_" {
            base
        } else {
            format!("expert-{}", chrono::Utc::now().timestamp_millis())
        }
    }

    /// 全量在册专家（按显示名不区分大小写排序——read_dir 顺序是平台
    /// 实现细节，显式排序消费）。
    pub fn list(&self) -> Vec<ExpertFull> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(slug) = path.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            if slug.starts_with('_') {
                continue;
            }
            if let Ok(full) = self.load(&slug) {
                out.push(full);
            }
        }
        out.sort_by_key(|e| e.name.to_lowercase());
        out
    }

    /// 按键读取（slug 或显示名，大小写不敏感——一律走 [`Self::lookup`]
    /// 解析成盘上真实 slug，不依赖文件系统大小写语义：NTFS 不敏感 /
    /// ext4 敏感，直拼路径在 Linux 上大小写错写即 miss）。
    pub fn get(&self, key: &str) -> Result<ExpertFull, String> {
        Self::slug_guard(key)?;
        self.lookup(key)
    }

    fn load(&self, slug: &str) -> Result<ExpertFull, String> {
        let path = self.root.join(slug).join(Self::EXPERT_FILE);
        let content = nemesis_utils::fsutil::read_config_string(&path)
            .map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
        let (fm_raw, prompt) = split_frontmatter(&content)?;
        let fm: ExpertFrontmatter =
            toml::from_str(fm_raw).map_err(|e| format!("EXPERT.md frontmatter 解析失败: {e}"))?;
        Ok(ExpertFull {
            slug: slug.to_string(),
            name: if fm.name.trim().is_empty() {
                slug.to_string()
            } else {
                fm.name.trim().to_string()
            },
            summary: fm.summary,
            tags: fm.tags,
            model: fm.model.trim().to_string(),
            enabled: fm.enabled,
            source: fm.source,
            hired_from: fm.hired_from,
            prompt,
        })
    }

    /// slug 或显示名查找（各不区分大小写精确匹配，slug 优先）。失败文案
    /// 自带在册名单——模型侧自纠面（诚实回灌，不猜）。
    pub fn lookup(&self, key: &str) -> Result<ExpertFull, String> {
        let key = key.trim();
        let all = self.list();
        let key_lc = key.to_lowercase();
        if let Some(hit) = all.iter().find(|e| e.slug.to_lowercase() == key_lc) {
            return Ok(hit.clone());
        }
        if let Some(hit) = all.iter().find(|e| e.name.to_lowercase() == key_lc) {
            return Ok(hit.clone());
        }
        Err(format!(
            "Unknown expert '{}'. Hired experts: {}. Omit `expert` for the default sub-agent, or call spawn with {{\"action\": \"list\"}} to refresh the roster.",
            key,
            self.names_summary_of(&all)
        ))
    }

    /// spawn 执行面解析（agent crate [`nemesis_agent::loop_tools::ExpertDispatch`]
    /// 契约）：lookup → 停用诚实拒绝 → 空提示词诚实拒绝 → prompt + model。
    pub fn dispatch(&self, key: &str) -> Result<nemesis_agent::loop_tools::ExpertDispatch, String> {
        let e = self.lookup(key)?;
        if !e.enabled {
            return Err(format!(
                "Expert '{}' is disabled (it stays defined but cannot be dispatched until re-enabled). Hired experts: {}.",
                e.name,
                self.names_summary_of(&self.list())
            ));
        }
        if e.prompt.trim().is_empty() {
            return Err(format!(
                "Expert '{}' has an empty prompt — edit the expert and fill in its system prompt before dispatching.",
                e.name
            ));
        }
        Ok(nemesis_agent::loop_tools::ExpertDispatch {
            slug: e.slug,
            name: e.name,
            prompt: e.prompt,
            model: e.model,
        })
    }

    fn names_summary_of(&self, all: &[ExpertFull]) -> String {
        if all.is_empty() {
            return "none".to_string();
        }
        all.iter()
            .map(|e| format!("{} ({})", e.slug, e.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 创建（slug 由名字归一；目录撞车诚实拒绝）。source 恒 `manual`。
    pub fn create(&self, up: &ExpertUpsert) -> Result<ExpertFull, String> {
        let name = up
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("专家名不能为空")?;
        let slug = Self::slug_for_name(name);
        // slug 守卫（BUG-20261114-016）：create 是唯一从自由文本名字派生
        // slug 的入口，slug_for_name 产出可能落保留域——① 用户名自带 `_`
        // 前缀（sanitize 白名单保留 `_`，前导下划线直通）；② 无辜名字撞
        // Windows 保留设备名（sanitize 出口中和插 `_` 前缀，"nul"→"_nul"）。
        // 这类 slug 落盘后 list() 跳过 `_` 目录 + get/update/remove/
        // set_enabled 全被 slug_guard 拒绝 = 不可见且不可管理的僵尸目录，
        // 再建同名只报「撞车」死路。hire 面在入口已有同款守卫（296 行），
        // create 漏挂——守卫前移到派生点。
        Self::slug_guard(&slug)?;
        let dir = self.root.join(&slug);
        if dir.exists() {
            return Err(format!(
                "专家目录已存在（slug '{slug}' 撞车）：换个名字，或直接编辑既有专家"
            ));
        }
        let full = ExpertFull {
            slug,
            name: name.to_string(),
            summary: up.summary.clone().unwrap_or_default(),
            tags: up.tags.clone().unwrap_or_default(),
            model: up.model.clone().unwrap_or_default().trim().to_string(),
            enabled: up.enabled.unwrap_or(true),
            source: "manual".to_string(),
            hired_from: String::new(),
            prompt: up.prompt.clone().unwrap_or_default(),
        };
        self.write(&full)?;
        Ok(full)
    }

    /// 超市雇佣落盘（幂等重雇覆盖：同 slug 目录直接重写）。source 恒
    /// `shop`，`hired_from` 记人格 id（解雇可从超市原记录再雇）。
    pub fn hire(
        &self,
        slug: &str,
        name: &str,
        summary: &str,
        tags: Vec<String>,
        prompt: String,
        hired_from: &str,
    ) -> Result<ExpertFull, String> {
        Self::slug_guard(slug)?;
        if name.trim().is_empty() {
            return Err("专家名不能为空".to_string());
        }
        if prompt.trim().is_empty() {
            return Err("专家提示词不能为空（人格 SOUL.md 缺失？）".to_string());
        }
        let full = ExpertFull {
            slug: slug.to_string(),
            name: name.trim().to_string(),
            summary: summary.to_string(),
            tags,
            model: String::new(),
            enabled: true,
            source: "shop".to_string(),
            hired_from: hired_from.to_string(),
            prompt,
        };
        self.write(&full)?;
        Ok(full)
    }

    /// 部分更新（slug/目录名稳定；改名只动 frontmatter）。键先经
    /// [`Self::lookup`] 解析成盘上真实 slug（大小写容错、FS 无关），
    /// 写回用解析后 slug——调用方传错大小写不会在 ext4 上改出平行目录。
    /// `_default` 不是专家——默认子代理改名走 [`Self::set_default_name`]。
    pub fn update(&self, key: &str, up: &ExpertUpsert) -> Result<ExpertFull, String> {
        Self::slug_guard(key)?;
        let mut cur = self
            .lookup(key)
            .map_err(|e| format!("专家 '{key}' 不存在或读取失败: {e}"))?;
        if let Some(n) = up.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            cur.name = n.to_string();
        }
        if let Some(s) = &up.summary {
            cur.summary = s.clone();
        }
        if let Some(t) = &up.tags {
            cur.tags = t.clone();
        }
        if let Some(m) = &up.model {
            cur.model = m.trim().to_string();
        }
        if let Some(en) = up.enabled {
            cur.enabled = en;
        }
        if let Some(p) = &up.prompt {
            cur.prompt = p.clone();
        }
        self.write(&cur)?;
        Ok(cur)
    }

    /// 启停（供给闸：停用 = 摘出名册 + 派发诚实拒绝；定义保留，可逆）。
    pub fn set_enabled(&self, slug: &str, enabled: bool) -> Result<ExpertFull, String> {
        self.update(
            slug,
            &ExpertUpsert {
                enabled: Some(enabled),
                ..Default::default()
            },
        )
    }

    /// 删除（解雇/手工删除同入口；超市原人格记录不动，可再雇）。
    /// 键经 lookup 解析成真实 slug（同 update，FS 无关）；删除目录走
    /// 韧性包装（Windows AV/索引器瞬时句柄是常态）。
    pub fn remove(&self, key: &str) -> Result<(), String> {
        Self::slug_guard(key)?;
        let resolved = self.lookup(key)?;
        let dir = self.root.join(&resolved.slug);
        if !dir.is_dir() {
            return Err(format!("专家 '{}' 目录缺席", resolved.slug));
        }
        if nemesis_utils::fsutil::remove_dir_all_resilient(&dir) {
            Ok(())
        } else {
            Err(format!(
                "删除专家目录失败（文件被占用？稍后重试）：{}",
                dir.display()
            ))
        }
    }

    /// 默认子代理显示名（`_default.meta.json` → 回落常量；损坏不炸）。
    pub fn default_name(&self) -> String {
        let path = self.root.join(Self::DEFAULT_META_FILE);
        nemesis_utils::fsutil::read_config_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("name")
                    .and_then(|n| n.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
            .unwrap_or_else(|| Self::FALLBACK_DEFAULT_NAME.to_string())
    }

    /// 默认子代理改名（默认卡唯一可编辑字段；出厂名 = 回落常量，首次
    /// 改名落盘）。
    pub fn set_default_name(&self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("默认子代理名不能为空".to_string());
        }
        std::fs::create_dir_all(&self.root).map_err(|e| format!("建 experts 目录失败: {e}"))?;
        let json = serde_json::json!({ "name": name }).to_string();
        nemesis_utils::write_file_atomic(
            &self.root.join(Self::DEFAULT_META_FILE).to_string_lossy(),
            json.as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写 {} 失败: {e}", Self::DEFAULT_META_FILE))
    }

    /// WSAPI `config_get` 投影：config `agents.experts` 段 flat 三键
    ///（与 [`Self::policy_now`] 同源现读；config 缺席 = Default 值）。
    pub fn mode_policy_json(&self) -> serde_json::Value {
        let p = self.policy_now();
        serde_json::json!({
            "enabled": p.enabled,
            "degrade": p.degrade,
            "notify": p.notify,
        })
    }

    /// WSAPI `config_set`：`agents.experts` 段部分更新（raw JSON patch
    /// 原子写——**不经 typed round-trip**：config.json 是运行时可能有非
    /// typed 键的活文档，最小 patch 写入把波及面钉在 experts 段；
    /// credentials reloader 先例）。校验先行（写前全验，不做部分写入）；
    /// 开启动作（enabled false→true）落结构化日志 = 用户显式 opt-in 的
    /// 后台记录（PLAN §零.1；审计链不扩——config 变更不入链是现状口径）。
    ///
    /// RMW 全程持 config 写串行锁（BUG-20261113-171）：与 gateway 进程内
    /// 其他 config.json 写者（配置页 update_config/update_raw_config、探针
    /// stale 标记、画像 sweep、sandbox/vm/cluster/插件段写）共用同一把锁，
    /// 无锁读旧快照回写会与并发写者交错丢更新（BUG-20261110-043 同族）。
    pub fn set_mode_policy(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let _serial = nemesis_config::config_write_lock();
        let Some(path) = &self.config_path else {
            return Err("experts 模式策略面未装配（本进程无 config 路径）".to_string());
        };
        // 校验先行：三键 boolean、未知键诚实拒绝（防拼写错静默无效）。
        if let Some(obj) = data.as_object() {
            for key in obj.keys() {
                if !matches!(key.as_str(), "enabled" | "degrade" | "notify") {
                    return Err(format!(
                        "未知字段 '{key}'（允许：enabled / degrade / notify）"
                    ));
                }
            }
        } else if !data.is_null() {
            return Err("参数需为 JSON 对象（enabled / degrade / notify，boolean）".to_string());
        }
        let mut patch: Vec<(&str, bool)> = Vec::new();
        for key in ["enabled", "degrade", "notify"] {
            if let Some(v) = data.get(key) {
                if v.is_null() {
                    continue;
                }
                let b = v
                    .as_bool()
                    .ok_or_else(|| format!("'{key}' 需要 boolean（收到 {v}）"))?;
                patch.push((key, b));
            }
        }
        if patch.is_empty() {
            return Err("无可更新字段（允许：enabled / degrade / notify，boolean）".to_string());
        }
        // raw JSON 读 + patch + 原子写。
        let mut raw: serde_json::Value = if path.exists() {
            let text = std::fs::read_to_string(path).map_err(|e| format!("读 config 失败: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("config JSON 解析失败: {e}"))?
        } else {
            serde_json::json!({})
        };
        if !raw.is_object() {
            return Err("config 顶层不是 JSON 对象".to_string());
        }
        let prev_enabled = raw
            .get("agents")
            .and_then(|a| a.get("experts"))
            .and_then(|e| e.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let agents = raw
            .as_object_mut()
            .expect("checked object")
            .entry("agents".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !agents.is_object() {
            return Err("config.agents 不是 JSON 对象".to_string());
        }
        let experts = agents
            .as_object_mut()
            .expect("checked object")
            .entry("experts".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !experts.is_object() {
            return Err("config.agents.experts 不是 JSON 对象".to_string());
        }
        for (key, b) in &patch {
            experts
                .as_object_mut()
                .expect("checked object")
                .insert((*key).to_string(), serde_json::Value::Bool(*b));
        }
        let pretty =
            serde_json::to_string_pretty(&raw).map_err(|e| format!("config 序列化失败: {e}"))?;
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), pretty.as_bytes(), 0o644)
            .map_err(|e| format!("写 config 失败: {e}"))?;
        // 结构化日志（含 opt-in 记录——用户裁定：开启动作后台记录）。
        let enabled = patch
            .iter()
            .find(|(k, _)| *k == "enabled")
            .map(|(_, v)| *v)
            .unwrap_or(prev_enabled);
        tracing::info!(
            target: "nemesis::experts",
            prev_enabled,
            enabled,
            degrade = patch.iter().find(|(k, _)| *k == "degrade").map(|(_, v)| *v),
            notify = patch.iter().find(|(k, _)| *k == "notify").map(|(_, v)| *v),
            "[experts] mode policy updated (agents.experts fresh-read生效, 无需重启)"
        );
        if !prev_enabled && enabled {
            tracing::info!(
                target: "nemesis::experts",
                "[experts] OPT-IN recorded: user explicitly enabled expert mode (agents.experts.enabled=false -> true)"
            );
        }
        Ok(self.mode_policy_json())
    }

    /// 专家模式策略现读（2026-11-13 总开关 + 自动降级）：config.json
    /// `agents.experts` 段 → 契约镜像 `ExpertsPolicy`。每次调用现读
    ///（fresh-read，spawn 调用稀疏，解析成本可忽略；运行中改键即时生效，
    /// 无需重启）。config 缺席/损坏 = 回落 `ExpertsPolicy::default()`
    ///（模式关 + 降级开——无可靠策略源时走保守姿态，读失败 WARN 一次
    /// 每调用）。映射是显式 fn 不是 serde（契约两侧字段同形，默认值
    /// 相等性由单测钉住）。
    fn policy_now(&self) -> nemesis_agent::loop_tools::ExpertsPolicy {
        let Some(path) = &self.config_path else {
            return nemesis_agent::loop_tools::ExpertsPolicy::default();
        };
        match nemesis_config::load_config(path) {
            Ok(cfg) => nemesis_agent::loop_tools::ExpertsPolicy {
                enabled: cfg.agents.experts.enabled,
                degrade: cfg.agents.experts.degrade,
                notify: cfg.agents.experts.notify,
            },
            Err(e) => {
                tracing::warn!(error = %e, "[experts] config read failed; falling back to default experts policy (mode off)");
                nemesis_agent::loop_tools::ExpertsPolicy::default()
            }
        }
    }

    /// agent crate 契约面（装配点单一注入：SpawnTool 拿它做 list/预检，
    /// spawn 闭包拿它取提示词执行——两条路径同源不漂移）。
    pub fn handle(self: &Arc<Self>) -> nemesis_agent::loop_tools::ExpertsHandle {
        let roster_store = Arc::clone(self);
        let roster = Arc::new(move || nemesis_agent::loop_tools::ExpertsRoster {
            experts: roster_store
                .list()
                .into_iter()
                .map(|e| nemesis_agent::loop_tools::ExpertRosterEntry {
                    slug: e.slug,
                    name: e.name,
                    summary: e.summary,
                    tags: e.tags,
                    enabled: e.enabled,
                })
                .collect(),
            default_name: roster_store.default_name(),
        });
        let resolve_store = Arc::clone(self);
        let resolve = Arc::new(move |key: &str| resolve_store.dispatch(key));
        let policy_store = Arc::clone(self);
        let policy = Arc::new(move || policy_store.policy_now());
        nemesis_agent::loop_tools::ExpertsHandle {
            roster,
            resolve,
            policy,
        }
    }

    fn write(&self, e: &ExpertFull) -> Result<(), String> {
        let dir = self.root.join(&e.slug);
        std::fs::create_dir_all(&dir).map_err(|err| format!("建专家目录失败: {err}"))?;
        let fm = FmOut {
            name: &e.name,
            summary: &e.summary,
            tags: &e.tags,
            model: &e.model,
            enabled: e.enabled,
            source: &e.source,
            hired_from: &e.hired_from,
        };
        let fm_str =
            toml::to_string_pretty(&fm).map_err(|err| format!("frontmatter 序列化失败: {err}"))?;
        let mut content = format!("---\n{fm_str}---\n\n{}", e.prompt);
        if !content.ends_with('\n') {
            content.push('\n');
        }
        nemesis_utils::write_file_atomic(
            &dir.join(Self::EXPERT_FILE).to_string_lossy(),
            content.as_bytes(),
            0o644,
        )
        .map_err(|err| format!("写 {} 失败: {err}", Self::EXPERT_FILE))
    }
}

/// EXPERT.md → (frontmatter TOML 块, 提示词正文)。围栏 = 行首 `---`：
/// 开头 `---\n` 开、下一个 `\n---` 行收（`\r\n` 容忍）。
pub(crate) fn split_frontmatter(content: &str) -> Result<(&str, String), String> {
    let body = content.trim_start_matches('\u{feff}');
    let after_open = body
        .strip_prefix("---")
        .ok_or("EXPERT.md 缺少 frontmatter 头（应以 --- 行开头）")?;
    let after_open = after_open
        .strip_prefix('\r')
        .and_then(|s| s.strip_prefix('\n'))
        .or_else(|| after_open.strip_prefix('\n'))
        .ok_or("frontmatter 开围栏格式错误（`---` 应独占一行）")?;
    let Some(idx) = after_open.find("\n---") else {
        return Err("frontmatter 缺少收围栏（\\n--- 行）".to_string());
    };
    let fm = &after_open[..idx];
    let after_close = &after_open[idx + 4..];
    // 正文两端 trim：前导剥分隔空行（write 落 `---\n\n{prompt}` 两换行，
    // 容错单换行手写形态）、尾端剥 write 补齐的换行——提示词 round-trip
    // 逐字节稳定（尾随空白对系统提示词是噪声）。
    let body = after_close.trim_start_matches(['\r', '\n']).trim_end();
    Ok((fm, body.to_string()))
}

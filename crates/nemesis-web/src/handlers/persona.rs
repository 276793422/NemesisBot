//! Persona handler — manage AI persona profiles with GitHub marketplace integration.
//!
//! Supports browsing, searching, previewing, and downloading personas from the
//! agency-agents GitHub repository. Downloaded agents are converted from single
//! .md files into the NemesisBot persona format (IDENTITY.md, SOUL.md, AGENT.md,
//! TOOLS.md).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::handlers::{get_str, require_workspace, resolve_path};
use crate::ws_router::{ModuleHandler, RequestContext};
use futures::StreamExt as _;

// Constants

const PERSONA_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "AGENT.md", "TOOLS.md"];

/// Files that are archived/restored during persona switch (in addition to PERSONA_FILES).
const PERSONA_ARCHIVE_FILES: &[&str] = &["HEARTBEAT.md"];

/// Directories that are archived/restored during persona switch.
const PERSONA_ARCHIVE_DIRS: &[&str] = &["memory", "sessions", "logs/session_logs"];
const GITHUB_API: &str = "https://api.github.com";
const AGENCY_REPO: &str = "msitarzewski/agency-agents";
const AGENCY_BRANCH: &str = "main";

/// System default AGENT.md content — embedded at compile time.
const DEFAULT_AGENT_MD: &str = include_str!("../../../../nemesisbot/workspace/AGENT.md");
/// System default TOOLS.md content — embedded at compile time.
const DEFAULT_TOOLS_MD: &str = include_str!("../../../../nemesisbot/workspace/TOOLS.md");

/// Directories in agency-agents that are NOT agent categories (skip entirely).
const SKIP_DIRS: &[&str] = &["scripts", "examples", "integrations", "strategy", ".github"];

/// Filenames to skip even inside valid category directories.
const SKIP_FILENAMES: &[&str] = &[
    "README.md",
    "CONTRIBUTING.md",
    "LICENSE.md",
    "SECURITY.md",
    "CONTRIBUTING_zh-CN.md",
];

/// Browser-like User-Agent for GitHub API requests.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

// Shop sources & curation（2026-11-15 多源模型 + 策展推荐）

/// 默认目录源 key（上游 GitHub 仓库 [`AGENCY_REPO`]）。裸 id / 缺省
/// `source` 参数都落这里（向后兼容：现行前端零改动）。
const DEFAULT_SOURCE_KEY: &str = "agency";

/// 本地伪源 key：编译期内嵌的工作区人格模板（自建内置人格），见
/// [`BUILTIN_PERSONAS_DIR`]。
const BUILTIN_SOURCE_KEY: &str = "builtin";

/// 自家仓库（builtin 目录源 live 臂，2026-11-15 拍板②）：本次提交后内置
/// 人格就在 GitHub 上自家仓库里，加内置人格 = 往仓库
/// `nemesisbot/workspace/personas/` 提交一个人格目录，客户端不重编即可见
/// 可装；编译期内嵌模板降级为离线兜底（与 agency「在线→快照」降级链同构）。
const BUILTIN_REPO: &str = "276793422/NemesisBot";
const BUILTIN_BRANCH: &str = "main";
const BUILTIN_PATH: &str = "nemesisbot/workspace/personas";

/// 目录源注册表（编译期 const 单一真相源；加源 = 注册表加一项，运行期
/// 用户自加源已拍板本期不做）。`path` = 仓库内抓取子树（agency = 仓库根），
/// `label` = 人读展示名。
struct ShopCatalogSource {
    key: &'static str,
    repo: &'static str,
    branch: &'static str,
    path: &'static str,
    label: &'static str,
}

const SHOP_SOURCES: &[ShopCatalogSource] = &[
    ShopCatalogSource {
        key: DEFAULT_SOURCE_KEY,
        repo: AGENCY_REPO,
        branch: AGENCY_BRANCH,
        path: "",
        label: "Agency Agents",
    },
    ShopCatalogSource {
        key: BUILTIN_SOURCE_KEY,
        repo: BUILTIN_REPO,
        branch: BUILTIN_BRANCH,
        path: BUILTIN_PATH,
        label: "内置人格",
    },
];

fn shop_source(key: &str) -> Option<&'static ShopCatalogSource> {
    SHOP_SOURCES.iter().find(|s| s.key == key)
}

/// 编译期内嵌的工作区人格模板（builtin 伪源数据面，B2 送达口径）：
/// 模板提取只发生在 onboard（老工作区拿不到新模板文件），builtin 列表
/// 直接读这里，安装/雇佣时按需释放到工作区。发行物 = raw.md（创作单一
/// 真相源）+ 转换管线开发期生成的四件套/PERSONA.json + HEARTBEAT.md。
static BUILTIN_PERSONAS_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../nemesisbot/workspace/personas");

/// 内置人格 id 列表（内嵌模板顶层子目录名；显式排序——顺序是平台实现
/// 细节，消费侧不依赖字典序以外的次序）。
fn builtin_persona_ids() -> Vec<String> {
    let mut ids: Vec<String> = BUILTIN_PERSONAS_DIR
        .dirs()
        .filter_map(|d| {
            d.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
        })
        .collect();
    ids.sort();
    ids
}

/// 内置人格内嵌目录（id 无效 / 非目录 → None）。
fn builtin_persona_dir(id: &str) -> Option<&'static include_dir::Dir<'static>> {
    BUILTIN_PERSONAS_DIR
        .dirs()
        .find(|d| d.path().file_name().and_then(|n| n.to_str()) == Some(id))
}

/// 内置人格原文（raw.md 是创作单一真相源，雇佣专家取材）。注意
/// include_dir 的 get_file 匹配**相对内嵌根的完整路径**（子目录里的文件
/// 是 `<id>/raw.md` 形态），不能在子目录对象上按裸文件名查。
fn builtin_persona_raw(id: &str) -> Option<&'static str> {
    BUILTIN_PERSONAS_DIR
        .get_file(format!("{id}/raw.md"))?
        .contents_utf8()
}

/// 把内嵌模板释放到工作区 personas/<id>/（安装/雇佣时的 B2 按需送达，
/// 老用户零操作拿到新内置人格）。已安装由调用方前置校验；模板里已有
/// 的文件**不覆盖**工作区同名件（如只雇佣过专家的目录里的 raw.md
/// 原文存档是本地事实，不吞）。
fn release_builtin_persona(workspace: &str, id: &str) -> Result<(), String> {
    let dir = builtin_persona_dir(id).ok_or_else(|| format!("builtin persona '{id}' not found"))?;
    let out_root = resolve_path(workspace, &format!("personas/{id}"))?;
    std::fs::create_dir_all(&out_root).map_err(|e| format!("failed to create dir: {e}"))?;
    // 先聚合、PERSONA.json 压轴（2026-11-15 四轮逐行复查修，与
    // [`write_persona_files`] 同一提交标记语义）：include_dir files() 迭代
    // 序是内部细节（字母序 PERSONA.json 列中段），写序中途失败（盘满/权限）
    // 会留下「半目录 + installed=true」假安装态堵死重试；压轴后部分态诚实
    // 呈「未安装」，重试按在场跳过幂等续传补齐。
    let mut entries: Vec<(&str, &[u8])> = dir
        .files()
        .filter_map(|f| {
            f.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| (n, f.contents()))
        })
        .collect();
    entries.sort_by_key(|(name, _)| if *name == "PERSONA.json" { 1 } else { 0 });
    let mut released = 0usize;
    for (name, contents) in &entries {
        let out = out_root.join(name);
        if out.exists() {
            continue;
        }
        // 原子写（同 BUG-20261121-020 家族：撕裂存档文件会被 activate 恢复）。
        nemesis_utils::write_file_atomic(&out.to_string_lossy(), contents, 0o644)
            .map_err(|e| format!("failed to write personas/{id}/{name}: {e}"))?;
        released += 1;
    }
    // 模板缺件（如 HEARTBEAT.md）按 shop.download 同款语义补齐
    let heartbeat = out_root.join("HEARTBEAT.md");
    if !heartbeat.exists() {
        nemesis_utils::write_file_atomic(&heartbeat.to_string_lossy(), b"", 0o644)
            .map_err(|e| format!("failed to write personas/{id}/HEARTBEAT.md: {e}"))?;
    }
    tracing::debug!(persona = %id, released, "[Persona] builtin persona released to workspace");
    Ok(())
}

/// 原文（raw.md 单文件形态）→ personas/<id>/ 六件套转换落盘（download 两臂
/// 与 builtin 免下载直聘用共用写入路径；调用方已前置安装态校验）。
/// HEARTBEAT.md 空文件占位；raw.md 上游原文原样存档（雇佣专家取材真相源 +
/// 转换器升级后的重转修复面）。
/// **PERSONA.json 压轴**（2026-11-15 四轮逐行复查修，提交标记语义）：
/// [`is_persona_installed`] 唯一口径 = PERSONA.json 在场——它必须最后写。
/// 此前排第一，写序中途失败（盘满/权限）留下「半目录 + installed=true」
/// 假安装态，重试被 "already installed" 堵死；压轴后部分态诚实呈
/// 「未安装」，重试整目录覆盖写干净恢复。
fn write_persona_files(workspace: &str, id: &str, content: &str) -> Result<(), String> {
    let dir = resolve_path(workspace, &format!("personas/{}", id))?;
    let converted = convert_agent_content(content);
    std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir: {}", e))?;
    // 人格存档文件统一原子写（2026-11-21 BUG-20261121-020 家族根修）：裸写
    // 半截文件会被 list 呈现、被 activate 恢复进工作区活体。写序保持
    // PERSONA.json 压轴（上方注释的 marker 语义不因原子化改变）。
    nemesis_utils::write_file_atomic(
        &dir.join("IDENTITY.md").to_string_lossy(),
        converted.identity.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write IDENTITY.md: {}", e))?;
    nemesis_utils::write_file_atomic(
        &dir.join("SOUL.md").to_string_lossy(),
        converted.soul.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write SOUL.md: {}", e))?;
    nemesis_utils::write_file_atomic(
        &dir.join("AGENT.md").to_string_lossy(),
        converted.agent_full.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write AGENT.md: {}", e))?;
    nemesis_utils::write_file_atomic(
        &dir.join("TOOLS.md").to_string_lossy(),
        converted.tools_full.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write TOOLS.md: {}", e))?;
    nemesis_utils::write_file_atomic(&dir.join("HEARTBEAT.md").to_string_lossy(), b"", 0o644)
        .map_err(|e| format!("failed to write HEARTBEAT.md: {}", e))?;
    nemesis_utils::write_file_atomic(
        &dir.join("raw.md").to_string_lossy(),
        content.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write raw.md: {}", e))?;
    nemesis_utils::write_file_atomic(
        &dir.join("PERSONA.json").to_string_lossy(),
        converted.persona_json.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write PERSONA.json: {}", e))?;
    Ok(())
}

/// 推荐橱窗行（2026-11-15 策展映射，方案 A）：编译期人工策展单一真相源，
/// 学 professions CATALOG 哲学——推荐是权威人工口径，不做 LLM 自动归类
/// 冒充推荐；映射表未覆盖的人格不显示推荐，不做假分类。
/// - `key`：六职能键与 professions CATALOG slug 对齐（行头模型档位注记
///   运行期查 `find_builtin(key).min_tier`）；方向键无权威档位数据，
///   不显示——不硬编。
/// - `division`：行头「查看该类全部 →」预置的全部页 division 过滤
///   （跨域方向无对应 division = None，不渲染跳转）。
/// - ref 形态：`source/id`；裸 id = 默认源（agency）简写，表保持干净。
struct RecommendSection {
    key: &'static str,
    title: &'static str,
    division: Option<&'static str>,
    refs: &'static [&'static str],
}

/// 行排序（用户拍板）：六职能在前（CATALOG 顺序），特殊方向在后；方向里
/// 代码 Review、BUG 分析（用户点名）排最前。
///
/// **ref id 口径（2026-11-15 冒烟实录教训）**：上游文件是 `<dir>-<name>.md`
/// 平铺命名，id = 文件名去 .md（含目录前缀，如 `design-ui-designer`）——
/// 初版手写裸 basename（`ui-designer`）全表错位，运行期防死链诚实丢弃后
/// 15 行只活 4 行；现全表 id 逐条对照当日 live tree 校正，并由
/// `recommend_refs_resolve_against_captured_roster` 漂移锁钉住（testdata
/// roster 快照）。未来加 ref：id 必须取自上游真实文件名（roster/rexplore），
/// 不是展示名。
const RECOMMEND_SECTIONS: &[RecommendSection] = &[
    RecommendSection {
        key: "product",
        title: "产品",
        division: Some("产品"),
        refs: &[
            "product-manager",
            "product-sprint-prioritizer",
            "product-trend-researcher",
            "product-feedback-synthesizer",
        ],
    },
    RecommendSection {
        key: "ui-design",
        title: "UI 设计",
        division: Some("创意"),
        refs: &[
            "design-ui-designer",
            "design-ux-architect",
            "design-ux-researcher",
            "design-ui-finish-gate-reviewer",
        ],
    },
    RecommendSection {
        key: "architecture",
        title: "架构",
        division: None,
        refs: &[
            "engineering-software-architect",
            "engineering-backend-architect",
            "engineering-multi-agent-systems-architect",
            "security-architect",
        ],
    },
    RecommendSection {
        key: "dev",
        title: "开发",
        division: Some("开发"),
        refs: &[
            "engineering-frontend-developer",
            "engineering-rust-refactoring-specialist",
            "engineering-minimal-change-engineer",
            "engineering-git-workflow-master",
            "specialized-mcp-builder",
            "engineering-rapid-prototyper",
        ],
    },
    RecommendSection {
        key: "test-whitebox",
        title: "白盒测试",
        division: None,
        refs: &[
            "engineering-code-reviewer",
            "security-ai-generated-code-auditor",
            "specialized-codebase-archaeologist",
            "security-appsec-engineer",
        ],
    },
    RecommendSection {
        key: "test-blackbox",
        title: "黑盒测试",
        division: Some("测试"),
        refs: &[
            "testing-reality-checker",
            "testing-evidence-collector",
            "testing-api-tester",
            "testing-test-automation-engineer",
            "testing-performance-benchmarker",
        ],
    },
    RecommendSection {
        key: "code-review",
        title: "代码 Review",
        division: None,
        refs: &[
            "engineering-code-reviewer",
            "security-ai-generated-code-auditor",
            "specialized-codebase-archaeologist",
            "security-appsec-engineer",
        ],
    },
    RecommendSection {
        key: "bug-analysis",
        title: "BUG 分析",
        division: None,
        refs: &[
            // 两个纯分析角色相邻成族（用户拍板 2026-11-15）：测试失败结果
            // 分析与 bug 根因分析互为上下游；事故指挥偏响应流程，行尾。
            "testing-test-results-analyzer",
            "builtin/bug-analyst",
            "engineering-incident-response-commander",
        ],
    },
    RecommendSection {
        key: "ai-agent",
        title: "AI·Agent 工程",
        division: None,
        refs: &[
            "engineering-ai-engineer",
            "engineering-rag-pipeline-engineer",
            // prompt-engineer = LLM 提示词工程（engineering 臂）；design-image-
            // prompt-engineer 是图像生成提示词，非本行语义。
            "engineering-prompt-engineer",
            "engineering-multi-agent-systems-architect",
            "specialized-mcp-builder",
            "engineering-llm-post-training-engineer",
            "agentic-identity-trust",
        ],
    },
    RecommendSection {
        key: "reliability",
        title: "可靠性运维",
        division: None,
        refs: &[
            "engineering-sre",
            "engineering-incident-response-commander",
            "engineering-database-reliability-engineer",
            "engineering-devops-automator",
            "support-infrastructure-maintainer",
            "engineering-finops-engineer",
        ],
    },
    RecommendSection {
        key: "security",
        title: "安全",
        division: Some("安全"),
        refs: &[
            "security-appsec-engineer",
            "security-secrets-credential-engineer",
            "security-penetration-tester",
            "security-incident-responder",
            "security-threat-detection-engineer",
        ],
    },
    RecommendSection {
        key: "data",
        title: "数据",
        division: None,
        refs: &[
            // data-engineer = 数据管线工程（engineering 臂）；gis-spatial-
            // data-engineer 是 GIS 空间数据，非本行语义。
            "engineering-data-engineer",
            "engineering-database-optimizer",
            "engineering-data-visualization-engineer",
            "support-analytics-reporter",
            "engineering-search-relevance-engineer",
        ],
    },
    RecommendSection {
        key: "iot",
        title: "IoT·嵌入式",
        division: Some("开发"),
        refs: &[
            "engineering-embedded-firmware-engineer",
            "engineering-iot-fleet-engineer",
        ],
    },
    RecommendSection {
        key: "docs",
        title: "文档",
        division: None,
        refs: &[
            "engineering-technical-writer",
            "engineering-codebase-onboarding-engineer",
        ],
    },
    RecommendSection {
        key: "china-eco",
        title: "中国生态",
        division: None,
        refs: &[
            "engineering-feishu-integration-developer",
            "engineering-wechat-mini-program-developer",
            "engineering-china-network-engineer",
            "engineering-gaussdb-expert",
            "marketing-china-market-localization-strategist",
        ],
    },
];

/// 策展 ref 归一：裸 id = 默认源简写。
fn normalize_recommend_ref(r: &str) -> String {
    if r.contains('/') {
        r.to_string()
    } else {
        format!("{DEFAULT_SOURCE_KEY}/{r}")
    }
}

/// 六职能行头模型档位注记（数据源 = professions CATALOG min_tier 单一
/// 真相源；方向键不在 CATALOG → None，不硬编）。
fn recommend_section_tier(key: &str) -> Option<&'static str> {
    nemesis_prompts::professions::meta::find_builtin(key).map(|m| m.min_tier)
}

/// 反查索引：composite ref（`source/id`）→ 推荐行 title 列表
/// （browse/search 投影注入 `recommended_for` 用；一个 id 可属多行）。
fn recommend_titles_by_ref() -> HashMap<String, Vec<&'static str>> {
    let mut map: HashMap<String, Vec<&'static str>> = HashMap::new();
    for s in RECOMMEND_SECTIONS {
        for r in s.refs {
            let entry = map.entry(normalize_recommend_ref(r)).or_default();
            if !entry.contains(&s.title) {
                entry.push(s.title);
            }
        }
    }
    map
}

/// shop.* WSAPI 可选 `source` 参数裁决（缺省 = 默认源，向后兼容；未知
/// key loud 报错——前端传错源名不该静默落到默认源）。值集 = [`SHOP_SOURCES`]
/// 注册表（单一真相源；加源自动进合法值集）。
fn resolve_shop_source(data: Option<&serde_json::Value>) -> Result<&'static str, String> {
    let key = data
        .and_then(|d| d.get("source"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_SOURCE_KEY);
    shop_source(key).map(|s| s.key).ok_or_else(|| {
        let legal = SHOP_SOURCES
            .iter()
            .map(|s| s.key)
            .collect::<Vec<_>>()
            .join(" | ");
        format!("unknown shop source '{key}'（合法：{legal}）")
    })
}

/// persona 名/id 路径安全闸——复用技能 slug 验证器单一真相源（
/// web 面 activate/remove/file.*/shop.download 此前零校验。`name: ""` 经
/// resolve_path("personas/") 命中 personas 目录本身：remove 直接全灭人格库
/// （default 档案 + 激活标记 + 全部已装人格，不可逆），activate 清空工作区
/// 人格文件后把激活标记写成空名。验证器拒空串/路径分隔符/`..`/`:`（盘符
/// 相对路径 + NTFS ADS）/Windows 保留名/尾点尾空格，必须在触碰文件系统
/// 与网络之前调用（CLI install/remove 同源， 起）。
fn validate_persona_name(name: &str) -> Result<(), String> {
    nemesis_skills::types::validate_skill_identifier(name)
        .map_err(|e| format!("persona name '{}' invalid: {}", name, e))
}

/// shop 物化写入面 id 保留名闸（2026-11-15 三轮逐行复查补）：`default` 是
/// 人格存储保留名——[`ensure_persona_store`] 把 workspace 根现有人格归档进
/// `personas/default/`，`cmd_remove` 拒删 default（归档不可再生）。download /
/// hire（builtin 免下载臂）/ hire_expert（raw.md 存档）都会写
/// `personas/<id>/`，id=default 时把 default 归档整目录覆盖（IDENTITY/SOUL
/// 等不可逆丢失——与 cmd_remove 的拒删闸同一保留语义，此前只有读侧/删侧
/// 有闸、写侧裸奔）。上游 283 id 与内置 id 现无 default（潜伏态防御缺口，
/// 直接 WSAPI 调用是现实触达面）。preview 只读不写不设闸；activate/remove/
/// file.* 面的 default 是合法名（归档本体语义），不能共用本闸。
/// **大小写不敏感比对**（2026-11-15 四轮逐行复查补，CLAUDE.md 环境敏感
/// 清单命中）：验证器无大小写约束，`Default` 过闸后 NTFS/FAT 大小写不敏感
/// 文件系统上 `personas/Default/` 与 `personas/default/` 是同一目录——精确
/// 匹配在 Windows 上拦不住归档覆盖。保留语义按跨平台保守口径：ASCII 大小
/// 写变体一律拒绝（Linux 区分大小写也不放行——诚实报错优于平台分歧行为）。
fn validate_shop_writable_id(id: &str) -> Result<(), String> {
    validate_persona_name(id)?;
    if id.eq_ignore_ascii_case("default") {
        return Err(
            "'default' 是人格存储保留名（default 归档目录不可覆盖），不可经超市下载/雇佣"
                .to_string(),
        );
    }
    Ok(())
}

/// persona 文件编辑面白名单（`file` 参数此前裸拼
/// resolve_path，`../config.json` 形态命中 workspace 内任意路径——人格文件
/// 编辑器成了任意 workspace 文件读/裸写原语，可绕过 config schema 校验
/// 直接改写 config.json / cron store / approval_rules 等。PersonaView
/// 逐 PERSONA_FILES 编辑，白名单与其完全一致）。
fn validate_persona_file(file: &str) -> Result<(), String> {
    if PERSONA_FILES.contains(&file) {
        Ok(())
    } else {
        Err(format!(
            "file '{}' 不是可编辑的人格文件（允许：{}）",
            file,
            PERSONA_FILES.join(", ")
        ))
    }
}

// Data types

#[derive(Clone)]
#[allow(dead_code)]
struct AgentEntry {
    id: String,
    name: String,
    emoji: String,
    description: String,
    category: String,
    path: String,
}

#[derive(Clone)]
struct Frontmatter {
    name: String,
    emoji: String,
    description: String,
    vibe: String,
    color: String,
    tools: String,
    raw_yaml: String,
}

struct Section {
    title: String,
    content: String,
}

// Category mapping

/// 目录名 → 中文分类展示名（统一定义；CLI `persona search` 同源消费）。
pub fn map_category(dir: &str) -> &'static str {
    match dir {
        "engineering" => "开发",
        "marketing" => "营销",
        "security" => "安全",
        "design" => "创意",
        "academic" => "学术",
        "product" => "产品",
        "paid-media" => "付费媒体",
        "sales" => "销售",
        "finance" => "金融",
        "game-development" => "游戏开发",
        "gis" => "GIS",
        "spatial-computing" => "空间计算",
        "specialized" => "专业",
        "testing" => "测试",
        "support" => "客服",
        "project-management" => "项目管理",
        _ => "通用",
    }
}

// File filtering & parsing

/// 超市.md 路径筛选（CLI `persona search` 同源消费； 前
/// CLI 侧简化版漏 `filename == id` 臂）。
pub fn is_agent_file(path: &str) -> bool {
    if !path.ends_with(".md") {
        return false;
    }
    if !path.contains('/') {
        return false;
    }
    let first_dir = path.split('/').next().unwrap_or("");
    if SKIP_DIRS.contains(&first_dir) {
        return false;
    }
    let filename = path.rsplit('/').next().unwrap_or("");
    if SKIP_FILENAMES.contains(&filename) {
        return false;
    }
    if filename.starts_with("QUICKSTART") || filename.starts_with("EXECUTIVE") {
        return false;
    }
    true
}

fn parse_agent_from_path(path: &str) -> AgentEntry {
    let parts: Vec<&str> = path.trim_end_matches(".md").split('/').collect();
    let id = parts.last().unwrap_or(&"").to_string();
    let category_dir = parts.first().unwrap_or(&"").to_string();
    let name = id
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    AgentEntry {
        id,
        name,
        emoji: "🤖".to_string(),
        description: String::new(),
        category: map_category(&category_dir).to_string(),
        path: path.to_string(),
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}", e))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("{}", e))? {
        let entry = entry.map_err(|e| format!("{}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        // BUG-20261116-016：目录符号链接跳过——`is_dir()` 是 metadata 语义
        // （跟随链接），链接指回祖先目录时无限递归拷贝直到路径长度上限
        // （dossier.rs copy_dir_sorted 同族先例）。file_type() 不穿越链接。
        let is_dir_link =
            entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) && src_path.is_dir();
        if is_dir_link {
            continue;
        }
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| format!("{}", e))?;
        }
    }
    Ok(())
}

/// 剥掉取值段的 markdown 强调标记与空白：「**姓名：** Alice」与
///「**姓名：Alice**」两种形态都不留星号（尾部 `**`
/// 残留曾写进 PERSONA.json 展示名）。
fn clean_name_value(raw: &str) -> String {
    raw.trim().trim_matches('*').trim().to_string()
}

/// Extract name and emoji from NemesisBot IDENTITY.md format.
fn extract_identity_info(content: &str) -> (String, String) {
    let mut name = "default".to_string();
    let mut emoji = "🤖".to_string();
    for line in content.lines() {
        let line = line.trim();
        // Match "**姓名：** xxx" or "**姓名:** xxx"
        if line.contains("姓名") && (line.contains("：") || line.contains(":")) {
            if let Some(idx) = line
                .find("：")
                .or_else(|| line.find(": **"))
                .or_else(|| line.find(":**"))
            {
                let rest = clean_name_value(&line[idx + "：".len()..]);
                if !rest.is_empty() {
                    name = rest;
                }
            } else if let Some(rest) = line.split_once(['：', ':']).map(|x| x.1) {
                let rest = clean_name_value(rest);
                if !rest.is_empty() {
                    name = rest;
                }
            }
        }
        if line.contains("表情符号") && (line.contains("：") || line.contains(":")) {
            for ch in line.chars() {
                if is_emoji_char(ch) {
                    emoji = ch.to_string();
                    break;
                }
            }
        }
    }
    (name, emoji)
}

// ---------------------------------------------------------------------------
// 超市数据源（在线 / 内置快照降级）
//
// 设计拍板（2026-11-12 附加资源 C-ABI 插件设计文档 §4.3/§4.6/拍板①）：
// GitHub 上游不可达时**必须自动降级**内置快照（不让用户选择；重新拉数据 =
// 切标签页再切回来 remount 触发 browse）；快照由 plugin_resources C-ABI 插件
// 承载（gateway 启动时装载，[`nemesis_plugin::resources`] 桥 peek），缺席 =
// 纯在线（现有行为）。快照内容**永不进** TREE_CACHE/FM_CACHE/CONTENT_CACHE
// （三级缓存只服务在线臂——快照进缓存会让上游恢复后的 browse 仍展示旧快照，
// 破坏刷新语义）。
// ---------------------------------------------------------------------------

/// 快照元信息（shop 命令响应 `snapshot_info` 字段；live → null）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct SnapshotInfo {
    commit: String,
    fetched_at: String,
    file_count: usize,
    /// 人读标签 `commit 前 7 位 @ YYYY-MM-DD`（前端徽标直用）。
    label: String,
}

/// 数据来源：Live（GitHub 在线）/ Snapshot（内置快照降级，带元信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ShopSource {
    Live,
    Snapshot(SnapshotInfo),
}

impl ShopSource {
    fn as_str(&self) -> &'static str {
        match self {
            ShopSource::Live => "live",
            ShopSource::Snapshot(_) => "snapshot",
        }
    }

    /// 响应 `snapshot_info` 字段（live → null）。
    fn info_json(&self) -> serde_json::Value {
        match self {
            ShopSource::Live => serde_json::Value::Null,
            ShopSource::Snapshot(info) => {
                serde_json::to_value(info).unwrap_or(serde_json::Value::Null)
            }
        }
    }
}

/// GitHub 上游错误分类（**只影响日志措辞与 refresh 报错文案，不分支降级行为**
/// ——拍板①：有快照时全部类别一律降级，无快照时全部类别一律报错）。
/// 输入 = online 臂拼装的错误串（reqwest 错误经 [`reqwest_error_chain`]
/// 展开 source 链，超时/DNS/连接细节才可见）。pub：CLI persona 臂同源复用
/// （分类口径单一真相源）。
pub fn classify_github_error(err: &str) -> &'static str {
    let e = err.to_lowercase();
    // 顺序敏感：特定类别先于泛化兜底（rate limit 走 403+专属文案；
    // upstream_gone = 仓库/分支被删的主形态 404）。
    if e.contains("rate limit") || (e.contains("403") && e.contains("forbidden")) {
        "rate_limited"
    } else if e.contains("404") || e.contains("409") {
        "upstream_gone"
    } else if e.contains("timed out")
        || e.contains("timeout")
        || e.contains("error sending request")
        || e.contains("connect")
        || e.contains("dns")
        || e.contains("network")
        || e.contains("broken pipe")
        || e.contains("connection reset")
        || e.contains("os error")
    {
        "network"
    } else if ["500", "502", "503", "504"].iter().any(|s| e.contains(s)) {
        "http_5xx"
    } else if e.contains("returned 4") {
        "http_4xx"
    } else {
        "other"
    }
}

/// reqwest 错误源链展开（Display 只有首环；超时/DNS/连接细节在 source() 链里
/// ——分类依赖这些词。nemesis-providers transport_error 同款纪律）。
fn reqwest_error_chain(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// 内置快照树（插件缺席 / manifest 无该 kind / 过滤后空 = None → 纯在线）。
/// 语义过滤单一真相源 = [`is_agent_file`]（快照与在线同一枚举口径——设计
/// 「机械/语义过滤分离」：插件存全量 *.md，语义过滤留消费侧）。size 恒 0
/// （下游不消费 size；在线树才带真实体积）。
fn snapshot_tree() -> Option<(Vec<(String, i64)>, nemesis_plugin::resources::ResourceEntry)> {
    let (paths, entry) = nemesis_plugin::resources::snapshot_tree_filtered(
        nemesis_plugin::resources::RESOURCES_KIND_PERSONA_CATALOG,
        is_agent_file,
    )?;
    Some((paths.into_iter().map(|p| (p, 0)).collect(), entry))
}

/// 树内查 id → 路径（在线/快照两臂共用的匹配规则单一真相源）。
fn tree_path_for<'a>(tree: &'a [(String, i64)], id: &str) -> Option<&'a str> {
    tree.iter()
        .find(|(p, _)| {
            let filename = p.rsplit('/').next().unwrap_or("");
            filename.trim_end_matches(".md") == id || filename == id
        })
        .map(|(p, _)| p.as_str())
}

/// shop.refresh 失败文案（快照在场带标签，缺席诚实说明）。
fn refresh_failure_message(class: &str, snapshot_label: Option<&str>) -> String {
    match snapshot_label {
        Some(label) => format!(
            "无法刷新：GitHub 上游不可达（{class}）；当前展示为内置快照（{label}），可稍后重试"
        ),
        None => format!("无法刷新：GitHub 上游不可达（{class}），且本机无内置快照插件"),
    }
}

// GitHub API helpers

fn get_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client")
    })
}

/// cache: file tree from GitHub Trees API.
static TREE_CACHE: std::sync::LazyLock<std::sync::Mutex<Option<Vec<(String, i64)>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn invalidate_cache() {
    if let Ok(mut tree) = TREE_CACHE.lock() {
        *tree = None;
    }
    FM_CACHE.lock().unwrap().clear();
    CONTENT_CACHE.lock().unwrap().clear();
    if let Ok(mut list) = BUILTIN_LIST_CACHE.lock() {
        *list = None;
    }
}

async fn fetch_tree_online() -> Result<Vec<(String, i64)>, String> {
    {
        let cache = TREE_CACHE.lock().unwrap();
        if let Some(ref tree) = *cache {
            return Ok(tree.clone());
        }
    }
    let url = format!(
        "{}/repos/{}/git/trees/{}?recursive=1",
        GITHUB_API, AGENCY_REPO, AGENCY_BRANCH
    );
    let resp = get_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub tree API error: {}", reqwest_error_chain(&e)))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = nemesis_utils::http_cap::read_error_body(resp).await;
        return Err(format!("GitHub tree API returned {}: {}", status, body));
    }
    // 成功响应体有帽读（BUG-20261114-046 同类横扫补，2026-11-15 六轮复查）：
    // 错误臂早已 read_error_body 有帽，成功臂是同款不对称面——reqwest 无
    // 默认体上限，超时只约束时间不约束体积（快管道超时窗口内可灌 GB 级）。
    // 帽值 = nemesis-skills registry 单一真相源（16MB API JSON）。
    let body = nemesis_utils::http_cap::read_body_capped(
        resp,
        nemesis_skills::registry::MAX_API_JSON_BODY_BYTES,
    )
    .await
    .map_err(|e| format!("GitHub tree API body read: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse tree response: {}", e))?;
    let mut entries = Vec::new();
    if let Some(tree) = json["tree"].as_array() {
        for item in tree {
            let path = item["path"].as_str().unwrap_or("");
            let size = item["size"].as_i64().unwrap_or(0);
            if is_agent_file(path) {
                entries.push((path.to_string(), size));
            }
        }
    }
    let mut cache = TREE_CACHE.lock().unwrap();
    *cache = Some(entries);
    Ok(cache.as_ref().unwrap().clone())
}

/// 在线优先 / 快照兜底的树获取（拍板①：失败必须自动降级，不让用户选择）。
/// 快照树**不写 TREE_CACHE**（缓存只服务在线臂）；降级 WARN 留痕类别与快照
/// 标签；快照缺席时原样返回在线臂错误（纯在线行为不变）。
async fn fetch_tree() -> Result<(Vec<(String, i64)>, ShopSource), String> {
    match fetch_tree_online().await {
        Ok(tree) => Ok((tree, ShopSource::Live)),
        Err(e) => {
            let class = classify_github_error(&e);
            match snapshot_tree() {
                Some((tree, entry)) => {
                    tracing::warn!(
                        "[Persona] GitHub 上游不可达（{class}），超市降级内置快照（{}）",
                        entry.short_label()
                    );
                    Ok((
                        tree,
                        ShopSource::Snapshot(SnapshotInfo {
                            commit: entry.upstream_commit.clone(),
                            fetched_at: entry.fetched_at_utc.clone(),
                            file_count: entry.file_count,
                            label: entry.short_label(),
                        }),
                    ))
                }
                None => Err(e),
            }
        }
    }
}

/// cache: frontmatter extracted from downloaded files.
/// 键 = [`cache_key`] 复合形态 `<source>/<id>`（2026-11-15 复合键化：同 id
/// 双源并存时互不串号——跨源覆盖会让 builtin live 内容被 agency 臂读走）。
static FM_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Frontmatter>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// cache: full.md content downloaded from GitHub. 键 = [`cache_key`] 复合形态。
static CONTENT_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// cache: builtin 目录源在仓清单（contents API 目录列举，进程内 once，
/// refresh 清）。只服务 live 臂——失败臂回落编译期内嵌模板（不进缓存），
/// 与 agency 快照「永不进缓存」同一纪律（缓存只服务在线臂）。
static BUILTIN_LIST_CACHE: std::sync::LazyLock<std::sync::Mutex<Option<Vec<String>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// FM_CACHE/CONTENT_CACHE 复合键（`<source>/<id>`）单一拼装点。
fn cache_key(source: &str, id: &str) -> String {
    format!("{source}/{id}")
}

/// builtin 单人格原文直链（raw.githubusercontent.com：**raw 域名不占
/// api.github.com 60 次/小时配额**——agency 283 个 fm 全 403 的冒烟实录
/// 教训；raw 可达网络零配额消耗）。
fn builtin_raw_file_url(id: &str, file: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{BUILTIN_REPO}/{BUILTIN_BRANCH}/{BUILTIN_PATH}/{id}/{file}"
    )
}

/// builtin 单人格原文 contents API 形态（2026-11-16 卡慢 BUG 修复①②臂：
/// raw 域在部分网络不可达——黑洞拖满 30s 超时才失败；api.github.com 与
/// agency 同域实测可达。占用配额，仅作 raw 传输层失败后的回落臂）。
fn builtin_contents_file_url(id: &str, file: &str) -> String {
    format!(
        "{GITHUB_API}/repos/{BUILTIN_REPO}/contents/{BUILTIN_PATH}/{id}/{file}?ref={BUILTIN_BRANCH}"
    )
}

/// GitHub contents API 响应 JSON → 原文文本（encoding=base64 解码；
/// agency [`fetch_agent_content_online`] 与 builtin 回落臂共用单一真相源）。
fn decode_github_contents_text(json: &serde_json::Value) -> Result<String, String> {
    let encoding = json["encoding"].as_str().unwrap_or("");
    let content_b64 = json["content"]
        .as_str()
        .ok_or("missing content field in GitHub response")?;
    let content = if encoding == "base64" {
        // GitHub returns base64 with newlines
        let cleaned: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned)
            .map_err(|e| format!("base64 decode error: {}", e))?
    } else {
        content_b64.as_bytes().to_vec()
    };
    String::from_utf8(content).map_err(|e| format!("utf8 error: {}", e))
}

/// builtin 目录源清单在线拉取（contents API 目录列举，1 次调用拿全清单；
/// 命中写 [`BUILTIN_LIST_CACHE`]）。只认顶层子目录（一个人格一个目录），
/// 名字过 [`validate_persona_name`] 安全闸（防脏目录名流到落盘面）。
async fn fetch_builtin_list_online() -> Result<Vec<String>, String> {
    {
        let cache = BUILTIN_LIST_CACHE.lock().unwrap();
        if let Some(ref list) = *cache {
            return Ok(list.clone());
        }
    }
    #[cfg(test)]
    forced_builtin_live_fail_check().await?;
    let src = shop_source(BUILTIN_SOURCE_KEY).expect("builtin source registered");
    let url = format!(
        "{}/repos/{}/contents/{}?ref={}",
        GITHUB_API, src.repo, src.path, src.branch
    );
    let resp = get_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub contents API error: {}", reqwest_error_chain(&e)))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = nemesis_utils::http_cap::read_error_body(resp).await;
        return Err(format!("GitHub contents API returned {}: {}", status, body));
    }
    // 成功响应体有帽读（046 同类横扫补，帽值 = registry 单一真相源 16MB）。
    let body = nemesis_utils::http_cap::read_body_capped(
        resp,
        nemesis_skills::registry::MAX_API_JSON_BODY_BYTES,
    )
    .await
    .map_err(|e| format!("GitHub contents API body read: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse contents response: {}", e))?;
    let arr = json
        .as_array()
        .ok_or("GitHub contents response is not an array")?;
    let mut ids: Vec<String> = arr
        .iter()
        .filter(|e| e["type"] == "dir")
        .filter_map(|e| e["name"].as_str())
        .filter(|n| {
            if validate_persona_name(n).is_ok() {
                true
            } else {
                tracing::warn!("[Persona] builtin 目录 '{n}' 名字未过安全闸，清单跳过");
                false
            }
        })
        .map(String::from)
        .collect();
    ids.sort();
    let mut cache = BUILTIN_LIST_CACHE.lock().unwrap();
    *cache = Some(ids.clone());
    Ok(ids)
}

/// builtin 单人格原文（raw.md）在线拉取（**两臂级联**，2026-11-16 卡慢 BUG
/// 修复①）：臂① raw.githubusercontent.com 直链（不占 api.github.com 60 次/
/// 小时配额；带 5s 短超时——黑洞网络快败不拖满 client 30s）→ 臂②
/// api.github.com contents API（与 agency 同域同法，base64 经
/// [`decode_github_contents_text`]；占配额，仅 raw 传输层失败后回落）。
/// 级联条件 = 传输层失败（send Err：域名不可达/黑洞）；HTTP 非 2xx 不级联
/// ——文件真缺失时两域同 404，白烧配额。失败返回 Err → 调用方回落编译期
/// 内嵌模板（第三臂，browse/preview/download/hire 各自既有兜底不动）。
/// 命中写 CONTENT_CACHE/FM_CACHE 复合键——browse/preview/download/hire 四链
/// 共享，与 agency [`fetch_agent_content_online`] 同款副作用养缓存语义。
async fn fetch_builtin_raw_online(id: &str) -> Result<String, String> {
    let key = cache_key(BUILTIN_SOURCE_KEY, id);
    {
        let cache = CONTENT_CACHE.lock().unwrap();
        if let Some(content) = cache.get(&key) {
            return Ok(content.clone());
        }
    }
    #[cfg(test)]
    forced_builtin_live_fail_check().await?;

    // 臂①：raw 直链（5s 单请求超时，快败让位臂②）
    let raw_url = builtin_raw_file_url(id, "raw.md");
    let raw_resp = match get_client()
        .get(&raw_url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => Some(resp),
        Ok(resp) => {
            // HTTP 层失败（404/403 等）：文件/仓库配置问题，臂②同 URL 同结果，
            // 诚实报错不级联。
            let status = resp.status();
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(format!("builtin raw fetch returned {}: {}", status, body));
        }
        Err(e) => {
            tracing::debug!(
                "builtin raw 直链传输失败（{:?}），回落 contents API：{}",
                e,
                reqwest_error_chain(&e)
            );
            None
        }
    };
    if let Some(resp) = raw_resp {
        // 成功响应体有帽读（046 同类横扫补）：markdown 人格文件 KB 量级，1MB
        // 与 SKILL.md 内容帽同源（registry 单一真相源）。resp.text() 的 lossy
        // 解码语义保留（from_utf8_lossy）。
        let body = nemesis_utils::http_cap::read_body_capped(
            resp,
            nemesis_skills::registry::MAX_SKILL_CONTENT_BODY_BYTES,
        )
        .await
        .map_err(|e| format!("builtin raw read error: {e}"))?;
        let text = String::from_utf8_lossy(&body).into_owned();
        if let Some(fm) = parse_frontmatter(&text) {
            FM_CACHE.lock().unwrap().insert(key.clone(), fm);
        }
        CONTENT_CACHE.lock().unwrap().insert(key, text.clone());
        return Ok(text);
    }

    // 臂②：contents API base64（raw 域不可达的网络——api.github.com 与
    // agency 同域可达）。
    let api_url = builtin_contents_file_url(id, "raw.md");
    let resp = get_client()
        .get(&api_url)
        .send()
        .await
        .map_err(|e| format!("builtin contents API error: {}", reqwest_error_chain(&e)))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = nemesis_utils::http_cap::read_error_body(resp).await;
        return Err(format!(
            "builtin contents API returned {}: {}",
            status, body
        ));
    }
    let body = nemesis_utils::http_cap::read_body_capped(
        resp,
        nemesis_skills::registry::MAX_API_JSON_BODY_BYTES,
    )
    .await
    .map_err(|e| format!("builtin contents API body read: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse builtin contents response: {}", e))?;
    let text = decode_github_contents_text(&json)?;
    if let Some(fm) = parse_frontmatter(&text) {
        FM_CACHE.lock().unwrap().insert(key.clone(), fm);
    }
    CONTENT_CACHE.lock().unwrap().insert(key, text.clone());
    Ok(text)
}

/// 测试强制 builtin live 臂失败（0=真实上游 / 2=强制失败；与
/// [`TEST_REFRESH_MODE`] 同款纪律，SHOP_TEST_LOCK 配合，用后归零——生产恒 0）。
#[cfg(test)]
pub(crate) static TEST_BUILTIN_LIVE_MODE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

/// 内联失败检查点（fetch_builtin_*_online 共用）：测试臂 2 = 模拟网络不可达，
/// 兜底臂离线可测。
#[cfg(test)]
async fn forced_builtin_live_fail_check() -> Result<(), String> {
    if TEST_BUILTIN_LIVE_MODE.load(std::sync::atomic::Ordering::Relaxed) == 2 {
        return Err("SSL connect error (builtin live forced for test)".to_string());
    }
    Ok(())
}

/// [`TEST_BUILTIN_LIVE_MODE`]=2 的 panic 安全守卫（Drop 归零——断言失败
/// 不会把强制态泄漏给后续测试；对照 TEST_REFRESH_MODE 的手动归零纪律）。
#[cfg(test)]
pub(crate) struct ForcedBuiltinLiveFail;

#[cfg(test)]
impl ForcedBuiltinLiveFail {
    pub(crate) fn arm() -> Self {
        TEST_BUILTIN_LIVE_MODE.store(2, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

#[cfg(test)]
impl Drop for ForcedBuiltinLiveFail {
    fn drop(&mut self) {
        TEST_BUILTIN_LIVE_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

async fn fetch_agent_content(id: &str) -> Result<(String, ShopSource), String> {
    // 内容缓存命中（只有在线臂写缓存 → 命中必为在线内容，live 语义成立）；
    // 键 = 复合形态（防跨源串号）。
    let key = cache_key(DEFAULT_SOURCE_KEY, id);
    {
        let cache = CONTENT_CACHE.lock().unwrap();
        if let Some(content) = cache.get(&key) {
            return Ok((content.clone(), ShopSource::Live));
        }
    }
    match fetch_agent_content_online(id).await {
        Ok(content) => Ok((content, ShopSource::Live)),
        Err(e) => {
            let class = classify_github_error(&e);
            let Some((tree, entry)) = snapshot_tree() else {
                return Err(e);
            };
            let info = SnapshotInfo {
                commit: entry.upstream_commit.clone(),
                fetched_at: entry.fetched_at_utc.clone(),
                file_count: entry.file_count,
                label: entry.short_label(),
            };
            // 快照两段解析：树内查 id → 读内容；任一环失败维持在线臂原始错误
            // （id 真缺失 / 快照较旧 / 内容读失败都不该被快照错误遮蔽）。
            let Some(path) = tree_path_for(&tree, id) else {
                tracing::warn!(
                    "[Persona] GitHub 上游不可达（{class}），快照树中也无 '{id}'（快照较旧或 id 真缺失），维持原始错误"
                );
                return Err(e);
            };
            match nemesis_plugin::resources::snapshot_content(
                nemesis_plugin::resources::RESOURCES_KIND_PERSONA_CATALOG,
                path,
            ) {
                Ok(content) => {
                    tracing::warn!(
                        "[Persona] GitHub 上游不可达（{class}），'{id}' 改读内置快照（{}）",
                        info.label
                    );
                    Ok((content, ShopSource::Snapshot(info)))
                }
                Err(pe) => {
                    tracing::warn!("[Persona] 快照读 '{path}' 失败（{pe}），维持原始错误");
                    Err(e)
                }
            }
        }
    }
}

async fn fetch_agent_content_online(id: &str) -> Result<String, String> {
    // 复合键（2026-11-15）：`<source>/<id>`，跨源同名互不覆写。
    let key = cache_key(DEFAULT_SOURCE_KEY, id);
    // Find path from tree
    let tree = fetch_tree_online().await?;
    let path = tree_path_for(&tree, id)
        .ok_or_else(|| format!("agent '{}' not found in repository", id))?;

    let url = format!(
        "{}/repos/{}/contents/{}?ref={}",
        GITHUB_API, AGENCY_REPO, path, AGENCY_BRANCH
    );
    let resp = get_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub contents API error: {}", reqwest_error_chain(&e)))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = nemesis_utils::http_cap::read_error_body(resp).await;
        return Err(format!("GitHub contents API returned {}: {}", status, body));
    }
    // 成功响应体有帽读（046 同类横扫补，帽值 = registry 单一真相源 16MB）。
    let body = nemesis_utils::http_cap::read_body_capped(
        resp,
        nemesis_skills::registry::MAX_API_JSON_BODY_BYTES,
    )
    .await
    .map_err(|e| format!("GitHub contents API body read: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("failed to parse contents response: {}", e))?;
    let text = decode_github_contents_text(&json)?;

    // Parse frontmatter and update cache（复合键）
    if let Some(fm) = parse_frontmatter(&text) {
        let mut cache = FM_CACHE.lock().unwrap();
        cache.insert(key.clone(), fm);
    }

    // Update cache
    {
        let mut cache = CONTENT_CACHE.lock().unwrap();
        cache.insert(key, text.clone());
    }

    Ok(text)
}

/// 测试强制 refresh 上游臂结果（0=真实上游 / 1=强制成功 / 2=强制失败）；
/// 与 SHOP_TEST_LOCK 配合使用，用后归零——refresh 语义会真打上游，测试必须
/// 离线可控（生产恒 0）。pub(crate)：handlers 级测试文件（persona_extra /
/// persona_more）也要注入。
#[cfg(test)]
pub(crate) static TEST_REFRESH_MODE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

/// refresh 专用上游拉取（语义同 [`fetch_tree_online`]，测试臂见
/// [`TEST_REFRESH_MODE`]）。
async fn refresh_fetch_tree() -> Result<Vec<(String, i64)>, String> {
    #[cfg(test)]
    {
        match TEST_REFRESH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => return Ok(Vec::new()),
            2 => return Err("SSL connect error (forced for test)".to_string()),
            _ => {}
        }
    }
    fetch_tree_online().await
}

// Frontmatter parsing (simple, no serde_yaml dependency)

fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let end = after_first.find("---")?;
    let yaml = &after_first[..end];

    let mut name = String::new();
    let mut emoji = String::new();
    let mut description = String::new();
    let mut vibe = String::new();
    let mut color = String::new();
    let mut tools = String::new();

    for line in yaml.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("emoji:") {
            emoji = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("vibe:") {
            vibe = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("color:") {
            color = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("tools:") {
            tools = val.trim().trim_matches('"').to_string();
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(Frontmatter {
        name,
        emoji,
        description,
        vibe,
        color,
        tools,
        raw_yaml: yaml.to_string(),
    })
}

// Section parsing

fn strip_emoji(s: &str) -> String {
    s.chars()
        .filter(|c| !is_emoji_char(*c))
        .collect::<String>()
        .trim()
        .to_lowercase()
}

fn is_emoji_char(c: char) -> bool {
    // Common emoji ranges
    matches!(
        c,
        '\u{1F300}'..='\u{1F9FF}'
            | '\u{2600}'..='\u{26FF}'
            | '\u{2700}'..='\u{27BF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{1F000}'..='\u{1FFFF}'
            | '\u{200D}' // ZWJ
    )
}

struct ParsedContent {
    preamble: String,
    sections: Vec<Section>,
}

fn parse_sections(content: &str) -> ParsedContent {
    let mut sections = Vec::new();

    // Find body after frontmatter
    let body = if content.trim_start().starts_with("---") {
        let after_first = &content.trim_start()[3..];
        if let Some(end) = after_first.find("---") {
            &after_first[end + 3..]
        } else {
            content
        }
    } else {
        content
    };

    let mut preamble_lines: Vec<String> = Vec::new();
    let mut current_title = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut found_first_h2 = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") && !trimmed.starts_with("### ") {
            if found_first_h2 {
                sections.push(Section {
                    title: current_title.clone(),
                    content: current_lines.join("\n").trim().to_string(),
                });
                current_lines.clear();
            }
            found_first_h2 = true;
            current_title = trimmed[3..].trim().to_string();
        } else if found_first_h2 {
            current_lines.push(line.to_string());
        } else {
            // Before first ## — collect as preamble (skip title)
            let is_h1 = trimmed.starts_with("# ") && !trimmed.starts_with("## ");
            if !is_h1 {
                preamble_lines.push(line.to_string());
            }
        }
    }

    if found_first_h2 && !current_title.is_empty() {
        sections.push(Section {
            title: current_title.clone(),
            content: current_lines.join("\n").trim().to_string(),
        });
    }

    ParsedContent {
        preamble: preamble_lines.join("\n").trim().to_string(),
        sections,
    }
}

// Conversion engine

enum SectionTarget {
    Identity,
    Soul,
    Agent,
    Tools,
}

fn classify_section(title: &str) -> SectionTarget {
    let norm = strip_emoji(title);
    if norm.contains("identity") || norm.contains("memory") {
        return SectionTarget::Identity;
    }
    if norm.contains("critical rule")
        || norm.contains("communication style")
        || norm.contains("checklist")
    {
        return SectionTarget::Soul;
    }
    if norm.contains("tool") || norm.contains("integration") {
        return SectionTarget::Tools;
    }
    // Everything else goes to Agent (including core mission, workflow, etc.)
    SectionTarget::Agent
}

fn build_persona_json(fm: Option<&Frontmatter>) -> serde_json::Value {
    let name = fm.map(|f| f.name.as_str()).unwrap_or("Unknown");
    let emoji = fm.map(|f| f.emoji.as_str()).unwrap_or("🤖");
    let description = fm.map(|f| f.description.as_str()).unwrap_or("");
    let mut obj = serde_json::json!({
        "name": name,
        "emoji": emoji,
        "description": description,
    });
    if let Some(f) = fm {
        if !f.color.is_empty() {
            obj["color"] = serde_json::Value::String(f.color.clone());
        }
        if !f.tools.is_empty() {
            obj["tools"] = serde_json::Value::String(f.tools.clone());
        }
        if !f.vibe.is_empty() {
            obj["vibe"] = serde_json::Value::String(f.vibe.clone());
        }
        if !f.raw_yaml.is_empty() {
            obj["frontmatter"] = serde_json::Value::String(f.raw_yaml.clone());
        }
    }
    obj
}

struct PersonaFiles {
    identity: String,
    soul: String,
    agent_extra: String, // to be appended to system default AGENT.md
    tools_extra: String, // to be appended to system default TOOLS.md (may be empty)
}

fn convert_agent_md(content: &str) -> PersonaFiles {
    let fm = parse_frontmatter(content);
    let parsed = parse_sections(content);

    let name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or("Unknown");
    let emoji = fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖");
    let vibe = fm.as_ref().map(|f| f.vibe.as_str()).unwrap_or("");
    let description = fm.as_ref().map(|f| f.description.as_str()).unwrap_or("");

    // Build IDENTITY.md (full replacement)
    let identity_sections: Vec<&Section> = parsed
        .sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Identity))
        .collect();

    let mut identity = format!(
        "# {}\n\n{}{}\n\n## 基本信息\n\n- 姓名：{}\n- Emoji：{}\n- 风格：{}\n- 描述：{}\n",
        name,
        if vibe.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", vibe)
        },
        if description.is_empty() {
            String::new()
        } else {
            format!("> {}\n\n", description)
        },
        name,
        emoji,
        vibe,
        description,
    );
    // Include preamble (role description between frontmatter and first ##)
    if !parsed.preamble.is_empty() {
        identity.push_str(&format!("\n---\n\n{}\n", parsed.preamble));
    }
    for sec in &identity_sections {
        identity.push_str(&format!("\n---\n\n## {}\n\n{}\n", sec.title, sec.content));
    }

    // Build SOUL.md (full replacement)
    let soul_sections: Vec<&Section> = parsed
        .sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Soul))
        .collect();

    let mut soul = format!("# {} — 行为规则\n\n", name);
    for sec in &soul_sections {
        soul.push_str(&format!("## {}\n\n{}\n\n", sec.title, sec.content));
    }

    // Build AGENT.md extra (to be appended to system default)
    let agent_sections: Vec<&Section> = parsed
        .sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Agent))
        .collect();

    let mut agent_extra = format!("\n---\n\n# {} 专用工作流\n", name);
    for sec in &agent_sections {
        agent_extra.push_str(&format!("\n## {}\n\n{}\n", sec.title, sec.content));
    }

    // Build TOOLS.md extra (to be appended to system default)
    let tools_sections: Vec<&Section> = parsed
        .sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Tools))
        .collect();

    let tools_extra = if tools_sections.is_empty() {
        String::new()
    } else {
        let mut extra = format!("\n---\n\n# {} 工具使用\n", name);
        for sec in &tools_sections {
            extra.push_str(&format!("\n## {}\n\n{}\n", sec.title, sec.content));
        }
        extra
    };

    PersonaFiles {
        identity,
        soul,
        agent_extra,
        tools_extra,
    }
}

// 共享真相源 API（CLI `nemesisbot persona` 同源消费。
// 此前 CLI 侧各自维护简化实现——activate 不归档旧人格、install 转换产物缺
// 字段——与 web 面语义分裂，跨入口切换会串档/丢文件。）

/// 初始化人格存储：首次（无 `_active.json`）把 workspace 根的现有人格文件
/// 与 memory/sessions/logs 归档进 `personas/default/` 并写 default 激活标记。
pub fn ensure_persona_store(workspace: &str) -> Result<(), String> {
    let personas_dir = resolve_path(workspace, "personas")?;
    // Gate on the real init marker (_active.json), NOT on the personas/ dir.
    // A persona-shop download creates personas/<id>/ (hence personas/)
    // without writing _active.json; gating on the dir alone would skip
    // initialization and leave _active.json missing, so cmd_current /
    // cmd_list fail with "failed to read _active.json".
    if personas_dir.join("_active.json").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&personas_dir)
        .map_err(|e| format!("failed to create personas dir: {}", e))?;

    let default_dir = personas_dir.join("default");
    std::fs::create_dir_all(&default_dir)
        .map_err(|e| format!("failed to create default dir: {}", e))?;

    for &file in PERSONA_FILES.iter().chain(PERSONA_ARCHIVE_FILES.iter()) {
        let src = resolve_path(workspace, file)?;
        if src.exists() {
            let content = std::fs::read_to_string(&src)
                .map_err(|e| format!("failed to read {}: {}", file, e))?;
            // 原子写（BUG-20261121-020 家族）。
            nemesis_utils::write_file_atomic(
                &default_dir.join(file).to_string_lossy(),
                content.as_bytes(),
                0o644,
            )
            .map_err(|e| format!("failed to write {}: {}", file, e))?;
        }
    }

    // Archive memory/ directory if it exists
    for &dir_name in PERSONA_ARCHIVE_DIRS {
        let src_dir = resolve_path(workspace, dir_name)?;
        if src_dir.exists() {
            let dst_dir = default_dir.join(dir_name);
            copy_dir_recursive(&src_dir, &dst_dir)
                .map_err(|e| format!("failed to archive {}: {}", dir_name, e))?;
        }
    }

    // Build PERSONA.json from actual workspace files
    let identity_path = resolve_path(workspace, "IDENTITY.md")?;
    let (name, emoji) = if identity_path.exists() {
        let content = std::fs::read_to_string(&identity_path).unwrap_or_default();
        extract_identity_info(&content)
    } else {
        ("default".to_string(), "🤖".to_string())
    };
    let persona_json = serde_json::json!({
        "name": name,
        "emoji": emoji,
        "description": "",
    });
    let json_str = serde_json::to_string_pretty(&persona_json)
        .map_err(|e| format!("failed to serialize PERSONA.json: {}", e))?;
    // 原子写（BUG-20261121-020 家族）。
    nemesis_utils::write_file_atomic(
        &default_dir.join("PERSONA.json").to_string_lossy(),
        json_str.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("failed to write PERSONA.json: {}", e))?;

    let active = serde_json::json!({"name": "default"});
    let active_str = serde_json::to_string_pretty(&active)
        .map_err(|e| format!("failed to serialize _active.json: {}", e))?;
    // 统一原子写入（人格激活状态）。
    nemesis_utils::write_file_atomic(
        &personas_dir.join("_active.json").to_string_lossy(),
        active_str.as_bytes(),
        0o600,
    )
    .map_err(|e| format!("failed to write _active.json: {}", e))?;

    Ok(())
}

/// 人格激活完整生命周期（web activate/restore 与 CLI activate/remove 共享
/// 的统一定义）：归档当前人格 → 清理 workspace 根 → 恢复目标人格 →
/// 原子写激活标记。
pub fn activate_persona_workspace(workspace: &str, name: &str) -> Result<(), String> {
    ensure_persona_store(workspace)?;

    let target_dir = resolve_path(workspace, &format!("personas/{}", name))?;
    if !target_dir.exists() {
        return Err(format!("persona '{}' not found", name));
    }

    let read_active = || -> Result<String, String> {
        let path = resolve_path(workspace, "personas/_active.json")?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read _active.json: {}", e))?;
        let v: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse _active.json: {}", e))?;
        v.get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "invalid _active.json: missing name".to_string())
    };

    // Step 1: Archive current persona data
    let current = read_active()?;
    if current != name {
        let archive_dir = resolve_path(workspace, &format!("personas/{}", current))?;
        std::fs::create_dir_all(&archive_dir)
            .map_err(|e| format!("failed to create archive dir: {}", e))?;
        let all_files: Vec<&&str> = PERSONA_FILES
            .iter()
            .chain(PERSONA_ARCHIVE_FILES.iter())
            .collect();
        for &file in &all_files {
            let src = resolve_path(workspace, file)?;
            if src.exists() {
                let content = std::fs::read_to_string(&src)
                    .map_err(|e| format!("archive: failed to read {}: {}", file, e))?;
                // 原子写（BUG-20261121-020）：裸写撕裂的存档文件（崩溃窗中途
                // 断电/进程被杀）会在「重激活同名人格」（current==name 跳过
                // 归档）时被 Step 3 原样恢复成活体人格——撕裂内容直接上线。
                nemesis_utils::write_file_atomic(
                    &archive_dir.join(file).to_string_lossy(),
                    content.as_bytes(),
                    0o644,
                )
                .map_err(|e| format!("archive: failed to write {}: {}", file, e))?;
            }
        }
        // Archive directories (memory/)
        for &dir_name in PERSONA_ARCHIVE_DIRS {
            let src_dir = resolve_path(workspace, dir_name)?;
            if src_dir.exists() {
                let dst_dir = archive_dir.join(dir_name);
                if dst_dir.exists() {
                    nemesis_utils::fsutil::remove_dir_all_retry(&dst_dir)
                        .map_err(|e| format!("archive: failed to clean {}: {}", dir_name, e))?;
                }
                copy_dir_recursive(&src_dir, &dst_dir)
                    .map_err(|e| format!("archive: failed to copy {}: {}", dir_name, e))?;
            }
        }
    }

    // Step 2: Clean workspace root of all persona-managed files/dirs
    let all_files: Vec<&&str> = PERSONA_FILES
        .iter()
        .chain(PERSONA_ARCHIVE_FILES.iter())
        .collect();
    for &file in &all_files {
        let dst = resolve_path(workspace, file)?;
        if dst.exists() {
            nemesis_utils::fsutil::remove_file_resilient(&dst);
        }
    }
    for &dir_name in PERSONA_ARCHIVE_DIRS {
        let dst_dir = resolve_path(workspace, dir_name)?;
        if dst_dir.exists() {
            nemesis_utils::fsutil::remove_dir_all_retry(&dst_dir)
                .map_err(|e| format!("clean: failed to remove {}: {}", dir_name, e))?;
        }
    }

    // Step 3: Restore target persona data
    for &file in &all_files {
        let src = target_dir.join(file);
        if src.exists() {
            let content = std::fs::read_to_string(&src)
                .map_err(|e| format!("restore: failed to read {}: {}", file, e))?;
            let dst = resolve_path(workspace, file)?;
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // 原子写（BUG-20261121-020）：这些是工作区活体人格文件（下次
            // AgentInstance 装配 system prompt 的直接来源）。裸写中途崩溃 =
            // 半截文件上线；且用户随后改激活别人格时 Step 1 会把半截文件
            // 归档进旧人格存档，永久污染。原子写把撕裂窗收窄到「文件级
            // 完整、文件间混合」——后者由 _active.json 压轴标记语义兜底
            //（标记仍指旧人格时重激活同名人格即可无损自愈）。
            nemesis_utils::write_file_atomic(&dst.to_string_lossy(), content.as_bytes(), 0o644)
                .map_err(|e| format!("restore: failed to write {}: {}", file, e))?;
        }
    }
    // Restore directories
    for &dir_name in PERSONA_ARCHIVE_DIRS {
        let src_dir = target_dir.join(dir_name);
        if src_dir.exists() {
            let dst_dir = resolve_path(workspace, dir_name)?;
            std::fs::create_dir_all(&dst_dir)
                .map_err(|e| format!("restore: failed to create {}: {}", dir_name, e))?;
            copy_dir_recursive(&src_dir, &dst_dir)
                .map_err(|e| format!("restore: failed to copy {}: {}", dir_name, e))?;
        }
    }

    // Ensure memory/ dir and default MEMORY.md exist for new personas
    let memory_dir = resolve_path(workspace, "memory")?;
    let memory_file = memory_dir.join("MEMORY.md");
    if !memory_file.exists() {
        let _ = std::fs::create_dir_all(&memory_dir);
        let default = "# 长期记忆\n\n（此文件存储应该跨会话持久化保存的重要信息。）\n\n## 用户信息\n\n（关于用户的重要事实）\n\n## 偏好\n\n（随时间了解到的用户偏好）\n\n## 重要笔记\n\n（需要记住的事情）\n";
        // 原子写（BUG-20261121-020 家族；best-effort 语义保持不变）。
        let _ = nemesis_utils::write_file_atomic(
            &memory_file.to_string_lossy(),
            default.as_bytes(),
            0o644,
        );
    }

    let path = resolve_path(workspace, "personas/_active.json")?;
    let v = serde_json::json!({"name": name});
    let s = serde_json::to_string_pretty(&v).map_err(|e| format!("serialize error: {}", e))?;
    // 统一原子写入（人格激活状态）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), s.as_bytes(), 0o600)
        .map_err(|e| format!("failed to write _active.json: {}", e))
}

/// 超市 agent md → 人格五件套的完整转换产物（install 转换统一定义）。
pub struct ConvertedPersona {
    pub identity: String,
    pub soul: String,
    pub agent_full: String,
    pub tools_full: String,
    pub persona_json: String,
}

/// 把超市.md 原文转换为五件套：IDENTITY/SOUL 全量替换；AGENT/TOOLS =
/// 系统默认模板 + 人格附加段；PERSONA.json 含 color/tools/vibe/frontmatter
/// 扩展字段（前 CLI 简化版缺这些字段与 HEARTBEAT.md）。
pub fn convert_agent_content(content: &str) -> ConvertedPersona {
    let fm = parse_frontmatter(content);
    let files = convert_agent_md(content);
    let agent_full = format!("{}\n{}", DEFAULT_AGENT_MD, files.agent_extra);
    let tools_full = if files.tools_extra.is_empty() {
        DEFAULT_TOOLS_MD.to_string()
    } else {
        format!("{}\n{}", DEFAULT_TOOLS_MD, files.tools_extra)
    };
    let persona_json =
        serde_json::to_string_pretty(&build_persona_json(fm.as_ref())).unwrap_or_default();
    ConvertedPersona {
        identity: files.identity,
        soul: files.soul,
        agent_full,
        tools_full,
        persona_json,
    }
}

// PersonaHandler

pub struct PersonaHandler;

/// #16 提示词安全常态化触发（人格变更后，W2 批三 2026-11-20 拍板）。
/// `security.prompt_eval.on_persona_change` 闸（缺省 false——编辑高频
/// 路径默认不跑，用户显式开才生效）；detached fire-and-forget（沙盒内
/// 自评，报告落 `<home>/workspace/logs/eval/`，流水落 prompt_eval.log）。
/// 评估对象 = workspace 根人格四件（IDENTITY/SOUL/USER/AGENT，在场的
/// 拼接）——activate/restore 后根文件即新人格；file.save 存量人格时根
/// 文件未变，评估的是生效面（保存内容激活时自会被再次评估），诚实边界。
fn trigger_persona_eval(workspace: &str, ctx: &RequestContext) {
    let Some(home) = ctx.home.as_deref() else {
        return;
    };
    let raw = crate::handlers::config::read_raw_config(home).ok();
    if !nemesis_config::resolve_prompt_eval(raw.as_ref()).on_persona_change {
        return;
    }
    let mut persona_text = String::new();
    for name in ["IDENTITY.md", "SOUL.md", "USER.md", "AGENT.md"] {
        if let Ok(content) = std::fs::read_to_string(std::path::Path::new(workspace).join(name)) {
            persona_text.push_str(&content);
            persona_text.push('\n');
        }
    }
    if persona_text.trim().is_empty() {
        return;
    }
    let _ =
        nemesis_utils::prompt_eval::trigger_prompt_eval(std::path::Path::new(home), &persona_text);
}

#[async_trait::async_trait]
impl ModuleHandler for PersonaHandler {
    fn module_name(&self) -> &str {
        "persona"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "current",
            "list",
            "activate",
            "restore",
            "remove",
            "file.get",
            "file.save",
            "shop.browse",
            "shop.search",
            "shop.refresh",
            "shop.preview",
            "shop.download",
            // 集群聘用依赖 cluster handler（写 cluster/ 三件套 + peers.toml 事务）
            #[cfg(feature = "cluster")]
            "shop.hire",
            // 雇佣专家（2026-11-13 专家子代理）：单机专家名册，无 feature 闸。
            "shop.hire_expert",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "current" => self.cmd_current(workspace),
            "list" => self.cmd_list(workspace),
            "activate" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                validate_persona_name(&name)?;
                let result = self.cmd_activate(workspace, &name)?;
                restart_agent(ctx)?;
                trigger_persona_eval(workspace, ctx);
                Ok(result)
            }
            "restore" => {
                let result = self.cmd_activate(workspace, "default")?;
                restart_agent(ctx)?;
                trigger_persona_eval(workspace, ctx);
                Ok(result)
            }
            "remove" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                validate_persona_name(&name)?;
                // BUG-20261126-093：was_active 判定与 cmd_remove 内部
                // 比对同口径（ASCII 变体视为同一人格）——否则 case 变体
                // remove 激活中人格后 agent 不重启，继续用已删人格的
                // system prompt。
                let was_active = self
                    .read_active(workspace)
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(&name);
                let result = self.cmd_remove(workspace, &name)?;
                // 次生修复：remove-active 在 cmd_remove 内部落
                // default（文件面已切），但此前不同步重启 agent——activate/restore
                // 臂都经 restart_agent 加载新人格，remove-active 漏重启让 agent
                // 继续用已删除人格的 system prompt 直到下次重启。agent 缺席
                // （纯本地/测试 ctx）best-effort 跳过（与 shop.hire 的 cluster
                // reload 缺席跳过同 spirit）。
                if was_active && ctx.state.agent_service.is_some() {
                    restart_agent(ctx)?;
                }
                Ok(result)
            }
            "file.get" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                let file = get_str(&d, "file")?;
                validate_persona_name(&name)?;
                validate_persona_file(&file)?;
                self.cmd_file_get(workspace, &name, &file)
            }
            "file.save" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                let file = get_str(&d, "file")?;
                let content = get_str(&d, "content")?;
                validate_persona_name(&name)?;
                validate_persona_file(&file)?;
                let result = self.cmd_file_save(workspace, &name, &file, &content)?;
                trigger_persona_eval(workspace, ctx);
                Ok(result)
            }
            "shop.browse" => self.cmd_shop_browse(workspace, data.as_ref()).await,
            "shop.search" => self.cmd_shop_search(workspace, data.as_ref()).await,
            "shop.refresh" => {
                invalidate_cache();
                match refresh_fetch_tree().await {
                    Ok(_) => Ok(Some(
                        serde_json::json!({"refreshed": true, "source": "live"}),
                    )),
                    Err(e) => {
                        let class = classify_github_error(&e);
                        let label = snapshot_tree().map(|(_, entry)| entry.short_label());
                        Err(refresh_failure_message(class, label.as_deref()))
                    }
                }
            }
            "shop.preview" => {
                let source_key = resolve_shop_source(data.as_ref())?;
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                // 闸补齐（2026-11-15 逐行复查）：builtin live 臂用裸 id 拼
                // raw.githubusercontent.com 直链，未校验 id（`/`、`..`、`?`）
                // 可改写 URL 路径/查询——读任意公共 raw 内容经本端点回吐。
                // agency 旧臂走 tree 白名单天然免疫，但闸在 dispatch 统一
                // 收口，与 download/hire/hire_expert 四臂同形。
                validate_persona_name(&id)?;
                self.cmd_shop_preview(workspace, &id, source_key).await
            }
            "shop.download" => {
                let source_key = resolve_shop_source(data.as_ref())?;
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                // 物化写 personas/<id>/ → 保留名闸（default 归档不可覆盖）。
                validate_shop_writable_id(&id)?;
                self.cmd_shop_download(workspace, &id, source_key).await
            }
            #[cfg(feature = "cluster")]
            "shop.hire" => {
                let source_key = resolve_shop_source(data.as_ref())?;
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                // builtin 免下载臂物化写 personas/<id>/ → 保留名闸。
                validate_shop_writable_id(&id)?;
                self.cmd_shop_hire(workspace, &id, source_key, ctx).await
            }
            "shop.hire_expert" => {
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                // raw.md 存档物化写 personas/<id>/ → 保留名闸。
                validate_shop_writable_id(&id)?;
                self.cmd_shop_hire_expert(workspace, &id).await
            }
            _ => Err(format!("unknown command: persona.{}", cmd)),
        }
    }
}

fn restart_agent(ctx: &crate::ws_router::RequestContext) -> Result<(), String> {
    let svc = ctx
        .state
        .agent_service
        .as_ref()
        .ok_or("Agent not available")?;
    svc.stop()?;
    tracing::info!("[Persona] Agent stopped for persona switch");
    svc.start()?;
    tracing::info!("[Persona] Agent restarted with new persona files");
    Ok(())
}

impl Default for PersonaHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl PersonaHandler {
    pub fn new() -> Self {
        Self
    }

    // Auto-migration

    fn ensure_initialized(&self, workspace: &str) -> Result<(), String> {
        ensure_persona_store(workspace)
    }

    fn read_active(&self, workspace: &str) -> Result<String, String> {
        let path = resolve_path(workspace, "personas/_active.json")?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read _active.json: {}", e))?;
        let v: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse _active.json: {}", e))?;
        v.get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "invalid _active.json: missing name".to_string())
    }

    fn read_persona_json(
        &self,
        workspace: &str,
        dir_name: &str,
    ) -> Result<serde_json::Value, String> {
        let path = resolve_path(workspace, &format!("personas/{}/PERSONA.json", dir_name))?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read personas/{}/PERSONA.json: {}", dir_name, e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse personas/{}/PERSONA.json: {}", dir_name, e))
    }

    // Local management commands (synchronous)

    fn cmd_current(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        self.ensure_initialized(workspace)?;
        let active = self.read_active(workspace)?;
        let pj = self.read_persona_json(workspace, &active)?;

        let files: Vec<String> = PERSONA_FILES.iter().map(|f| f.to_string()).collect();

        Ok(Some(serde_json::json!({
            "name": pj["name"],
            "emoji": pj["emoji"],
            "description": pj["description"],
            "active_dir": active,
            "files": files,
        })))
    }

    fn cmd_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        self.ensure_initialized(workspace)?;
        let active = self.read_active(workspace)?;

        let personas_dir = resolve_path(workspace, "personas")?;
        let entries = std::fs::read_dir(&personas_dir)
            .map_err(|e| format!("failed to read personas dir: {}", e))?;

        let mut personas = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let pj = match self.read_persona_json(workspace, &dir_name) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let is_active = dir_name == active;
            let is_default = dir_name == "default";

            let mut files = Vec::new();
            for &f in PERSONA_FILES {
                if resolve_path(workspace, &format!("personas/{}/{}", dir_name, f))?.exists() {
                    files.push(f.to_string());
                }
            }

            personas.push(serde_json::json!({
                "dir": dir_name,
                "name": pj["name"],
                "emoji": pj["emoji"],
                "description": pj["description"],
                "is_default": is_default,
                "is_active": is_active,
                "files": files,
            }));
        }

        personas.sort_by(|a, b| {
            let a_default = a["is_default"].as_bool().unwrap_or(false);
            let b_default = b["is_default"].as_bool().unwrap_or(false);
            if a_default {
                return std::cmp::Ordering::Less;
            }
            if b_default {
                return std::cmp::Ordering::Greater;
            }
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        Ok(Some(serde_json::json!({
            "active": active,
            "personas": personas,
        })))
    }

    fn cmd_activate(
        &self,
        workspace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        activate_persona_workspace(workspace, name)?;
        Ok(Some(serde_json::json!({
            "activated": true,
            "name": name,
        })))
    }

    fn cmd_remove(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        // 保留名闸（BUG-20261125-069）：ASCII 大小写不敏感。"Default" 是合法
        // slug（validate_persona_name 无大小写约束），Windows 大小写不敏感
        // 文件系统上 personas/Default/ 与 personas/default/ 是同一目录——精确
        // 匹配拦不住删掉不可再生的 default 归档（Linux 同输入诚实报 not found
        // = 平台分歧行为）。与 validate_shop_writable_id 写侧同口径：跨平台
        // 保守，ASCII 变体一律拒绝。
        if name.eq_ignore_ascii_case("default") {
            return Err("cannot remove default persona".to_string());
        }

        self.ensure_initialized(workspace)?;
        let active = self.read_active(workspace)?;
        // BUG-20261126-093：目录解析在 Windows 上大小写不敏感（case 变体
        // 名命中同一物理目录），active 比对必须同口径——精确 == 会让
        // 「remove Coder 传 coder」跳过 default 恢复，marker 悬空指向已删
        // 人格。ASCII 变体同口径（BUG-20261125-069 保留名闸同款纪律）。
        if active.eq_ignore_ascii_case(name) {
            self.cmd_activate(workspace, "default")?;
        }

        let dir = resolve_path(workspace, &format!("personas/{}", name))?;
        if !dir.exists() {
            return Err(format!("persona '{}' not found", name));
        }
        nemesis_utils::fsutil::remove_dir_all_retry(&dir)
            .map_err(|e| format!("failed to remove persona '{}': {}", name, e))?;

        Ok(Some(serde_json::json!({"removed": true, "name": name})))
    }

    fn cmd_file_get(
        &self,
        workspace: &str,
        name: &str,
        file: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = resolve_path(workspace, &format!("personas/{}/{}", name, file))?;
        if !path.exists() {
            return Ok(Some(serde_json::json!({"name": file, "content": ""})));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {}", file, e))?;
        Ok(Some(serde_json::json!({"name": file, "content": content})))
    }

    fn cmd_file_save(
        &self,
        workspace: &str,
        name: &str,
        file: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = resolve_path(workspace, &format!("personas/{}/{}", name, file))?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 原子写（BUG-20261121-020 家族）：存档副本与下方活体同步两路都
        // 是人格状态文件——撕裂存档被 activate 恢复、撕裂活体直接上线。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o644)
            .map_err(|e| format!("failed to write {}: {}", file, e))?;

        let active = self.read_active(workspace).unwrap_or_default();
        // BUG-20261126-093：激活判定 ASCII 变体同口径——存档副本与活体
        // 是同一人格（Windows 大小写不敏感目录解析命中同一目录），同步
        // 不能因大小写拼写差异漏挂。
        if active.eq_ignore_ascii_case(name) {
            let root_path = resolve_path(workspace, file)?;
            nemesis_utils::write_file_atomic(
                &root_path.to_string_lossy(),
                content.as_bytes(),
                0o644,
            )
            .map_err(|e| format!("failed to sync to root: {}", e))?;
        }

        Ok(Some(serde_json::json!({"saved": true, "name": file})))
    }

    // Shop commands (async, GitHub API)

    async fn cmd_shop_browse(
        &self,
        workspace: &str,
        data: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        // source 参数裁决（2026-11-15 内置独立页拍板）：缺省 = agency 合并臂
        // （行为完全不变——推荐行解析靠它）；builtin = 内置专列。
        let source_key = resolve_shop_source(data)?;
        if source_key == BUILTIN_SOURCE_KEY {
            return self.cmd_shop_browse_builtin(workspace).await;
        }
        let division_filter = data
            .and_then(|d| d.get("division"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (tree, source) = fetch_tree().await?;
        let fm_cache = FM_CACHE.lock().unwrap();
        let rec_titles = recommend_titles_by_ref();

        let mut items = Vec::new();
        // 已知条目集（composite `source/id` 形态）：策展 ref 解析的存在性
        // 真相源——ref 缺席自动不渲染（上游删文件/改名防死链），与 division
        // 过滤无关（过滤前收集全集）。
        let mut known_refs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (path, _size) in &tree {
            let mut entry = parse_agent_from_path(path);
            let agency_key = cache_key(DEFAULT_SOURCE_KEY, &entry.id);
            // Override with cached frontmatter if available（复合键）
            if let Some(fm) = fm_cache.get(&agency_key) {
                entry.name = fm.name.clone();
                entry.emoji = fm.emoji.clone();
                entry.description = fm.description.clone();
            }
            known_refs.insert(agency_key.clone());
            if !division_filter.is_empty() && entry.category != division_filter {
                continue;
            }
            let installed = is_persona_installed(workspace, &entry.id);
            items.push(serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "emoji": entry.emoji,
                "division": entry.category,
                "description": entry.description,
                "installed": installed,
                "source": DEFAULT_SOURCE_KEY,
                "recommended_for": rec_titles
                    .get(&agency_key)
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        drop(fm_cache);

        // builtin 伪源合并（B2）：内嵌模板中目录源没有的人格进列表（自建
        // 内置获得超市曝光，与上游人格同权陈列；division 标「内置」）。
        // known_refs 入集**不受** division 过滤门控（遵守上方「过滤前收集
        // 全集」不变量）：过滤态 browse 的策展解析仍需命中
        // `builtin/bug-analyst`，否则每次 division 过滤都误打
        // 「策展 id 错或上游改名/删除」假 WARN（2026-11-15 逐行复查修）。
        for id in builtin_persona_ids() {
            let composite = format!("{BUILTIN_SOURCE_KEY}/{id}");
            if known_refs.contains(&composite) {
                continue; // 目录源已有同名条目 → 不重复陈列（集合去重）
            }
            let Some(raw) = builtin_persona_raw(&id) else {
                continue; // 模板缺 raw.md（发行物损坏）诚实跳过
            };
            known_refs.insert(composite.clone());
            if !division_filter.is_empty() && division_filter != "内置" {
                continue; // 过滤态：条目不进列表，ref 已入全集
            }
            let fm = parse_frontmatter(raw);
            let installed = is_persona_installed(workspace, &id);
            items.push(serde_json::json!({
                "id": id,
                "name": fm.as_ref().map(|f| f.name.as_str()).unwrap_or(&id),
                "emoji": fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖"),
                "division": "内置",
                "description": fm.as_ref().map(|f| f.description.as_str()).unwrap_or(""),
                "installed": installed,
                "source": BUILTIN_SOURCE_KEY,
                "recommended_for": rec_titles.get(&composite).cloned().unwrap_or_default(),
            }));
        }

        // 推荐橱窗结构（服务端管策展顺序与内容，前端零策展逻辑；ref 逐一
        // 解析到已知条目集，缺席自动不渲染——不做假分类不留死链；丢弃
        // loud WARN 可观测，策展 id 拼错/上游改名第一时间在日志可见）。
        let recommend_sections: Vec<serde_json::Value> = RECOMMEND_SECTIONS
            .iter()
            .map(|s| {
                let (refs, dropped): (Vec<String>, Vec<String>) = s
                    .refs
                    .iter()
                    .map(|r| normalize_recommend_ref(r))
                    .partition(|r| known_refs.contains(r));
                if !dropped.is_empty() {
                    tracing::warn!(
                        "[Persona] 推荐 '{}' 有 {} 个 ref 未命中上游条目（策展 id 错或上游改名/删除）：{}",
                        s.title,
                        dropped.len(),
                        dropped.join(", ")
                    );
                }
                serde_json::json!({
                    "key": s.key,
                    "title": s.title,
                    "tier": recommend_section_tier(s.key),
                    "division": s.division,
                    "refs": refs,
                })
            })
            .collect();

        Ok(Some(serde_json::json!({
            "items": items,
            "source": source.as_str(),
            "snapshot_info": source.info_json(),
            "recommend_sections": recommend_sections,
        })))
    }

    /// 内置人格专列（2026-11-15 内置独立页拍板 ③browse 双臂）：live 目录
    /// 清单 + raw fm → items；live 失败（配额/离线）→ 编译期内嵌清单兜底，
    /// `source` 响应字段诚实标注 live / embedded（snapshot_info 携带人读
    /// label——与 agency 的 live/snapshot 注记同形纪律）。fm 逐人格拉取
    /// （raw 直链不占 API 配额），单格失败不炸整页——卡片名回落 id（与
    /// agency 无 fm 同形态）。
    async fn cmd_shop_browse_builtin(
        &self,
        workspace: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let rec_titles = recommend_titles_by_ref();
        let (ids, source_label, snapshot_info) = match fetch_builtin_list_online().await {
            Ok(ids) => (ids, "live", serde_json::Value::Null),
            Err(e) => {
                let class = classify_github_error(&e);
                let ids = builtin_persona_ids();
                let label = shop_source(BUILTIN_SOURCE_KEY)
                    .map(|s| s.label)
                    .unwrap_or("builtin");
                tracing::warn!(
                    "[Persona] {}源 live 不可达（{class}），降级编译期内嵌模板（{} 个人格）",
                    label,
                    ids.len()
                );
                let n = ids.len();
                (
                    ids,
                    "embedded",
                    serde_json::json!({
                        "label": format!("编译期内嵌模板（{n} 个人格）"),
                    }),
                )
            }
        };
        let live_ok = source_label == "live";

        // 并发预取（2026-11-16 卡慢 BUG 修复②）：逐卡串行 await 在 live 臂
        // 慢/不可达时把首屏拖成 N×超时放大；buffer_unordered(8) 后墙钟 ≈
        // ⌈N/8⌉×单卡最慢臂（raw 5s 快败把单卡上限从 30s 压到 ~6s）。结果
        // 按位归座，下方装配仍按 ids 顺序（卡片次序稳定，不随完成先后抖动）。
        let mut raws: Vec<Option<Result<String, String>>> = vec![None; ids.len()];
        if live_ok {
            let owned: Vec<(usize, String)> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| (i, id.clone()))
                .collect();
            let mut stream = futures::stream::iter(
                owned
                    .into_iter()
                    .map(|(i, id)| async move { (i, fetch_builtin_raw_online(&id).await) }),
            )
            .buffer_unordered(8);
            while let Some((i, r)) = stream.next().await {
                raws[i] = Some(r);
            }
        }

        let mut items = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let fm = match raws[i].take() {
                Some(Ok(raw)) => parse_frontmatter(&raw),
                Some(Err(e)) => {
                    let class = classify_github_error(&e);
                    tracing::warn!(
                        "[Persona] builtin '{id}' raw 拉取失败（{class}），卡片名回落 id"
                    );
                    parse_frontmatter(builtin_persona_raw(id).unwrap_or(""))
                }
                None => parse_frontmatter(builtin_persona_raw(id).unwrap_or("")),
            };
            let installed = is_persona_installed(workspace, id);
            items.push(serde_json::json!({
                "id": id,
                "name": fm.as_ref().map(|f| f.name.as_str()).unwrap_or(id),
                "emoji": fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖"),
                "division": "内置",
                "description": fm.as_ref().map(|f| f.description.as_str()).unwrap_or(""),
                "installed": installed,
                "source": BUILTIN_SOURCE_KEY,
                "recommended_for": rec_titles
                    .get(&cache_key(BUILTIN_SOURCE_KEY, id))
                    .cloned()
                    .unwrap_or_default(),
            }));
        }

        Ok(Some(serde_json::json!({
            "items": items,
            "source": source_label,
            "snapshot_info": snapshot_info,
            "recommend_sections": [],
        })))
    }

    async fn cmd_shop_search(
        &self,
        workspace: &str,
        data: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let query = data
            .and_then(|d| d.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let (tree, source) = fetch_tree().await?;
        let fm_cache = FM_CACHE.lock().unwrap();
        let rec_titles = recommend_titles_by_ref();

        let mut items = Vec::new();
        // 目录源 id 全集（query 过滤前收集）：builtin 并轨去重的裁决集——
        // 与 browse 合并臂「目录源已有同名条目 → 不重复陈列」同款口径
        //（2026-11-15 二轮逐行复查补：此前 search 无去重，同 id 双源并存
        // 时同名双卡，且下载/雇佣都落同一 personas/<id>/ 目录互扰）。
        let mut agency_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (path, _size) in &tree {
            let mut entry = parse_agent_from_path(path);
            agency_ids.insert(entry.id.clone());
            let agency_key = cache_key(DEFAULT_SOURCE_KEY, &entry.id);
            if let Some(fm) = fm_cache.get(&agency_key) {
                entry.name = fm.name.clone();
                entry.emoji = fm.emoji.clone();
                entry.description = fm.description.clone();
            }
            let haystack = format!("{} {} {}", entry.name, entry.category, entry.id).to_lowercase();
            if !query.is_empty() && !haystack.contains(&query) {
                continue;
            }
            let installed = is_persona_installed(workspace, &entry.id);
            items.push(serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "emoji": entry.emoji,
                "division": entry.category,
                "description": entry.description,
                "installed": installed,
                "source": DEFAULT_SOURCE_KEY,
                "recommended_for": rec_titles.get(&agency_key).cloned().unwrap_or_default(),
            }));
        }
        drop(fm_cache);

        // builtin 伪源并轨搜索（自建内置可被搜到；同款 id/名/描述 haystack）。
        // 同 id 双源去重（agency 优先陈列，见上方 agency_ids 口径注）。
        for id in builtin_persona_ids() {
            if agency_ids.contains(&id) {
                continue;
            }
            let Some(raw) = builtin_persona_raw(&id) else {
                continue;
            };
            let fm = parse_frontmatter(raw);
            let name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or(&id);
            let description = fm.as_ref().map(|f| f.description.as_str()).unwrap_or("");
            let haystack = format!("{name} 内置 {id} {description}").to_lowercase();
            if !query.is_empty() && !haystack.contains(&query) {
                continue;
            }
            let installed = is_persona_installed(workspace, &id);
            let builtin_key = cache_key(BUILTIN_SOURCE_KEY, &id);
            items.push(serde_json::json!({
                "id": id,
                "name": name,
                "emoji": fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖"),
                "division": "内置",
                "description": description,
                "installed": installed,
                "source": BUILTIN_SOURCE_KEY,
                "recommended_for": rec_titles.get(&builtin_key).cloned().unwrap_or_default(),
            }));
        }

        Ok(Some(serde_json::json!({
            "items": items,
            "source": source.as_str(),
            "snapshot_info": source.info_json(),
        })))
    }

    async fn cmd_shop_preview(
        &self,
        workspace: &str,
        id: &str,
        source_key: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // builtin 源取材序（2026-11-15 拍板）：live raw 优先 → 内嵌兜底 →
        // 诚实报错；agency 源 fetch_agent_content（自带快照降级）。
        let (content, src_label, snapshot_info) = if source_key == BUILTIN_SOURCE_KEY {
            match fetch_builtin_raw_online(id).await {
                Ok(c) => (c, BUILTIN_SOURCE_KEY, serde_json::Value::Null),
                Err(e) => match builtin_persona_raw(id) {
                    Some(raw) => {
                        let class = classify_github_error(&e);
                        tracing::warn!(
                            "[Persona] builtin '{id}' live 预览拉取失败（{class}），读编译期内嵌原文兜底"
                        );
                        (
                            raw.to_string(),
                            BUILTIN_SOURCE_KEY,
                            serde_json::json!({"label": "编译期内嵌模板（离线兜底）"}),
                        )
                    }
                    None => return Err(e),
                },
            }
        } else {
            let (content, source) = fetch_agent_content(id).await?;
            (content, DEFAULT_SOURCE_KEY, source.info_json())
        };
        let fm = parse_frontmatter(&content);
        let converted = convert_agent_content(&content);

        let name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or(id);
        let emoji = fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖");

        let installed = is_persona_installed(workspace, id);

        // Build converted files preview
        Ok(Some(serde_json::json!({
            "id": id,
            "name": name,
            "emoji": emoji,
            "installed": installed,
            "source": src_label,
            "snapshot_info": snapshot_info,
            "raw": content,
            "converted": {
                "IDENTITY.md": converted.identity,
                "SOUL.md": converted.soul,
                "AGENT.md": converted.agent_full,
                "TOOLS.md": converted.tools_full,
                "PERSONA.json": converted.persona_json,
            },
        })))
    }

    async fn cmd_shop_download(
        &self,
        workspace: &str,
        id: &str,
        source_key: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // Ensure the persona store (default persona + _active.json) exists so a
        // download never leaves a half-initialized state (personas/<id>/ present
        // but _active.json missing) — that state broke cmd_current/cmd_remove.
        self.ensure_initialized(workspace)?;
        // Check not already installed（安装态 = PERSONA.json 在场，见
        // [`is_persona_installed`]——只雇佣过专家的目录只有 raw.md 原文
        // 存档，不阻塞补下载四文件）
        if is_persona_installed(workspace, id) {
            return Err(format!("persona '{}' already installed", id));
        }

        // builtin 源：live raw → 内嵌兜底（2026-11-15 拍板取材序）；
        // agency 源：在线 → 快照降级 → 转换落盘。
        let (display_name, snapshot_info, src_label) = if source_key == BUILTIN_SOURCE_KEY {
            match fetch_builtin_raw_online(id).await {
                Ok(content) => {
                    write_persona_files(workspace, id, &content)?;
                    let fm = parse_frontmatter(&content);
                    (
                        fm.as_ref()
                            .map(|f| f.name.as_str())
                            .unwrap_or(id)
                            .to_string(),
                        serde_json::Value::Null,
                        BUILTIN_SOURCE_KEY,
                    )
                }
                Err(e) => {
                    let class = classify_github_error(&e);
                    tracing::warn!(
                        "[Persona] builtin '{id}' live 拉取失败（{class}），释放编译期内嵌模板兜底"
                    );
                    release_builtin_persona(workspace, id)?;
                    let raw = builtin_persona_raw(id).ok_or_else(|| {
                        format!("builtin persona '{id}' 模板缺 raw.md（发行物损坏）")
                    })?;
                    let fm = parse_frontmatter(raw);
                    (
                        fm.as_ref()
                            .map(|f| f.name.as_str())
                            .unwrap_or(id)
                            .to_string(),
                        serde_json::json!({"label": "编译期内嵌模板（离线兜底）"}),
                        BUILTIN_SOURCE_KEY,
                    )
                }
            }
        } else {
            let (content, source) = fetch_agent_content(id).await?;
            write_persona_files(workspace, id, &content)?;
            let fm = parse_frontmatter(&content);
            (
                fm.as_ref()
                    .map(|f| f.name.as_str())
                    .unwrap_or(id)
                    .to_string(),
                source.info_json(),
                DEFAULT_SOURCE_KEY,
            )
        };

        Ok(Some(serde_json::json!({
            "downloaded": true,
            "name": display_name,
            "id": id,
            "source": src_label,
            "snapshot_info": snapshot_info,
        })))
    }

    /// 集群聘用：把已安装的本地超市人格映射进 workspace/cluster/ 三件套。
    /// 决策与零丢失契约：集群聘用需求评估与决策。
    /// 前置：必须先 shop.download（决策 4）；覆盖前 zip 备份（决策 5）；
    /// 节点身份只换 name（决策 3）；EXPERTISE 保守不动（决策 2）。
    /// §4.3 source 字段注记：hire 读**本地已下载** personas/{id}（零上游
    /// 依赖），数据既非 live 也非 snapshot——source/snapshot_info 不适用
    /// （值集没有第三个值；内容来源由先行的 download 响应承载）。
    #[cfg(feature = "cluster")]
    async fn cmd_shop_hire(
        &self,
        workspace: &str,
        id: &str,
        source_key: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = resolve_path(workspace, &format!("personas/{}", id))?;
        if !is_persona_installed(workspace, id) {
            // builtin 源免先下载（B2）：取材序 live raw → 内嵌兜底（2026-11-15
            // 拍板）——live 命中走转换落盘拿最新四件套，失败释放内嵌模板；
            // 目录源维持「必须先下载」前置。
            if source_key == BUILTIN_SOURCE_KEY {
                match fetch_builtin_raw_online(id).await {
                    Ok(content) => write_persona_files(workspace, id, &content)?,
                    Err(e) => {
                        let class = classify_github_error(&e);
                        tracing::warn!(
                            "[Persona] builtin '{id}' live 拉取失败（{class}），免下载直聘用释放内嵌模板兜底"
                        );
                        release_builtin_persona(workspace, id)?;
                    }
                }
            } else {
                return Err(format!(
                    "persona '{}' not installed — 集群聘用必须先下载（shop.download）",
                    id
                ));
            }
        }
        let (name, backup) = hire_write_cluster_persona(workspace, &dir, id)?;

        // runtime 内存态同步（persona_apply 同款「runtime + peers.toml 双写」
        // 纪律，只换 name——决策 3）：peers.toml 是持久真相源（重启重读），
        // runtime 同步消掉「重启失败/未完成时 nodes.list/拓扑仍显旧名」的
        // 显示失配窗口。handle 缺席（集群未装配）best-effort 跳过。
        if let Some(cluster) = ctx.state.cluster.clone() {
            cluster.set_node_name(&name);
        }

        // Best-effort reload（语义与 cluster.rs persona_apply 一致）：只在集群
        // 运行中重启；从不为了应用人格而启动一个停止的集群。
        let mut reloaded = false;
        let mut note = String::new();
        if let Some(svc) = ctx.state.cluster_service.as_ref() {
            let running = nemesis_services::bot_service::LifecycleService::is_running(svc.as_ref());
            if running {
                match svc.stop().and_then(|_| svc.start()) {
                    Ok(()) => reloaded = true,
                    Err(e) => {
                        tracing::warn!("[Persona] shop.hire cluster reload failed: {}", e);
                        note = format!("（重启集群以加载新人格失败：{e}，将在下次启动集群时生效）");
                    }
                }
            } else {
                note = "（集群当前未运行，新人格将在下次启动集群时生效）".to_string();
            }
        } else {
            note = "（集群服务不可用，新人格将在下次启动集群时生效）".to_string();
        }

        tracing::info!(persona = %id, name = %name, reloaded, "[Persona] hired into cluster");
        Ok(Some(serde_json::json!({
            "hired": true,
            "name": name,
            "id": id,
            "backed_up": backup,
            "reloaded": reloaded,
            "note": note,
        })))
    }

    /// 雇佣专家（2026-11-13 专家子代理批次③；2026-11-15 原文直用改版）：
    /// 把超市人格映射成单机专家（workspace/experts/）。**原文直用，不再
    /// 要求先下载**——本地 raw.md 缺席时在线拉取并保存后再雇佣（取材顺序
    /// 见 [`expert_hire_payload`]）；prompt = 原文正文（剥 frontmatter），
    /// name/summary 从原文 frontmatter 解析；slug = 人格目录 id（下载面
    /// 已 FS 归一，hire 面 slug_guard 复核）；幂等重雇（同 slug 覆盖，
    /// model/enabled 重置出厂态）。无 feature 闸——专家面恒编译；experts
    /// 桥缺席（CLI/headless 测试态）诚实报错。
    async fn cmd_shop_hire_expert(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let payload = expert_hire_payload(workspace, id).await?;
        let bridge = crate::handlers::experts::experts_bridge()?;
        let entry = bridge.hire(&payload)?;
        tracing::info!(persona = %id, "[Persona] hired as expert");
        Ok(Some(
            serde_json::json!({ "hired": true, "id": id, "expert": entry }),
        ))
    }
}

/// 安装态判断（browse/search/preview「已安装」徽标 + 下载/集群聘用前置的
/// 单一口径）：personas/<id>/PERSONA.json 在场。只雇佣专家落盘的目录只有
/// raw.md 原文存档（无四文件）——不算已安装，防超市误标「已安装」诱导
/// 激活出缺四文件的空人格；同时不阻塞「先雇佣后补下载」的合法路径。
fn is_persona_installed(workspace: &str, id: &str) -> bool {
    resolve_path(workspace, &format!("personas/{}/PERSONA.json", id))
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// 剥掉开头 frontmatter 块返回正文（无 frontmatter / 未闭合原样返回）。
/// 与 [`parse_frontmatter`]/`parse_sections` 同款判定口径。
fn strip_frontmatter_body(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_first = &trimmed[3..];
    let Some(end) = after_first.find("---") else {
        return content;
    };
    after_first[end + 3..].trim_start_matches(['\n', '\r'])
}

/// 雇佣专家载荷构造（2026-11-15 原文直用改版）：专家 prompt = 上游 .md
/// **原文正文**（剥 frontmatter），零拼接零转换——原文是作者写好的完整
/// 专家人格，拆桶再回组有损（丢段/乱序/混入系统模板）。取材顺序（2026-11-15
/// 自仓源拍板）：本地 personas/<id>/raw.md（离线可用）→ builtin live raw
///（落盘存档——加内置人格不重编即可雇佣）→ builtin 内嵌原文（编译期模板，
/// 零写盘——四件套由下载/集群聘用臂按需送达，雇佣面只读）→ agency 在线
/// 拉取（自带快照降级）并**落盘保存**再雇佣（此后原文永远有本地存档）。
/// name/description 从原文 frontmatter 解析（只雇佣未下载的目录没有
/// PERSONA.json）；name 空/缺席回落 id。
async fn expert_hire_payload(workspace: &str, id: &str) -> Result<serde_json::Value, String> {
    let dir = resolve_path(workspace, &format!("personas/{}", id))?;
    let raw_path = dir.join("raw.md");
    let raw = if raw_path.exists() {
        std::fs::read_to_string(&raw_path)
            .map_err(|e| format!("failed to read raw.md（原文存档读失败）: {e}"))?
    } else {
        match fetch_builtin_raw_online(id).await {
            Ok(content) => {
                std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir: {e}"))?;
                // 原子写（BUG-20261121-020 家族：raw.md 是雇佣取材真相源存档）。
                nemesis_utils::write_file_atomic(
                    &raw_path.to_string_lossy(),
                    content.as_bytes(),
                    0o644,
                )
                .map_err(|e| format!("failed to write raw.md（保存原文失败）: {e}"))?;
                content
            }
            Err(live_err) => match builtin_persona_raw(id) {
                Some(embedded) => {
                    let class = classify_github_error(&live_err);
                    tracing::warn!(
                        "[Persona] builtin '{id}' live 拉取失败（{class}），雇佣走编译期内嵌原文（零写盘）"
                    );
                    embedded.to_string()
                }
                None => {
                    let (content, _source) = fetch_agent_content(id).await?;
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| format!("failed to create dir: {e}"))?;
                    // 原子写（BUG-20261121-020 家族）。
                    nemesis_utils::write_file_atomic(
                        &raw_path.to_string_lossy(),
                        content.as_bytes(),
                        0o644,
                    )
                    .map_err(|e| format!("failed to write raw.md（保存原文失败）: {e}"))?;
                    content
                }
            },
        }
    };
    let fm = parse_frontmatter(&raw);
    let prompt = strip_frontmatter_body(&raw).trim().to_string();
    if prompt.is_empty() {
        return Err(format!(
            "persona '{id}' 原文正文为空 — 没有可雇佣的专家提示词"
        ));
    }
    let name = fm
        .as_ref()
        .map(|f| f.name.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(id);
    let summary = fm.as_ref().map(|f| f.description.as_str()).unwrap_or("");
    Ok(serde_json::json!({
        "slug": id,
        "name": name,
        "summary": summary,
        "tags": [],
        "prompt": prompt,
        "hired_from": id,
    }))
}

// Cluster hire（集群聘用）—— 映射/备份/身份纯函数 + 写盘主体
// 全体随 cluster feature 门控：write 盘主体跨调 cluster handler（feature 门
// 模块），映射纯函数虽不依赖 cluster，随主体同进出避免 feature-off 死代码。

/// 剥离系统模板前缀，取人格自有附加段。超市下载写入形态是
/// `format!("{}\n{}", DEFAULT_*, extra)`；strip 失败（用户手改过 / 模板
/// 升级导致前缀不匹配）时**整个文件原样返回**——宁可把系统模板文字也
/// 塞进 SOUL，绝不丢人格内容（零丢失兜底）。
#[cfg(feature = "cluster")]
fn strip_system_prefix(file_content: &str, system_default: &str) -> String {
    match file_content.strip_prefix(system_default) {
        Some(rest) => rest.trim_start_matches('\n').to_string(),
        None => file_content.to_string(),
    }
}

/// 聘用映射：cluster/SOUL.md = 原 SOUL + 专用工作流尾节 + 工具偏好尾节
///（非空才追加，`---` 分隔）。IDENTITY 1:1 由调用方直接拷贝。
#[cfg(feature = "cluster")]
fn build_cluster_soul(soul_md: &str, agent_md: &str, tools_md: &str) -> String {
    let agent_extra = strip_system_prefix(agent_md, DEFAULT_AGENT_MD);
    let tools_extra = strip_system_prefix(tools_md, DEFAULT_TOOLS_MD);
    let mut out = soul_md.trim_end().to_string();
    for extra in [agent_extra, tools_extra] {
        if extra.trim().is_empty() {
            continue;
        }
        out.push_str("\n\n---\n\n");
        out.push_str(extra.trim_end());
    }
    out.push('\n');
    out
}

/// PERSONA.json 的 name 字段（frontmatter 展示名）；缺失/空/非法回落 id。
#[cfg(feature = "cluster")]
fn read_persona_display_name(dir: &std::path::Path, id: &str) -> String {
    std::fs::read_to_string(dir.join("PERSONA.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| id.to_string())
}

/// 覆盖前备份 hire 即将触碰的全部既有文件到 `cluster/backups/persona_<毫秒
/// 时间戳>.zip`：人格三件（IDENTITY/SOUL/EXPERTISE，存在哪个备哪个）+
/// peers.toml（hire 会改 [node].name，回滚需要它）。全部不存在（含
/// cluster/ 目录不存在）→ Ok(None)，无需备份。**字节级读取**（fs::read，
/// 非 UTF-8 手改文件照备不丢）；存在但读不出（权限/占用）→ Err abort，
/// fail-closed：绝不在无备份的情况下覆盖旧文件。同毫秒重入以 `_N` 后缀
/// 递增防覆盖（备份被覆盖 = 丢备份）。全留不滚动（单份几 KB，决策 5）。
#[cfg(feature = "cluster")]
fn backup_cluster_persona_files(workspace: &str) -> Result<Option<String>, String> {
    const BACKUP_FILES: [&str; 4] = ["IDENTITY.md", "SOUL.md", "EXPERTISE.md", "peers.toml"];
    let mut present: Vec<(&str, Vec<u8>)> = Vec::new();
    for f in BACKUP_FILES {
        let path = resolve_path(workspace, &format!("cluster/{f}"))?;
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| {
                format!("备份前读取 cluster/{f} 失败，拒绝在无备份情况下覆盖旧文件: {e}")
            })?;
            present.push((f, bytes));
        }
    }
    if present.is_empty() {
        return Ok(None);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut rel = format!("cluster/backups/persona_{ts}.zip");
    let mut n = 1u32;
    while resolve_path(workspace, &rel)?.exists() {
        rel = format!("cluster/backups/persona_{ts}_{n}.zip");
        n += 1;
    }
    let zip_path = resolve_path(workspace, &rel)?;
    if let Some(parent) = zip_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建备份目录失败: {e}"))?;
    }
    let file = std::fs::File::create(&zip_path).map_err(|e| format!("创建备份文件失败: {e}"))?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in &present {
        zw.start_file(*name, options)
            .map_err(|e| format!("写入备份条目 {name} 失败: {e}"))?;
        std::io::Write::write_all(&mut zw, content)
            .map_err(|e| format!("写入备份条目 {name} 失败: {e}"))?;
    }
    // finish 返回的 BufWriter 若靠 drop 收尾，缓冲里最后
    // ≤8KiB 的 flush 错误会被 std 静默吞掉——本函数返回 Ok 却留下截断的
    // 备份 zip，调用方照常覆盖旧人格文件，架空「绝不在无备份的情况下
    // 覆盖旧文件」的 fail-closed 契约（截断备份比没有更糟：虚假信心）。
    // 对齐 纪律：显式落盘报错 + sync_all 后才算备份成功（备份必须
    // 先于紧随其后的覆盖写，确保掉电也存活）。
    let inner = zw.finish().map_err(|e| format!("收尾备份 zip 失败: {e}"))?;
    inner
        .into_inner()
        .map_err(|e| format!("备份 zip 落盘失败: {}", e.into_error()))?
        .sync_all()
        .map_err(|e| format!("备份 zip sync 失败: {e}"))?;
    Ok(Some(rel))
}

/// 聘用写盘主体（无 ctx 的可测核心）：备份（hire 触碰的全部既有文件）→
/// cluster/IDENTITY.md（1:1 拷贝）→ cluster/SOUL.md（拼接）→ peers.toml
/// [node] 只换 name。EXPERTISE 保守不动（决策 2）。返回 (展示名, 备份相对
/// 路径 Option)。
#[cfg(feature = "cluster")]
fn hire_write_cluster_persona(
    workspace: &str,
    persona_dir: &std::path::Path,
    id: &str,
) -> Result<(String, Option<String>), String> {
    let read = |f: &str| -> Result<String, String> {
        std::fs::read_to_string(persona_dir.join(f))
            .map_err(|e| format!("读取 personas/{id}/{f} 失败: {e}"))
    };
    let identity = read("IDENTITY.md")?;
    let soul = read("SOUL.md")?;
    let agent = read("AGENT.md")?;
    let tools = read("TOOLS.md")?;

    let backup = backup_cluster_persona_files(workspace)?;

    crate::handlers::cluster::ClusterHandler::write_cluster_file(
        workspace,
        "IDENTITY.md",
        &identity,
    )?;
    crate::handlers::cluster::ClusterHandler::write_cluster_file(
        workspace,
        "SOUL.md",
        &build_cluster_soul(&soul, &agent, &tools),
    )?;

    // 节点身份：只换 name（纯展示；tags/role/professions 等派发硬条件绝不
    // 碰——决策 3，超市标签不准且 tags 是 matcher 交集条件）。
    let name = read_persona_display_name(persona_dir, id);
    crate::handlers::cluster::update_peers_node(workspace, |node| {
        node.name = name.clone();
        Ok(())
    })?;
    Ok((name, backup))
}

// Tests

// pub(crate)：仅测试构建生效——SHOP_TEST_LOCK（shop.* 缓存串行锁）需要被
// sibling 测试模块（persona_extra/persona_more）引用，防全局缓存互踩。
#[cfg(test)]
pub(crate) mod tests;

// (, quality-hardening goal 冲刺 web): uncovered
// helpers + offline cache-injected shop paths + activate/restore archive
// arms.
#[cfg(test)]
mod s10b_tests;

// AGT 覆盖率批次：Default impl；其余零区为网络绑定
// （GITHUB_API 硬编码 https 无注入缝）或防御臂，定性见 agt_tests 文件头注。
#[cfg(test)]
mod agt_tests;

// 集群聘用：strip/build 映射纯函数 + 备份 zip + 写盘主体
//（无 ctx，temp workspace 文件级验证）。随 cluster feature 门控。
#[cfg(all(test, feature = "cluster"))]
mod hire_tests;

// 雇佣专家（shop.hire_expert）：expert_hire_payload 纯函数
//（无 ctx 无桥依赖，temp workspace 文件级验证）。无 feature 闸（专家面恒编译）。
#[cfg(test)]
mod hire_expert_tests;

// 推荐橱窗 + 源参数 + builtin 伪源（2026-11-15 策展映射，方案 A）：
// sections 结构/顺序/防死链 + 投影注入 + builtin 下载释放/雇佣内嵌臂
// + 漂移锁与开发期生成闸（NEMESIS_GEN_BUILTIN=1）。
#[cfg(test)]
mod recommend_tests;

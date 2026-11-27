//! 知识库重定向运行时面（天工系统 v1）——声明注入 + 路径重定向。
//!
//! 程序不理解「知识库」：用户配置声明（`config.knowledge.json` 类型化
//! 字段，schema 在 nemesis-config::knowledge）→ 本模块按固定协议机械执行
//! 两件事 → LLM 从注入声明与 TIANGONG.md 内容获得全部语义：
//!
//! 1. **存在性声明**（[`knowledge_decl_section`]）：build_messages 新增并列
//!    小段（经 AgentLoop `knowledge_section_source` 闭包注入，闭包缺席 =
//!    段缺席零字节）。pointer = 只报「TIANGONG.md 存在」；full = 根层
//!    TIANGONG.md 全文每轮注入（带生效路径）。**永不自动注入
//!    AGENTS.md/CLAUDE.md 旁路侧全文**（等 LLM 自己读）。
//! 2. **路径重定向**（[`redirect_instruction_path`]）：文件工具
//!    （read_file / write_file / edit_file 含 append / multiedit）入口
//!    纯函数——路径以三件套文件名结尾 且 父目录 ∈ 某挂载 code_root 子树
//!    且 **本地不存在** → 改写为 `info_root` 镜像路径。本地存在 = 本地
//!    生效（overlay 天然成立，现状零变化）。
//!
//! 开关 fail-closed：`enabled=false`（默认 / 配置缺席 / 坏 JSON）时声明
//! 返回 None、重定向原样返回——系统行为与功能不存在时逐字节一致。
//!
//! 热生效：[`KnowledgeHandle`] 按配置文件 mtime 缓存——knowledge.set 写盘
//! 后下一次工具调用 / 下一轮 build_messages 自动重读，无需重启。

use nemesis_config::knowledge::{
    KnowledgeConfig, KnowledgePair, TIANGONG_LOAD_FULL, path_components_prefix_of,
    path_strip_prefix_ci,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// 三件套：AGENTS.md / CLAUDE.md / TIANGONG.md 同规则（决策 2）。
pub const INSTRUCTION_FILE_NAMES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "TIANGONG.md"];

/// 天工引导文件名（声明文案与 full 模式的固定目标）。
pub const TIANGONG_FILE_NAME: &str = "TIANGONG.md";

/// full 模式全文注入的体积上限（第九轮横扫 2026-11-27，字节）。正常
/// 维护者内容（分层地图 + 规则，量级对齐常见 CLAUDE.md 的 10–40KB）远在
/// 线下；超限 = 异常形态（维护者失误 / LLM 异常写入），降级 pointer +
/// WARN（见 [`knowledge_decl_section`] full 分支）。
pub const TIANGONG_FULL_MAX_BYTES: usize = 64 * 1024;

/// 根层在层集合内部的标识（第七轮横扫 2026-11-27）：哨兵 = 空串——
/// 子目录文件名永不为空，不会与 [`knowledge_decl_section`] 的层合并键
/// 冲突。此前用中文字面「根目录」作键：用户 code_root 下恰有同名子目录
/// 时两层合并，子目录假命中触发 root_only 零括注（LLM 以为根层有
/// TIANGONG → 直奔根层 File not found——第五轮括注如实化修掉的行为从
/// 合并键这条路绕回来）。渲染时才映射显示名「根目录」。
const ROOT_LAYER_KEY: &str = "";

/// 三件套文件名判定（大小写敏感——Windows 文件系统大小写不敏感时
/// `agents.md` 物理命中同一文件，但 LLM 按文档惯例发送精确名；宽匹配
/// 带来的误重定向面大于收益，从紧）。
pub fn is_instruction_file_name(name: &str) -> bool {
    INSTRUCTION_FILE_NAMES.contains(&name)
}

/// 知识库配置运行时句柄：配置路径 + mtime 缓存。
///
/// 每次工具调用 / 每轮 build_messages 走 [`Self::get`]——mtime 没变直接
/// 用缓存（一次 metadata 的成本），变了重读。读盘 fail-closed（缺席 /
/// 坏 JSON = Default 关闭态，与 load_knowledge_config 契约一致）。
/// 恒装形态：配置缺席时句柄照常构造（get 恒返回关闭 Default）——装配点
/// 无分支，行为与「不装」逐字节一致。
pub struct KnowledgeHandle {
    config_path: PathBuf,
    cache: parking_lot::RwLock<Option<(Option<SystemTime>, Arc<KnowledgeConfig>)>>,
}

impl KnowledgeHandle {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            cache: parking_lot::RwLock::new(None),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// 当前生效配置（mtime 缓存；fail-closed）。
    pub fn get(&self) -> Arc<KnowledgeConfig> {
        let mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        {
            let cache = self.cache.read();
            if let Some((cached_mtime, cfg)) = cache.as_ref()
                && *cached_mtime == mtime
            {
                return Arc::clone(cfg);
            }
        }
        let cfg = Arc::new(nemesis_config::knowledge::load_knowledge_config(
            &self.config_path,
        ));
        *self.cache.write() = Some((mtime, Arc::clone(&cfg)));
        cfg
    }

    /// 知识库配对双根围栏放行集合（天工系统 v1）——**单一真相源**（第四轮
    /// 横扫 2026-11-27 自 bot 侧下沉：全局盒 ini 的写穿透清单需要第五消费点
    /// nemesis-web（WSAPI `sandbox.set_network` 整文件重写 ini 时不能抹线），
    /// lib crate 拿不到 bot 的 pub(crate) 函数）。enabled=false → 空集（功能
    /// 不存在零放行）；enabled 时全部 code_root + info_root 排序去重（防
    /// read_dir 顺序语义 + 多对共享目录重复放行）。
    pub fn fence_roots(&self) -> Vec<PathBuf> {
        let c = self.get();
        if !c.enabled {
            return Vec::new();
        }
        let mut roots: Vec<PathBuf> = c
            .pairs
            .iter()
            .flat_map(|p| [PathBuf::from(&p.code_root), PathBuf::from(&p.info_root)])
            .map(normalize_root_separators)
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

/// 围栏/写穿透根分隔符归一（第五轮横扫 2026-11-27）：用户 JSON 配置里的
/// 正斜杠形态（前端占位示例即此形态）在围栏组件匹配 / 文件 IO / executor
/// env 侧恒与反斜杠等价，但 Sandboxie ini `OpenFilePath` 行是字面前缀
/// 匹配——混合分隔符行（`D:/kb\*`）匹配语义未经验证。Windows 统一归一
/// 为反斜杠（`fence_roots` 是全部消费点的单一真相源：boundary extra_roots
/// / executor env / ini 写穿透 / WSAPI set_network 同源受益）；非 Windows
/// 原样（正斜杠即分隔符）。纯字符串归一不做 canonicalize——根目录允许
/// 尚不存在（校验只查绝对路径形态），canonicalize 会失败。
fn normalize_root_separators(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(p.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        p
    }
}

/// 全局盒知识库写穿透**全景清单**（第八轮横扫 2026-11-27）：home 域内
/// 全部知识库实体的围栏双根并集——workspace 级配置 + `<workspace>/agents/`
/// 逐 agent 的 `home/config/config.knowledge.json`，各过
/// [`KnowledgeHandle::fence_roots`]（缺席 / disabled / 坏 JSON = 空集零
/// 贡献）后合并排序去重。
///
/// 为什么存在（组合缺陷修复）：全局 Sandboxie 盒（NemesisBox）ini 是
/// **跨实体共享可变状态**——CLI `sandbox start`、WSAPI `sandbox.set_network`、
/// 装配期 `upsert_global_box_section`（main / 项目 / workflow world 各臂
/// 共用）都会**段级重写** `[NemesisBox]`。第四轮横扫让各写入方带「自己
/// 实体」的双根清单，靠「等值重写自愈漂移」防坏——该假设只在单实体恒定
/// 时成立：多实体各传不同清单互相抹线（实体 A 的写穿透行被实体 B 的
/// upsert 摘除 → A 的镜像写困盒静默丢失，`sandbox pending/commit` 只枚举
/// 工作区子树救不回；knowledge=None 的臂更把全部线摘空）。修复 = 本函数
/// 成为**唯一清单**：域内所有写入方恒传同一集合，互抹消灭、第四轮的
/// 「等值重写自愈漂移」语义恢复成立。per-agent 专属盒（NemesisAgent_*，
/// `upsert_agent_box_section`）段名独立不在本函数射程，保持实体级清单。
///
/// `workspace` = 主 workspace 锚（gateway canonical = `home.join("workspace")`；
/// CLI sandbox / WSAPI set_network / exec_world 盒臂全部调用方同锚）。
/// headless `run --workspace DIR` 自定义工作区形态的实体级配置不在此清单
/// （锚不可达）——该形态全局盒镜像写穿透不生效，诚实边界备案（v1）。
pub fn global_write_through_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = KnowledgeHandle::new(
        nemesis_path::resolve_workspace_knowledge_config_path_in_workspace(workspace),
    )
    .fence_roots();
    let agents_root = nemesis_path::resolve_agents_root_in_workspace(workspace);
    let Ok(rd) = std::fs::read_dir(&agents_root) else {
        return roots;
    };
    let mut agent_dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    agent_dirs.sort();
    for dir in agent_dirs {
        let Some(agent_id) = dir.file_name() else {
            continue;
        };
        roots.extend(
            KnowledgeHandle::new(
                nemesis_path::resolve_agent_knowledge_config_path_in_workspace(
                    workspace,
                    &agent_id.to_string_lossy(),
                ),
            )
            .fence_roots(),
        );
    }
    roots.sort();
    roots.dedup();
    roots
}

/// 工具侧便捷入口：重定向能力在场时走纯函数，缺席（headless / 测试 /
/// 基线注册形态）原样返回——与旧「无重定向」行为逐字节一致。
pub fn maybe_redirect_path(path: &Path, handle: Option<&KnowledgeHandle>) -> PathBuf {
    match handle {
        Some(h) => redirect_instruction_path(path, &h.get()),
        None => path.to_path_buf(),
    }
}

/// 路径重定向纯函数（决策 13）。
///
/// 路径以三件套文件名结尾 且 父目录 ∈ 某挂载 code_root 子树 且
/// **本地不存在** → 改写为 `info_root + 相对后缀`；否则原样返回。
/// 本地存在即用本地（实时判断 = overlay 天然成立）；本地存在的判断
/// 在子树匹配**之后**（非挂载目录三件套不花 stat）。
pub fn redirect_instruction_path(path: &Path, cfg: &KnowledgeConfig) -> PathBuf {
    if !cfg.enabled {
        return path.to_path_buf();
    }
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    if !is_instruction_file_name(&name.to_string_lossy()) {
        return path.to_path_buf();
    }
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    // BUG-20261127-047：parent 先词法归一再做子树判定。组件级前缀匹配不折
    // `..`——净 `..` 超出后缀深度的路径（真实落点在 code 子树外，如
    // `code/a/b/../../..` ≡ 子树外的兄弟目录）照样命中前缀，镜像 rel 含
    // 越根 `..` → 归一后越出 info_root。链路语义：围栏前臂（绝对入参）
    // 的越界产物被围栏 backstop 拦住；**围栏后臂（相对入参围栏放行后二次
    // 重定向）输出无人复检** = 越授权根读写原语。归一后 parent 内不可能
    // 残留可折叠 `..`（每个 `..` 要么折叠、要么落在最前不成前缀）→ 后缀
    // rel 恒无 `..` → 镜像恒在 info_root 内（info_root 本就是授权根——
    // fence_roots 放行面）；归一后落点出子树的路径不再命中 → 透传真实
    // 路径（overlay 语义：子树外本来就不该重定向）。合法形态（`..` 折叠
    // 后仍落子树内，如 `code/src/../AGENTS.md`）行为不变。
    let parent = normalize_lexical(parent.to_path_buf());
    for pair in &cfg.pairs {
        let code = Path::new(&pair.code_root);
        if !path_components_prefix_of(code, &parent) {
            continue;
        }
        // overlay：本地存在 = 本地生效（现状零变化）。
        if path.exists() {
            return path.to_path_buf();
        }
        let Some(rel) = path_strip_prefix_ci(code, &parent) else {
            continue;
        };
        // 镜像路径词法归一（第五轮横扫 2026-11-27 + BUG-20261127-047）：
        // LLM 入参可能带 `.`/`..` 组件——BUG-047 修复后 rel 恒无 `..`
        // （parent 归一后才做子树判定，见循环上方），此处归一只剩剔除
        // 残余 `.` 的作用，保留为防线冗余。归一的意义史：未归一时镜像
        // 含 `..` 会被 checkpoint 的 safe_path 诚实拒绝（快照读 None），
        // rewind 恢复把该声明当「不存在」走删除臂，误删已有镜像文件；
        // 归一后声明路径即真实路径，快照/恢复/指纹全链对齐（OS 语义
        // 等价——纯词法折叠，镜像路径不含 symlink 间接）。
        let mirror = Path::new(&pair.info_root).join(rel).join(name);
        return normalize_lexical(mirror);
    }
    path.to_path_buf()
}

/// 路径词法归一：`.` 组件剔除、`..` 组件就地折叠（纯词法，不解析
/// symlink）。前导/越根的 `..` 保留（OS 语义：相对路径前导 `..` 合法、
/// 盘根之上即盘根）。
fn normalize_lexical(p: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(c),
            },
            c => out.push(c),
        }
    }
    out.iter().collect()
}

/// 某 pair 的镜像层是否存在 TIANGONG.md。
fn mirror_tiangong_present(layer: &Path, pair: &KnowledgePair) -> bool {
    let code = Path::new(&pair.code_root);
    let Some(rel) = path_strip_prefix_ci(code, layer) else {
        return false;
    };
    Path::new(&pair.info_root)
        .join(rel)
        .join(TIANGONG_FILE_NAME)
        .exists()
}

/// 层生效 TIANGONG 文件（overlay：本地优先 → 镜像 fallback，镜像按
/// pairs 顺序第一个真有文件的 pair）。返回 (生效路径, 来源 pair 序号
/// Option——本地来源 None)。
fn effective_tiangong(layer: &Path, cfg: &KnowledgeConfig) -> Option<(PathBuf, Option<usize>)> {
    let local = layer.join(TIANGONG_FILE_NAME);
    if local.exists() {
        return Some((local, None));
    }
    for (i, pair) in cfg.pairs.iter().enumerate() {
        let code = Path::new(&pair.code_root);
        let Some(rel) = path_strip_prefix_ci(code, layer) else {
            continue;
        };
        let m = Path::new(&pair.info_root)
            .join(rel)
            .join(TIANGONG_FILE_NAME);
        if m.exists() {
            return Some((m, Some(i)));
        }
    }
    None
}

/// 存在性声明 section（决策 3 文案，用户定稿）。
///
/// 层锚定：**每个 pair 的 code_root 侧**（项目层级——「根目录」=
/// code_root 本身，「src」等直接子目录名 = code_root 直接子目录；与
/// [`redirect_instruction_path`] 同锚）。初版曾锚 workspace root +
/// 其直接子目录——真实链路 UAT 抓出该形态在主对话场景（ws_root =
/// workspace 根，与 code_root 分离）永不命中（workspace 模板目录
/// memory/skills/config 与项目层级毫无交集），属实现没跟上设计（决策 3
/// 文案的层级本就是项目层级），2026-11-26 修正。
///
/// 扫描深度：根层 + 直接子目录（depth ≤1；递归扫描成本无界且深层发现
/// 本就靠 TIANGONG 内容指引）。子目录按字典序排序（read_dir 顺序是平台
/// 实现细节）。每层 overlay 裁决（本地 → 镜像）。跨 pair 同显示名合并
/// 为一条（判定时任一路径命中即命中）。命中空 = None（段缺席零字节）；
/// 命中多层合并一条声明（括注存在层级，避免逐层重复）；**位置不代劳**
/// ——LLM 要读自己去 list/read 探索。
///
/// full 模式（决策 6，per-pair）：按 pairs 顺序第一个根层生效 TIANGONG
/// 的 pair——**生效来源 pair** 配置为 full 时额外注入全文（带生效路径，
/// 对齐注入链 per-file 形态）；读盘失败诚实降级为 pointer 声明。
/// 本地来源归属当前 pair（第六轮横扫 2026-11-27 修正：原 `any(pair
/// full)` 让 pointer pair 的本地文件被另一 pair 的 full 配置误全文注入）。
pub fn knowledge_decl_section(cfg: &KnowledgeConfig) -> Option<String> {
    if !cfg.enabled {
        return None;
    }

    // 层集合收集：per-pair 根层 + 直接子目录；同显示名合并（多 pair
    // 挂载同名子目录时声明只出现一次，判定时任一路径命中即命中）。
    let mut layers: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let push_layer = |name: String, path: PathBuf, layers: &mut Vec<(String, Vec<PathBuf>)>| {
        if let Some(entry) = layers.iter_mut().find(|(n, _)| n == &name) {
            entry.1.push(path);
        } else {
            layers.push((name, vec![path]));
        }
    };
    for pair in &cfg.pairs {
        let code = Path::new(&pair.code_root);
        push_layer(ROOT_LAYER_KEY.to_string(), code.to_path_buf(), &mut layers);
        if let Ok(rd) = std::fs::read_dir(code) {
            let mut subdirs: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subdirs.sort();
            for p in subdirs {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                push_layer(name, p, &mut layers);
            }
        }
    }

    let mut hit_names: Vec<String> = Vec::new();
    for (name, paths) in &layers {
        if paths.iter().any(|p| local_or_mirror_tiangong(p, cfg)) {
            hit_names.push(name.clone());
        }
    }
    if hit_names.is_empty() {
        return None;
    }

    // full 模式判定：按 pairs 顺序第一个根层生效 TIANGONG 的 pair——
    // 生效来源 pair 配置 full → 全文注入（根层生效文件），失败降级
    // pointer；生效来源非 full → pointer。**mode 判定锚定生效来源
    // pair**（第六轮横扫 2026-11-27：原实现本地臂 `any(pair full)`——
    // 本地文件归属当前 pair（local 臂 = 当前 pair code_root 根层本地
    // 文件；镜像臂因 code_root 互不重叠 validate 保证恒当前 pair），
    // 却因另一 pair 配了 full 被误全文注入——pointer pair 的意图违背
    // + 无谓 token 成本）。镜像臂 Some(i) 恒当前 pair（重叠形态只在
    // 手改配置绕过 validate 时出现，届时「生效镜像来源 pair 的 mode」
    // 语义仍正确）。
    let pointer_decl = render_pointer_decl(&hit_names);
    for (i, pair) in cfg.pairs.iter().enumerate() {
        let Some((path, source)) = effective_tiangong(Path::new(&pair.code_root), cfg) else {
            continue;
        };
        let source_pair = match source {
            Some(j) => j,
            None => i,
        };
        if cfg.pairs[source_pair].tiangong_load != TIANGONG_LOAD_FULL {
            return Some(pointer_decl);
        }
        // full 模式体积护栏（第九轮横扫 2026-11-27）：全文注入此前无上限——
        // 巨大 TIANGONG（维护者失误或 LLM 经 write_file 异常写入）每轮全文
        // 进上下文 → provider 请求超限 4xx → 会话不可用（一次异常写毁掉
        // 会话，与 [`crate::loop`] 的图片超限自动降采样同构的资源护栏缺失）。
        // metadata 预检先行（每 build 热路径：避免每轮读入超大文件才发现
        // 超限）；超限 = WARN + 诚实降级 pointer 声明——LLM 仍按声明自己去
        // 读，功能降级不炸会话（维护者侧修复 = 拆分文件或改回 pointer）。
        if std::fs::metadata(&path)
            .map(|m| m.len() > TIANGONG_FULL_MAX_BYTES as u64)
            .unwrap_or(false)
        {
            tracing::warn!(
                "[Knowledge] TIANGONG.md（{}）超过 full 模式体积上限 {} 字节——降级 pointer 声明（请拆分文件或改用 pointer 模式）",
                path.display(),
                TIANGONG_FULL_MAX_BYTES
            );
            return Some(pointer_decl);
        }
        return match std::fs::read_to_string(&path) {
            Ok(content) => render_full_section_guarded(&path, &content, &pointer_decl),
            Err(e) => {
                tracing::warn!(
                    "[Knowledge] TIANGONG.md full 模式读盘失败（{}），降级存在性声明: {}",
                    path.display(),
                    e
                );
                Some(pointer_decl)
            }
        };
    }
    Some(pointer_decl)
}

/// 层是否（本地或镜像）有 TIANGONG.md。
fn local_or_mirror_tiangong(layer: &Path, cfg: &KnowledgeConfig) -> bool {
    if layer.join(TIANGONG_FILE_NAME).exists() {
        return true;
    }
    cfg.pairs.iter().any(|p| mirror_tiangong_present(layer, p))
}

/// pointer 声明（决策 3 文案；括注**如实列举命中层**——根层命中才列
/// 「根目录」，全部命中都在子层时括注只列子层，单子层命中同样给括注。
/// 第五轮横扫 2026-11-27 修正：原实现 `multi = len > 1` 且无条件以
/// 「根目录」开头——根层未命中的形态括注谎报根目录（LLM 按声明去根层
/// 读 → 诚实 File not found），单子层命中则完全无位置线索）。
fn render_pointer_decl(hit_names: &[String]) -> String {
    let mut s = String::from(
        "# 天工\n本项目存在天工引导文件 TIANGONG.md。\n该文件包含当前项目层级地图、知识地图、行为规则、层级说明及其他，若有需求可读。",
    );
    // 层名映射（第七轮横扫 2026-11-27）：哨兵（空串）→ 显示名「根目录」
    // ——「根目录」字样当且仅当根层真命中；同名子目录的命中是独立层，
    // 不再与根层合并。映射后按显示名去重（子目录恰名「根目录」时两层
    // 同显示名，重复字样无信息量；LLM 根层未读到会按名探索子目录，
    // 声明不代劳位置）。
    let mut display: Vec<&str> = Vec::new();
    for n in hit_names {
        let shown = if n.is_empty() {
            "根目录"
        } else {
            n.as_str()
        };
        if !display.contains(&shown) {
            display.push(shown);
        }
    }
    // 根层唯一命中 = 最常见形态，声明本身已足够，零附加字节（判定看
    // 原始 hit_names 的哨兵——display 去重后无法区分「根层唯一」与
    // 「同名子目录唯一」）；其余形态括注照实列举。
    let root_only = hit_names.len() == 1 && hit_names[0].is_empty();
    if !root_only {
        s.push_str(&format!("（存在层级：{}）", display.join("、")));
    }
    s
}

/// full 模式全文 section（带生效路径；`</system-reminder>` 逃逸同注入链）。
fn render_full_section(path: &Path, content: &str) -> String {
    format!(
        "# 天工（TIANGONG.md 全文注入，full 模式）\n生效文件：{}\n\n{}",
        path.display(),
        nemesis_utils::sanitize::escape_closing_tag(content, "system-reminder")
    )
}

/// full 模式读后组装（第十轮横扫 2026-11-27）：体积回检 + 渲染。metadata
/// 预检与 read 之间存在替换竞窗（维护者恰好在此间保存超大文件——预检放行
/// 但内容已超限），测试无法确定性复现该窗口，回检谓词抽纯函数直测。超限
/// = WARN + 降级 pointer（一次大读的成本照付，注入不再发生；下一轮
/// metadata 预检直接拦截）。
fn render_full_section_guarded(path: &Path, content: &str, pointer_decl: &str) -> Option<String> {
    if content.len() > TIANGONG_FULL_MAX_BYTES {
        tracing::warn!(
            "[Knowledge] TIANGONG.md（{}）超过 full 模式体积上限 {} 字节（读后回检）——降级 pointer 声明",
            path.display(),
            TIANGONG_FULL_MAX_BYTES
        );
        return Some(pointer_decl.to_string());
    }
    Some(render_full_section(path, content))
}

#[cfg(test)]
mod tests;

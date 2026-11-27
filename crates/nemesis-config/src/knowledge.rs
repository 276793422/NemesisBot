//! 知识库重定向配置（`config.knowledge.json`）——天工系统 v1。
//!
//! 把「项目知识」与「代码仓库」解耦：知识住用户自有的知识库目录（info_root，
//! 按层级镜像项目结构），代码仓库零污染。每条配对描述一组
//! `code_root ↔ info_root` 镜像挂载 + TIANGONG.md 加载模式。
//!
//! - 程序不理解「知识库」——用户配置声明（本文件类型化字段）→ 程序按
//!   固定协议机械执行（存在性声明 + 路径重定向）→ LLM 从注入声明与
//!   TIANGONG.md 内容获得全部语义。
//! - 开关 fail-closed：`enabled` 默认 false；配置文件缺席 / 坏 JSON /
//!   形状非法 / 键缺省一律视为关闭（= 功能不存在）。
//! - 单一权威 = 本文件；声明注入（nemesis-agent knowledge 模块）与
//!   Dashboard 知识库设置页都经由这里读写。
//!
//! 落点（nemesis-path 统一定义，调用方取路径）：
//! - main：`<workspace>/config/config.knowledge.json`（main home = workspace 根）
//! - selfbuilt agent：`agents/<id>/home/config/config.knowledge.json`

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// TIANGONG.md 加载模式：pointer = 只注入存在性声明（默认）。
pub const TIANGONG_LOAD_POINTER: &str = "pointer";
/// TIANGONG.md 加载模式：full = 根层 TIANGONG.md 全文每轮注入（带生效路径）。
pub const TIANGONG_LOAD_FULL: &str = "full";

/// 一条知识库挂载配对（`code_root ↔ info_root` + TIANGONG 加载模式）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgePair {
    /// 代码目录根（绝对路径；重定向命中子树 = 本地缺席时旁路）。
    pub code_root: String,
    /// 知识库目录根（绝对路径；按层级镜像 code_root 结构）。
    pub info_root: String,
    /// TIANGONG.md 加载模式：`pointer`（默认）/ `full`。
    #[serde(default = "default_tiangong_load")]
    pub tiangong_load: String,
}

fn default_tiangong_load() -> String {
    TIANGONG_LOAD_POINTER.to_string()
}

/// config.knowledge.json 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeConfig {
    /// 总开关。**默认 false**；缺省键 = false（fail-closed = 功能不存在）。
    #[serde(default)]
    pub enabled: bool,
    /// 挂载配对列表（正常一对一；允许多对）。
    #[serde(default)]
    pub pairs: Vec<KnowledgePair>,
}

/// 配对数上限（第十轮横扫 2026-11-27 定值 32）：声明扫描每轮
/// 对每 pair read_dir + 每层 stat（成本线性）、围栏放行 / env 透传 / ini
/// 写穿透面同步线性放大——保存面 loud 拒绝超限配置（正常一对一/少数多对
/// 远在线下）。手改文件绕过校验不在读盘面拦截（fail-closed 只对形状非法，
/// 巨大合法配置按备案降级声明扫描成本，不静默禁用）。
pub const MAX_KNOWLEDGE_PAIRS: usize = 32;

/// 校验（保存面前置，读盘面不跑——读盘 fail-closed 降级，错误配置不炸 loop）。
///
/// 规则：两根必须绝对路径；`tiangong_load` ∈ {pointer, full}（未知 loud 拒绝，
/// 不静默纠偏——flag 是当次意志）；code_root 相互重叠 loud 拒绝（同代码目录
/// 两套镜像旁路裁决歧义）；配对数超 [`MAX_KNOWLEDGE_PAIRS`] loud 拒绝；
/// 两根含控制字符 loud 拒绝（第十轮横扫 2026-11-27——ini 写穿透行是行格式，
/// 根含 LF/CR 即 Sandboxie 指令注入槽，见 nemesis-sandbox ini
/// `write_through_directives` 的 sink 侧过滤；此处是 Dashboard/WSAPI 保存
/// 面的源头 loud 拒绝，手改文件由 sink 兜底）。
pub fn validate_knowledge_config(cfg: &KnowledgeConfig) -> Result<(), String> {
    if cfg.pairs.len() > MAX_KNOWLEDGE_PAIRS {
        return Err(format!(
            "配对数量超上限（{} > {MAX_KNOWLEDGE_PAIRS}）：知识库配置面向人工规模设计，请合并或减少配对",
            cfg.pairs.len()
        ));
    }
    for (i, p) in cfg.pairs.iter().enumerate() {
        let label = format!("配对 {}", i + 1);
        if p.code_root.trim().is_empty() || p.info_root.trim().is_empty() {
            return Err(format!("{label}：项目目录与知识库目录均不能为空"));
        }
        for (field, v) in [("项目目录", &p.code_root), ("知识库目录", &p.info_root)] {
            if v.chars().any(char::is_control) {
                return Err(format!(
                    "{label}：{field}含控制字符（路径不可含换行/制表符等控制字符）"
                ));
            }
        }
        let code = std::path::Path::new(&p.code_root);
        let info = std::path::Path::new(&p.info_root);
        if !code.is_absolute() {
            return Err(format!(
                "{label}：项目目录必须是绝对路径（{}）",
                p.code_root
            ));
        }
        if !info.is_absolute() {
            return Err(format!(
                "{label}：知识库目录必须是绝对路径（{}）",
                p.info_root
            ));
        }
        if p.tiangong_load != TIANGONG_LOAD_POINTER && p.tiangong_load != TIANGONG_LOAD_FULL {
            return Err(format!(
                "{label}：加载模式必须是 pointer 或 full（当前 {}）",
                p.tiangong_load
            ));
        }
        // code_root 重叠拒绝（路径前缀语义；不分隔符半截匹配——按组件比较
        // 由调用方 canonical 化后逐段前缀，此处保守做字符串 component 前缀）。
        for (j, q) in cfg.pairs.iter().enumerate() {
            if j <= i {
                continue;
            }
            if path_components_prefix_of(Path::new(&p.code_root), Path::new(&q.code_root))
                || path_components_prefix_of(Path::new(&q.code_root), Path::new(&p.code_root))
            {
                return Err(format!(
                    "{label} 与配对 {} 的项目目录重叠（{} ↔ {}）：同一代码目录不能挂两套知识库",
                    j + 1,
                    p.code_root,
                    q.code_root
                ));
            }
        }
    }
    Ok(())
}

/// 组件级前缀 strip：`full` 的前缀部分与 `prefix` 逐组件相等（Windows
/// 大小写不敏感，其余平台敏感）时返回剩余后缀。`Some(_)` 即「full 在
/// prefix 子树内」。比 `Path::strip_prefix` 严——后者大小写敏感，会在
/// Windows 大小写差异路径（配置写 `C:/Code`、LLM 发 `C:/code/...`）上
/// 漏改写；子树判定与后缀提取必须同一判定语义。
pub fn path_strip_prefix_ci(prefix: &Path, full: &Path) -> Option<PathBuf> {
    if prefix.as_os_str().is_empty() {
        return None;
    }
    let mut fc = full.components();
    for pcomp in prefix.components() {
        let fcomp = fc.next()?;
        let eq = if cfg!(windows) {
            pcomp
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&fcomp.as_os_str().to_string_lossy())
        } else {
            pcomp.as_os_str() == fcomp.as_os_str()
        };
        if !eq {
            return None;
        }
    }
    let rest: PathBuf = fc.collect();
    Some(rest)
}

/// 组件级前缀判定（`/a/b` 是 `/a/b/c` 的前缀；`/a/bc` 不是 `/a/b` 的）。
/// 空根不成前缀。Windows 大小写不敏感（盘符/目录名惯例），其余平台敏感。
/// pub：nemesis-agent 的路径重定向（子树命中判定）与配置校验同一实现
/// ——单一真相源，两处语义必须一致。
pub fn path_components_prefix_of(prefix: &Path, full: &Path) -> bool {
    path_strip_prefix_ci(prefix, full).is_some()
}

/// `<workspace>/config/config.knowledge.json` 读取（fail-closed）。
///
/// 契约：必须容忍缺失文件、错误不外泄——坏 JSON / 形状非法（`[]` 等非
/// object）/ 字段类型错 / 读盘 IO 错误一律 `Default`（enabled=false =
/// 功能不存在）。日志分臂：解析失败与**非 NotFound** 读盘错误 WARN
/// （文件在手边却读不了是异常态，静默关闭零诊断）；NotFound 安静（句柄
/// 恒装形态下配置缺席是常态，零噪声）。形状闸前置（`ensure_object`，同
/// commands.rs 的 BUG-20261125-008 教训：全字段 default 的 struct 无形状
/// 闸会把违形文档静默解析成空配置零诊断）。
pub fn load_knowledge_config(path: &Path) -> KnowledgeConfig {
    match nemesis_utils::fsutil::read_config_string(path) {
        Ok(raw) => {
            let parsed = serde_json::from_str::<serde_json::Value>(&raw)
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    nemesis_utils::json_shape::ensure_object(&v, "config.knowledge.json")?;
                    serde_json::from_value::<KnowledgeConfig>(v).map_err(|e| e.to_string())
                });
            match parsed {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        "[Knowledge] config 解析失败/形状非法（{}），按关闭处理: {}",
                        path.display(),
                        e
                    );
                    KnowledgeConfig::default()
                }
            }
        }
        // 读盘失败分臂（第六轮横扫 2026-11-27）：NotFound 安静（句柄恒装
        // 形态下配置缺席是常态——装配点无分支「行为与不装逐字节一致」
        // 的承诺含日志流；mtime 缓存让缺席态只判一次）；其余 IO 错误
        // （权限拒绝 / 被锁 / 盘故障）WARN——文件在手边却读不了是异常态，
        // 静默关闭会让「配置了却永不生效」零诊断。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => KnowledgeConfig::default(),
        Err(e) => {
            tracing::warn!(
                "[Knowledge] config 读盘失败（{}），按关闭处理: {}",
                path.display(),
                e
            );
            KnowledgeConfig::default()
        }
    }
}

/// 保存（校验前置 + 统一原子 helper——tmp+sync+rename，失败清理）。
pub fn save_knowledge_config(path: &Path, cfg: &KnowledgeConfig) -> Result<(), String> {
    validate_knowledge_config(cfg)?;
    let body = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), body.as_bytes(), 0o600)
}

#[cfg(test)]
mod tests;

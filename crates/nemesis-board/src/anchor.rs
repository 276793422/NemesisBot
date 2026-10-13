//! 全自动流转 P2（B1）：确定性验收锚点——零 LLM 成本、零命令执行的客观
//! 检查层（单一真相源：本模块与 planner prompt 的 `[CHECK]` 指令段同步
//! 演化）。
//!
//! 验收标准（`acceptance_criteria` 自由文本）中以 `[CHECK]` 开头的行是
//! 客观锚点，其余行照旧走 LLM 语义项（`parse_anchors` 返回二元组）：
//!
//! ```text
//! [CHECK] file:src/lib.rs exists            — 文件存在
//! [CHECK] re:交付完成                        — 对 worker delivery 文本正则匹配
//! [CHECK] file:docs/api.md contains:鉴权     — 文件内容包含关键词
//! [CHECK] file:src/main.rs re:^fn main      — 文件内容正则匹配
//! ```
//!
//! **行内注释（P4 编写陷阱防线，2026-10-13）**：行内 ` # `（空格#空格）
//! 之后是编写者注释，不参与匹配——`[CHECK] re:^测试通过 # 汇报须含测试结论`
//! 只用前半段。引号感知：引号**成对**时引号内的 ` # ` 是字面内容不切
//! （`exec:git log --format="%h # %s"` 不被误截）；引号不成对退简单切分。
//! re: 模式无分隔却含空白/非 ASCII（疑似把描述写进匹配式）时
//! **WARN 不拒装**（兼容存量），且该锚点 FAIL 时明细尾部带编写面诊断
//! （「应用 ` # ` 分隔注释」）——歧义输入宁 WARN 不假 FAIL，重派前先给
//! 编写者纠错线索。
//!
//! **exec: 锚点（M7，config 闸门默认关）**：`[CHECK] exec:<命令>` 在锚点
//! 根目录（项目目录在场优先，否则 workspace 根）以子进程执行命令，退出码
//! 0 = 过、非 0 = 不过。服务「表现层零可执行验证手段」（21 个 JS 文件全
//! 靠人眼的教训）。护栏三件：[`ExecAnchorPolicy`]（默认
//! [`ExecAnchorPolicy::Disabled`]，须 `board.review.exec_anchors_enabled`
//! 显式开启）+ 单条执行超时（默认 120s，超时 = 不过）+ 输出截断（防大输
//! 出撑爆评论/planner prompt）。关闭时 exec 锚点**诚实跳过并注记**
//! （[`run_anchors_dual_ext`] 的第二返回值），不做 PASS 也不做 FAIL——
//! 跳过项进提示/评论，不进失败集（否则会触发重派，重蹈假红病灶）。planner
//! 对可运行时验证的子单**应**产出此形态（§6.1 改动二翻转；按交付物技术栈
//! 选命令）。两道闸护航：解析期危险命令黑名单（[`validate_exec_anchor_command`]
//! ：删除/网络外联/任意代码执行形态直接拒绝）+ 环境依赖探针（
//! [`run_exec_anchor`]：命令解释器/关键工具缺失 = 诚实 SKIP 不计失败不进
//! 失败签名集——master 环境缺失不得冤枉重派，NB-8 同型冤案防线）。
//!
//! 其余文件类锚点只读 `std::fs`，路径经 [`resolve_anchor_path`] 单点收紧：
//! 仅限根子树内相对路径，拒绝绝对路径 / `..` 穿越 / 8.3 短名 /
//! 符号链接越界（canonicalize 后前缀校验）。无法解析的 `[CHECK]` 行不
//! 拒绝——回落语义项，诚实降级不炸验收。
//!
//! **双根核验（[`run_anchors_dual`]，2026-10-09 web-FPS NB-22 锚点全败根修）**：
//! 档案管线把 worker 变更集合并在**项目目录**（`board-projects/<项目名>/`，
//! `git_repo::merge_changeset` 的唯一落点，设计上不进主 workspace），而非
//! 档案管线的首跑件直写主 workspace 根——两种交付形态并存时，单根核验
//! 必然误杀其一。文件类锚点因此支持**主根 + 项目目录 fallback 的双根 OR**：
//! 任一根满足谓词即通过；`re:` 交付文本锚点与 FS 无关，不受 fallback 影响。

use regex::Regex;
use std::path::{Path, PathBuf};

/// 锚点检查上限（超过按失败处理，诚实降级；防锚点检查本身变成内存攻击面）。
const MAX_ANCHOR_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// exec: 锚点单条执行超时缺省值（秒；超时 = 不过 + kill）。config 显式
/// 配置 `board.review.exec_anchor_timeout_secs` 时可覆盖。
pub const EXEC_ANCHOR_DEFAULT_TIMEOUT_SECS: u64 = 120;

/// exec: 锚点输出保留上限（字符，取尾部）——防大输出撑爆评论与 prompt。
const EXEC_OUTPUT_TAIL_CHARS: usize = 1024;

/// 锚点行前缀（trim 后判定；planner 指令段使用同一标记）。
pub const ANCHOR_PREFIX: &str = "[CHECK]";

/// re: 模式疑似描述混入的失败面诊断（编写面提示，只挂在 FAIL 明细尾部——
/// PASS 不噪音）。
const PROSE_PATTERN_HINT: &str =
    "\n提示：pattern 含空白/非 ASCII，疑似描述混入，应用 ` # ` 分隔注释";

/// 按首个**引号外**的 ` # ` 切掉行尾显式注释，返回（前段, 是否有显式注释）
/// ——单一真相源，re:/exec:/file: 各臂共用。
///
/// 引号感知：单/双引号**成对**（全行偶数）时，引号内的 ` # ` 是字面内容
/// 不是注释——`exec:git log --format="%h # %s"`、`re:"\d+ # \d+"` 不被误切
/// （复检批发现 quote-blind 切分会把这类命令截断在引号中间，编写者意图
/// 与语义漂移的同族问题）。转义形态 `\ \#\ ` 的 `#` 前是反斜杠非空格，
/// 天然不命中。字节扫描安全：`'`/`"`/`#`/` ` 均为 ASCII，UTF-8 多字节
/// 序列（≥0x80）不会误配，`rest[..i]` 落在字符边界上。
///
/// 引号不成对 → 退 quote-blind 盲切（多角度审计批 2026-10-13 改向）：
/// 视引号为普通字符，注释切在**最后一个** ` # `——编写约定注释在行尾，
/// 残引号在 pattern 中间时早切会把 pattern 主体截断在引号残迹前
///（`re:"\d+ # \d+ # 备注` 少个闭引号，早切只剩 `\d+`）；取末个保住主体。
/// 英文叙述撇号（`it's done # 备注`）单 ` # ` 场景首末等价，不受影响。
/// 切分落 **debug**（闭环批 2026-10-13 降档）：本函数在 sweep/验收热路径
/// 被反复调用（has_file_anchor 等 tick 级扫查），残缺引号的存量单子会
/// 每 tick 刷一条 WARN（30s tick ≈ 2880 条/天）——编写面诊断的正确落点
/// 是安装期 WARN（`warn_if_prose_pattern` 的 prose 检查）与 FAIL 面明细，
/// 此处降 debug 保现场可查不刷屏。
fn split_anchor_comment(rest: &str) -> (&str, bool) {
    let quote_aware =
        rest.matches('\'').count().is_multiple_of(2) && rest.matches('"').count().is_multiple_of(2);
    if !quote_aware {
        tracing::debug!(
            "[Anchor] 锚点引号不成对（残缺引号），退 quote-blind 注释切分（最后一个 ` # ` 为注释起点）：{rest}"
        );
        if let Some((idx, _)) = rest.rmatch_indices(" # ").next() {
            // idx 指向 ` # ` 的前导空格；前段含该空格（与 quote-aware 臂
            // 的 `&rest[..i]` 同形态，调用方各自 trim）。
            return (&rest[..idx + 1], true);
        }
        return (rest, false);
    }
    let bytes = rest.as_bytes();
    let (mut in_single, mut in_double) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if i > 0
                && bytes[i - 1] == b' '
                && bytes.get(i + 1) == Some(&b' ')
                && !in_single
                && !in_double =>
            {
                return (&rest[..i], true);
            }
            _ => {}
        }
    }
    (rest, false)
}

/// 剥模式两端的成对包裹引号（多角度审计批 2026-10-13）：编写者借引号包住
/// 含空白的模式（shell 习惯），但 re:/contains: 的模式是裸文本——引号字面
/// 进了正则/关键词几乎永远匹配不上（`re:"交付完成"` 字面找 `"交付完成"`，
/// 而交付文本极少逐字含引号 → 大概率 FAIL → 无谓重派）。只剥**成对**包裹
/// （首尾同引号字符且长度 ≥2）；不成对（残引号）不动——那是
/// split_anchor_comment 盲切分支的病态输入域，此处不猜。
/// 方向注记（诚实面）：剥引号是**超集匹配**（不带引号的匹配 ⊇ 带引号字面
/// 的匹配），只会把 FAIL 翻成 PASS、绝不把 PASS 翻成 FAIL——不构成验收
/// 放水面（planner/评审对交付内容的核验不靠锚点单点，exec:/file: 锚点
/// 是客观实核）。
fn strip_wrap_quotes(pattern: &str) -> &str {
    let b = pattern.as_bytes();
    let wrapped = b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'"' || b[0] == b'\'');
    if wrapped {
        &pattern[1..pattern.len() - 1]
    } else {
        pattern
    }
}

/// re: 模式「疑似描述混入」判定——两档谓词（多角度审计批 2026-10-13 拆分）：
/// - **安装 WARN 用窄档**（[`pattern_looks_like_prose_strict`]）：含空白才
///   疑似——多词自然语言描述的强信号。纯 CJK 无空白（`re:交付完成`，planner
///   提示词自己推荐的形态）是合法匹配式，WARN 是误报噪音。
/// - **FAIL 提示用宽档**（本函数）：含空白**或**非 ASCII——FAIL 时的提示
///   是零成本诊断（PASS 不噪音），CJK 模式失配也值得提醒「可能把描述写进
///   了匹配式」。
fn pattern_looks_like_prose(pattern: &str) -> bool {
    pattern.chars().any(|c| c.is_whitespace() || !c.is_ascii())
}

/// 安装 WARN 谓词（窄档）：仅空白。见 [`pattern_looks_like_prose`] 分档说明。
fn pattern_looks_like_prose_strict(pattern: &str) -> bool {
    pattern.chars().any(|c| c.is_whitespace())
}

/// 解析期编写面 WARN（不拒装）：无显式 ` # ` 分隔 yet 模式含空白 → 大概率
/// 把描述写进了匹配式（核验几乎必然失败 → 无谓重派）。照常安装（存量验收
/// 标准零迁移），只落 WARN 供编写者自查。纯 CJK 无空白不 WARN（合法匹配式）。
fn warn_if_prose_pattern(pattern: &str, explicit_comment: bool, raw: &str) {
    if !explicit_comment && pattern_looks_like_prose_strict(pattern) {
        tracing::warn!(
            "[Anchor] re: 模式疑似混入描述文本（含空白且未用 ` # ` 分隔注释），已照常安装；若本意为「pattern + 注释」请改写为 `<pattern> # <注释>`：{raw}"
        );
    }
}

/// 失败面编写诊断：prose 形模式未命中时在 FAIL 明细尾部追加提示（PASS 不加）。
fn prose_pattern_hint(pattern: &str) -> &str {
    if pattern_looks_like_prose(pattern) {
        PROSE_PATTERN_HINT
    } else {
        ""
    }
}

/// exec: 锚点的执行策略（M7 护栏闸）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecAnchorPolicy {
    /// 不执行（默认）：exec 锚点诚实跳过并注记，不进失败集。
    Disabled,
    /// 执行：`timeout_secs` 为单条超时（超时 = 不过）。
    Enabled { timeout_secs: u64 },
}

/// 锚点种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorKind {
    /// `file:<路径> exists` — 文件存在。
    FileExists,
    /// `file:<路径> re:<正则>` — 文件内容正则匹配。
    FileRegex,
    /// `file:<路径> contains:<关键词>` — 文件内容包含关键词。
    FileContains,
    /// `re:<正则>` — 对 worker delivery 文本（验收输入）正则匹配。
    ContentRegex,
    /// `exec:<命令>` — 在锚点根目录执行命令，退出码 0 = 过（M7；
    /// 受 [`ExecAnchorPolicy`] 闸门，默认不执行）。
    Exec,
}

impl AnchorKind {
    /// 派发/评审文案里的短名（`AnchorResult::detail` 拼接用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorKind::FileExists => "文件存在",
            AnchorKind::FileRegex => "文件正则",
            AnchorKind::FileContains => "文件包含",
            AnchorKind::ContentRegex => "交付文本正则",
            AnchorKind::Exec => "命令执行",
        }
    }

    /// 是否 file: 类锚点（存在/正则/包含——锚点根解析产物路径的三种形态）。
    /// 拓扑分类单一真相源：file: 锚点在派发端 workspace 实核 → 只能派本节点；
    /// `re:`（对交付文本核对）与 `exec:`（worker 自证 + master 重放，环境
    /// 缺失诚实 SKIP）跨节点安全，不计入拓扑约束。
    pub fn is_file_anchor(&self) -> bool {
        matches!(
            self,
            AnchorKind::FileExists | AnchorKind::FileRegex | AnchorKind::FileContains
        )
    }
}

/// 一条解析后的锚点（`raw` = 原始行，明细评论里回显）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorCheck {
    pub raw: String,
    pub kind: AnchorKind,
    /// 文件类 = workspace 相对路径；ContentRegex = 空串。
    pub target: String,
    /// 正则模式 / 关键词；FileExists = None。
    pub pattern: Option<String>,
}

/// 单条锚点执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorResult {
    pub anchor: AnchorCheck,
    pub passed: bool,
    pub detail: String,
}

/// 被拒绝的锚点行（路径形态不安全：绝对路径 / `..` 穿越 / 盘符 / UNC /
/// 8.3 短名）。拒绝 = 不产出锚点、行回落语义项 + 调用方落告警评论——
/// 恶意/不安全锚点是验收标准自身的毛病，重派惩罚不到执行者，也不得
/// 让验收炸掉或变成对工作区外文件系统的探测激励。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedAnchor {
    pub raw: String,
    pub reason: String,
}

/// AC 是否含 `re:`（交付文本正则）锚点（2026-09-30 F6）：这类锚点核对的
/// 是 worker delivery 文本本体，可被「把标记字符串抄进交付文本」欺骗
/// （真机 NB-13 实证）；file: 类锚点核的是真实文件，不在本判定内。调用方
/// （评审）据此按需启用自检取证。
pub fn has_content_regex_anchor(acceptance_criteria: &str) -> bool {
    let (anchors, _, _) = parse_anchors(acceptance_criteria);
    anchors.iter().any(|a| a.kind == AnchorKind::ContentRegex)
}

/// AC 是否含 file: 类锚点（`file:` 存在/正则/包含三种形态）。**拓扑约束
/// 谓词（单一真相源）**：file: 锚点在派发端（看板权威端）workspace 解析
/// 实核，而远端节点的交付文件不在派发端 workspace——含 file: 锚点的单只能
/// 派给本节点执行。`re:`（交付文本）与 `exec:`（worker 自证 + master 重放，
/// 见 §6.1）锚点跨节点安全，**不**计入本谓词。消费方（全部必须走本谓词，
/// 不许各自内联解析）：`reject_remote_file_anchors` 拓扑硬闸 /
/// `rank_dispatch_candidates` 候选过滤（换节点/兜底/匹配器决策面）/
/// `rebalance_queued_to_worker` 重平衡偷单过滤。真机 NB-4/NB-5/NB-6
/// 实证（2026-10-08）：决策面不感知本谓词 → 产出非法远端目标 → 拓扑闸
/// 拦下 → 死锁或取消后无补偿。E2E-1 验收实证（2026-10-12）：exec 锚点
/// 误计入本谓词 → 纯 exec/re 锚点单全体停车。
pub fn has_file_anchor(acceptance_criteria: &str) -> bool {
    let (anchors, _, _) = parse_anchors(acceptance_criteria);
    anchors.iter().any(|a| a.kind.is_file_anchor())
}

/// 解析验收标准：`[CHECK]` 行 → 锚点，其余行（含无法解析的 `[CHECK]` 行）
/// → 语义项回落。非法正则同样回落语义项（诚实降级，不拒绝整段验收标准）。
/// 路径形态不安全的锚点行进入第三元组 `Vec<RejectedAnchor>`（同样回落语
///义项，供调用方落系统告警评论）。
pub fn parse_anchors(
    acceptance_criteria: &str,
) -> (Vec<AnchorCheck>, Vec<String>, Vec<RejectedAnchor>) {
    let mut anchors = Vec::new();
    let mut semantic = Vec::new();
    let mut rejected = Vec::new();
    for line in acceptance_criteria.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(ANCHOR_PREFIX) {
            match parse_anchor_line(rest.trim()) {
                Ok(Some(anchor)) => anchors.push(anchor),
                Ok(None) => semantic.push(line.to_string()),
                Err(reason) => {
                    semantic.push(line.to_string());
                    rejected.push(RejectedAnchor {
                        raw: trimmed.to_string(),
                        reason,
                    });
                }
            }
        } else if !trimmed.is_empty() {
            semantic.push(line.to_string());
        }
    }
    (anchors, semantic, rejected)
}

/// 解析 `[CHECK]` 之后的剩余文本。
/// - `Ok(Some(anchor))` = 合法锚点；
/// - `Ok(None)` = 无法解析（回落语义项）；
/// - `Err(reason)` = 路径形态不安全（回落语义项 + 告警）。
fn parse_anchor_line(rest: &str) -> Result<Option<AnchorCheck>, String> {
    let raw = format!("{ANCHOR_PREFIX} {rest}");
    if rest.is_empty() {
        return Ok(None);
    }
    // 显式注释切分（行级单点，P4）：引号外的 ` # ` 之后不参与任何臂的
    // 语义——file: 路径/谓词、re: 模式、exec: 命令（含黑名单校验）都只看
    // 前段。行尾注释因此不会把关键词/命令弄脏（`exec:npm test # 不用 curl`
    // 不再因注释里的黑名单词被误拒）；引号内的 ` # ` 是字面内容不切
    // （split_anchor_comment 文档）。raw 保留全行（回显/自指防御语义不变）。
    let (rest, explicit_comment) = split_anchor_comment(rest);
    if let Some(target) = rest.strip_prefix("file:") {
        // 目标路径到首个空白为止；谓词可含空格（正则/关键词）。
        let (target, predicate) = match target.find(char::is_whitespace) {
            Some(idx) => (&target[..idx], target[idx..].trim()),
            None => (target, ""),
        };
        if target.is_empty() {
            return Ok(None);
        }
        // 路径安全形态闸（解析期，FS 无关）：绝对路径 / 盘符 / UNC /
        // `..` 穿越 / 8.3 短名直接拒绝——不安全锚点不得成为重派理由，
        // 更不能诱导执行者向工作区外写文件来「满足锚点」。
        validate_anchor_path_shape(target)?;
        let (kind, pattern) = if predicate == "exists" {
            (AnchorKind::FileExists, None)
        } else if let Some(pattern) = predicate.strip_prefix("re:") {
            let pattern = strip_wrap_quotes(pattern.trim());
            if pattern.is_empty() || Regex::new(pattern).is_err() {
                return Ok(None); // 空模式 / 非法正则 → 语义项回落
            }
            warn_if_prose_pattern(pattern, explicit_comment, &raw);
            (AnchorKind::FileRegex, Some(pattern.to_string()))
        } else if let Some(keyword) = predicate.strip_prefix("contains:") {
            let keyword = strip_wrap_quotes(keyword.trim());
            if keyword.is_empty() {
                return Ok(None);
            }
            (AnchorKind::FileContains, Some(keyword.to_string()))
        } else {
            return Ok(None); // 未知谓词 → 语义项回落
        };
        return Ok(Some(AnchorCheck {
            raw,
            kind,
            target: target.to_string(),
            pattern,
        }));
    }
    if let Some(pattern) = rest.strip_prefix("re:") {
        let pattern = strip_wrap_quotes(pattern.trim());
        if pattern.is_empty() || Regex::new(pattern).is_err() {
            return Ok(None);
        }
        warn_if_prose_pattern(pattern, explicit_comment, &raw);
        return Ok(Some(AnchorCheck {
            raw,
            kind: AnchorKind::ContentRegex,
            target: String::new(),
            pattern: Some(pattern.to_string()),
        }));
    }
    if let Some(command) = rest.strip_prefix("exec:") {
        let command = command.trim();
        if command.is_empty() {
            return Ok(None);
        }
        // 危险命令黑名单（§6.1 改动二配套）：验证锚点只允许只读/测试类
        // 命令；删除/网络外联/任意代码执行形态解析期即拒绝（与路径形态
        // 不安全同通道：回落语义项 + RejectedAnchor 告警）。
        validate_exec_anchor_command(command)?;
        return Ok(Some(AnchorCheck {
            raw,
            kind: AnchorKind::Exec,
            target: command.to_string(),
            pattern: None,
        }));
    }
    Ok(None)
}

/// exec: 锚点危险命令黑名单（token 级精确匹配——`npm` 含子串 `rm` 不误伤，
/// 词法边界由分词保证）。黑名单取「验证命令永远不需要」的破坏性形态：
/// 删除 / 移动 / 磁盘格式化 / 网络外联 / 远程任意代码 / 进程与系统控制。
/// 拒绝 = 锚点不生效（回落语义评审）+ 告警评论，验收不炸、执行者不被惩罚。
const EXEC_ANCHOR_BLOCKED_TOKENS: &[&str] = &[
    // 删除/破坏类
    "rm",
    "rmdir",
    "del",
    "erase",
    "rd",
    "remove-item",
    "ri",
    "unlink",
    "shred",
    "format",
    "mkfs",
    "dd",
    "diskpart",
    "clear-content",
    "truncate",
    // 移动/越目录写类
    "mv",
    "move",
    "robocopy",
    "xcopy",
    // 网络外联类
    "curl",
    "wget",
    "nc",
    "ncat",
    "netcat",
    "ssh",
    "scp",
    "sftp",
    "ftp",
    "telnet",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
    "certutil",
    "bitsadmin",
    // 任意代码执行/注入类
    "iex",
    "invoke-expression",
    // 进程/系统控制类
    "taskkill",
    "kill",
    "pkill",
    "stop-process",
    "stop-service",
    "shutdown",
    "reboot",
    "reg",
    "regedit",
    "bcdedit",
    "net",
    "sc",
];

/// exec: 锚点命令分词：按空白与 shell 结构符切开、去引号、小写化——token
/// 级精确匹配黑名单（子串不误伤：`npm`/`del.js`/`format.js` 均放行）。
fn exec_command_tokens(command: &str) -> Vec<String> {
    command
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '|' | ';' | '&' | '(' | ')' | '<' | '>' | '`')
        })
        .map(|t| {
            t.trim_matches(|c| c == '"' || c == '\'')
                .to_ascii_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// 危险命令形态闸（解析期，FS/平台无关）：命中黑名单 = `Err(reason)`
/// （调用方按 RejectedAnchor 通道处置）。环境变量前缀（`FOO=bar`）与
/// 路径形态首 token（`./scripts/run.sh`）不参与黑名单匹配。
pub fn validate_exec_anchor_command(command: &str) -> Result<(), String> {
    for tok in exec_command_tokens(command) {
        if tok.contains('=') || tok.contains('/') || tok.contains('\\') {
            continue; // 环境变量赋值 / 带路径的脚本调用不按裸词匹配
        }
        if EXEC_ANCHOR_BLOCKED_TOKENS.contains(&tok.as_str()) {
            return Err(format!(
                "exec 锚点拒绝危险命令形态（命中黑名单词「{tok}」）：验证锚点只允许只读/测试类命令"
            ));
        }
    }
    Ok(())
}

/// M2 决策链三态分流：锚点执行前先验根有效性。主根与 fallback 根**都不
/// 可达**（canonicalize 失败）= 基础设施故障（`Err`），调用方必须转人工 +
/// WARN，**不得**照常执行锚点（那会产出逐锚点 FAIL → 触发重派——机器上
/// 没人能修好「根不存在」，重派循环烧预算还惩罚不到任何人）；至少一根可
/// 达、或锚点组全部与 FS 无关（`re:` 交付文本锚点 / 空组）= `Ok(())`，
/// 照常执行。根可达但文件未命中 = 真 FAIL（质量面），不在此判定内。
///
/// 注意与拓扑谓词 [`has_file_anchor`] 的分工：本函数判定「**评审机器**上
/// 锚点执行是否需要可达根」——exec: 锚点执行 cwd = 锚点根，需要根可达，
/// 故计入（`!ContentRegex` 口径在此正确）；拓扑谓词判定「**派发**能否去
/// 远端」——exec 走 worker 自证 + master 重放，跨节点安全，不计入。两套
/// 口径不同是刻意的，勿互相「统一」（E2E-1 实证 2026-10-12：拓扑口径
/// 误纳 exec → 派发全体停车）。
pub fn validate_anchor_roots(
    anchors: &[AnchorCheck],
    workspace_root: &Path,
    project_root: Option<&Path>,
) -> Result<(), String> {
    let fs_needed = anchors
        .iter()
        .any(|a| !matches!(a.kind, AnchorKind::ContentRegex));
    if !fs_needed {
        return Ok(());
    }
    match workspace_root.canonicalize() {
        Ok(_) => Ok(()),
        Err(ws_err) => match project_root {
            Some(dir) => match dir.canonicalize() {
                Ok(_) => Ok(()),
                Err(proj_err) => Err(format!(
                    "锚点主根与项目目录均不可达：workspace（{ws_err}）；project（{proj_err}）"
                )),
            },
            None => Err(format!(
                "锚点主根不可达且无项目目录 fallback：workspace（{ws_err}）"
            )),
        },
    }
}

/// 执行锚点组：文件类只读 `std::fs`，正则走 regex crate（线性时间，无
/// ReDoS 面）。全部判定保守——任何执行层意外（路径不安全 / 读取失败 /
/// 超限）都算未通过并带诚实明细，不静默放行。
pub fn run_anchors(
    anchors: &[AnchorCheck],
    workspace_root: &Path,
    delivery_text: &str,
) -> Vec<AnchorResult> {
    run_anchors_dual(anchors, workspace_root, None, delivery_text)
}

/// 双根版 [`run_anchors`]：`project_root` 在场（issue 挂了看板项目且项目
/// 目录已分配）时，文件类锚点在主根未命中后再核项目目录——档案管线合并
/// 产物落在项目目录、非档案管线首跑件落在主 workspace 根，双根 OR 兼容
/// 两种交付形态（NB-22 锚点全败事故根修；见模块文档）。`project_root`
/// 传 `None` 时与 [`run_anchors`] 行为一致。exec: 锚点按
/// [`ExecAnchorPolicy::Disabled`] 处理（跳过注记被丢弃）——需要消费跳过
/// 注记的调用方走 [`run_anchors_dual_ext`]。
pub fn run_anchors_dual(
    anchors: &[AnchorCheck],
    workspace_root: &Path,
    project_root: Option<&Path>,
    delivery_text: &str,
) -> Vec<AnchorResult> {
    run_anchors_dual_ext(
        anchors,
        workspace_root,
        project_root,
        delivery_text,
        &ExecAnchorPolicy::Disabled,
    )
    .0
}

/// 双根 + exec 策略版 [`run_anchors_dual`]（M7）：返回 `(结果组, 跳过注记)`。
/// 关闭策略下的 exec 锚点**不进结果组**（做 PASS 会假放行、做 FAIL 会触发
/// 重派——跳过是第三态），只进跳过注记，由调用方落提示/评论。
pub fn run_anchors_dual_ext(
    anchors: &[AnchorCheck],
    workspace_root: &Path,
    project_root: Option<&Path>,
    delivery_text: &str,
    exec_policy: &ExecAnchorPolicy,
) -> (Vec<AnchorResult>, Vec<String>) {
    let mut results = Vec::new();
    let mut skipped = Vec::new();
    for anchor in anchors {
        if matches!(anchor.kind, AnchorKind::Exec) {
            let ExecAnchorPolicy::Enabled { timeout_secs } = exec_policy else {
                skipped.push(format!(
                    "⏭ exec 锚点未执行（board.review.exec_anchors_enabled 未开启）：{}",
                    anchor.raw
                ));
                continue;
            };
            // exec 的执行根：项目目录在场优先（档案管线合并产物的主落点），
            // 否则 workspace 根。cwd 语义与文件锚点的项目目录优先 fallback
            // 同构（§6.1 改动四核实结论：项目目录在场优先已是现状，测试钉住）。
            let root = project_root.unwrap_or(workspace_root);
            match run_exec_anchor(anchor, root, *timeout_secs) {
                ExecAnchorOutcome::Ran(passed, detail) => results.push(AnchorResult {
                    anchor: anchor.clone(),
                    passed,
                    detail,
                }),
                ExecAnchorOutcome::EnvSkip(note) => skipped.push(note),
            }
            continue;
        }
        let (passed, detail) = run_one(anchor, workspace_root, project_root, delivery_text);
        results.push(AnchorResult {
            anchor: anchor.clone(),
            passed,
            detail,
        });
    }
    (results, skipped)
}

/// exec: 锚点单条执行结局。
enum ExecAnchorOutcome {
    /// 实跑完成：(通过, 明细)。
    Ran(bool, String),
    /// 环境依赖缺失——诚实跳过（不计失败、不进失败签名集），注记进跳过
    /// 通道（调用方落提示/评论）。
    EnvSkip(String),
}

/// exec: 锚点单条执行（std::process，nemesis-board 无 tokio 依赖）：先跑
/// 环境依赖探针（缺工具 = [`ExecAnchorOutcome::EnvSkip`] 诚实跳过），再实
/// 跑命令——退出码 0 = 过；启动失败 / 超时 / 非 0 退出 = 不过，明细带诚实
/// 原因与输出尾部。
fn run_exec_anchor(anchor: &AnchorCheck, root: &Path, timeout_secs: u64) -> ExecAnchorOutcome {
    let command = anchor.target.trim();
    if command.is_empty() {
        return ExecAnchorOutcome::Ran(false, "exec 锚点命令为空".to_string());
    }
    if !root.is_dir() {
        return ExecAnchorOutcome::Ran(
            false,
            format!("exec 锚点执行根不存在（{}）", root.display()),
        );
    }
    // 环境依赖探针（§6.1 master 侧执行环境保障；仓库纪律「二进制在场 ≠
    // 可用，用前实跑 trivial 探针」）：实跑前先以 where/command -v 实探
    // 首 token 工具是否在场——缺失 = EnvSkip 不计失败（master 缺环境不得
    // 变成 FAIL 冤枉重派，NB-8 同型冤案）。shell 内建（cd/echo/set…解释器
    // 在场即成立）与路径形态首 token（相对脚本缺失属交付缺陷，走真 FAIL）
    // 不探。
    if let Some(tool) = probe_target_token(command)
        && !probe_tool_present(&tool, root)
    {
        return ExecAnchorOutcome::EnvSkip(format!(
            "⏭ exec 锚点环境依赖缺失（{tool} 不在本机 PATH），诚实跳过（不计失败、不进失败签名集；请在评审端预装该工具链）：{}",
            anchor.raw
        ));
    }
    let mut cmd = if cfg!(windows) {
        // raw_arg/creation_flags 同源于 CommandExt（Windows）。
        #[cfg(windows)]
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C");
        // raw_arg：命令串**原样**进命令行（与 cc_hooks.rs 同型修复）。
        // 默认 argv 转义把内嵌引号变 `\"`（MSVCRT 规则），cmd 不认 `\"
        // `——`findstr /C:"..."` 形态的验证命令被打挂（退出码 1 假 FAIL，
        // NB-22 实证：文件在场且内容精确仍三连失败）。raw_arg 让 cmd 按
        // 原生语法解析，与真 shell 收到的一致。
        #[cfg(windows)]
        c.raw_arg(command);
        #[cfg(not(windows))]
        c.arg(command);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        // 后台网关进程不弹控制台窗口（CREATE_NO_WINDOW，仓库纪律）。
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ExecAnchorOutcome::Ran(false, format!("exec 锚点进程启动失败：{e}"));
        }
    };
    // 管道读取放独立线程（避免孙进程继承句柄导致 exit 后 read 阻塞死锁），
    // 主线程 try_wait 轮询到超时上限。
    let out_rx = spawn_pipe_reader(child.stdout.take().map(|p| Box::new(p) as PipeReader));
    let err_rx = spawn_pipe_reader(child.stderr.take().map(|p| Box::new(p) as PipeReader));
    // 上界钳制（D12-6）：config 的 exec_anchor_timeout_secs 是无界 u64，
    // `Instant + Duration` 溢出按 std 契约 panic——clamp 只防荒谬值，
    // 正常配置（≤1 天）逐字节不变。
    let timeout_secs = timeout_secs.clamp(1, 86_400);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                return ExecAnchorOutcome::Ran(false, format!("exec 锚点执行失败：{e}"));
            }
        }
    };
    let Some(status) = status else {
        return ExecAnchorOutcome::Ran(
            false,
            format!("exec 锚点执行超时（>{timeout_secs}s），进程已终止"),
        );
    };
    // 收输出尾部（2s 宽限；孙进程占住管道时诚实放弃，不阻塞评审）。
    let out = out_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap_or_default();
    let err = err_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap_or_default();
    let mut detail = if status.success() {
        "命令退出码 0".to_string()
    } else {
        format!(
            "命令退出码 {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "被信号终止".to_string())
        )
    };
    let tail = truncate_tail(&out, &err);
    if !tail.is_empty() {
        detail.push_str(&format!("\n输出尾部：\n{tail}"));
    }
    ExecAnchorOutcome::Ran(status.success(), detail)
}

/// 环境探针目标 token 提取：首 token 为裸词（非 env 赋值 / 非路径形态）且
/// 不是 shell 内建（内建随解释器在场成立）时返回，供 [`probe_tool_present`]。
fn probe_target_token(command: &str) -> Option<String> {
    let tok = exec_command_tokens(command).into_iter().next()?;
    if tok.contains('=') || tok.contains('/') || tok.contains('\\') {
        return None;
    }
    const SHELL_BUILTINS: &[&str] = &[
        "cd", "echo", "set", "exit", "if", "for", "while", "dir", "type", "call", "rem", "goto",
        "shift", "ver", "cls",
    ];
    if SHELL_BUILTINS.contains(&tok.as_str()) {
        return None;
    }
    Some(tok)
}

/// 工具在场探针（trivial 实跑）：Windows `where <tool>` / Unix
/// `command -v <tool>`，退出码 0 = 在场。10s 探针超时（工具解析毫秒级，
/// 慢盘/杀软扫描兜底）；探针自身起不来 = 视为不在场（保守跳过）。
fn probe_tool_present(tool: &str, root: &Path) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(format!("where {tool}"));
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c")
            .arg(format!("command -v {tool} >/dev/null 2>&1"));
        c
    };
    cmd.current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

type PipeReader = Box<dyn std::io::Read + Send>;

/// 单管道捕获上限：孙进程占住管道时读者线程照常退出（不再无限阻塞），
/// 且缓冲有界——曾是无界 `read_to_string`，孙进程每写一行都追加进永不
/// 被消费的 String（D10-4）。锚点只消费尾部，尾部保留语义不变。
const PIPE_CAPTURE_CAP_BYTES: usize = 1 << 20;

/// 管道→字符串读取线程（exit 后由 [`run_exec_anchor`] 带超时回收）。
/// 有界读循环：累计超 [`PIPE_CAPTURE_CAP_BYTES`] 丢头保尾——孙进程占住
/// 管道时曾是无界 `read_to_string`，每写一行都追加进永不消费的 String
/// （D10-4：缓冲无界增长面已封；安静孙进程仍可让线程阻塞在 read 上，
/// 但内存有界，主线程 2s 宽限后照常放弃）。锚点只消费尾部，语义不变。
fn spawn_pipe_reader(pipe: Option<PipeReader>) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
        if let Some(mut p) = pipe {
            use std::io::Read;
            let mut chunk = [0u8; 8192];
            loop {
                match p.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend(&chunk[..n]);
                        if buf.len() > PIPE_CAPTURE_CAP_BYTES {
                            let drop = buf.len() - PIPE_CAPTURE_CAP_BYTES;
                            buf.drain(..drop);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let text = String::from_utf8_lossy(buf.make_contiguous()).to_string();
        let _ = tx.send(text);
    });
    rx
}

/// stdout+stderr 合并取尾（[`EXEC_OUTPUT_TAIL_CHARS`] 字符；二进制输出
/// lossy 容忍）。
fn truncate_tail(stdout: &str, stderr: &str) -> String {
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout);
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str("[stderr] ");
        combined.push_str(stderr);
    }
    let total = combined.chars().count();
    if total <= EXEC_OUTPUT_TAIL_CHARS {
        return combined.trim_end().to_string();
    }
    let skipped = total - EXEC_OUTPUT_TAIL_CHARS;
    format!(
        "…（前 {skipped} 字符截断）{}",
        combined.chars().skip(skipped).collect::<String>()
    )
    .trim_end()
    .to_string()
}

fn run_one(
    anchor: &AnchorCheck,
    workspace_root: &Path,
    project_root: Option<&Path>,
    delivery_text: &str,
) -> (bool, String) {
    match &anchor.kind {
        AnchorKind::ContentRegex => {
            let Some(pattern) = &anchor.pattern else {
                return (false, "锚点缺少正则模式".to_string());
            };
            match Regex::new(pattern) {
                Ok(re) => {
                    // 自指防御（2026-09-12 UAT T30③ 实证）：交付文本逐字
                    // 引用锚点行本身（重派 prompt 回显、复述验收标准的汇报）
                    // 不构成满足锚点的证据——剥离全部锚点行原文后再匹配。
                    // 否则 worker 只要把任务卡原样抄回来，re: 锚点就在引文
                    // 里自命中 → 假 PASS → 自动收货。
                    let cleaned = delivery_text.replace(&anchor.raw, "");
                    if re.is_match(&cleaned) {
                        (true, format!("交付文本命中 /{pattern}/"))
                    } else {
                        (
                            false,
                            format!("交付文本未命中 /{pattern}/{}", prose_pattern_hint(pattern)),
                        )
                    }
                }
                Err(e) => (false, format!("正则编译失败（{e}）")),
            }
        }
        AnchorKind::FileExists => {
            match resolve_anchor_path(workspace_root, &anchor.target) {
                // resolve 内部 canonicalize 成功 = 存在。
                Ok(path) => (true, format!("文件存在（{}）", path.display())),
                Err(primary_err) => match project_root {
                    // 主根缺席 → 项目目录 fallback（档案管线合并产物）。
                    Some(dir) => match resolve_anchor_path(dir, &anchor.target) {
                        Ok(path) => (true, format!("文件存在（{}，项目目录）", path.display())),
                        Err(_) => (false, primary_err),
                    },
                    None => (false, primary_err),
                },
            }
        }
        AnchorKind::FileRegex | AnchorKind::FileContains => {
            // 文件内容类：先解析路径（安全闸），再读内容按谓词判定。
            // 主根判定不过（路径缺席/读取失败/谓词未命中）→ 项目目录
            // fallback 再判一次，任一根满足即过（双根 OR）。
            let primary = read_and_match(anchor, workspace_root);
            if primary.0 {
                return primary;
            }
            if let Some(dir) = project_root {
                let alt = read_and_match(anchor, dir);
                if alt.0 {
                    return (true, format!("{}（项目目录）", alt.1));
                }
            }
            primary
        }
        // exec 锚点由 [`run_anchors_dual_ext`] 统一分派（策略闸 + 执行根
        // 选择在那里），不经本函数。
        AnchorKind::Exec => unreachable!("exec 锚点由 run_anchors_dual_ext 分派"),
    }
}

/// 文件内容类锚点的单根判定（resolve 安全闸 → 限量读 → 谓词）。
/// 返回 `(通过, 明细)`——失败明细如实携带失败环节（主根失败时原样上抛）。
fn read_and_match(anchor: &AnchorCheck, root: &Path) -> (bool, String) {
    let path = match resolve_anchor_path(root, &anchor.target) {
        Ok(p) => p,
        Err(e) => return (false, e),
    };
    let content = match read_limited(&path) {
        Ok(c) => c,
        Err(e) => return (false, e),
    };
    match &anchor.kind {
        AnchorKind::FileRegex => {
            let Some(pattern) = &anchor.pattern else {
                return (false, "锚点缺少正则模式".to_string());
            };
            match Regex::new(pattern) {
                Ok(re) => {
                    if re.is_match(&content) {
                        (true, format!("文件命中 /{pattern}/"))
                    } else {
                        (
                            false,
                            format!("文件未命中 /{pattern}/{}", prose_pattern_hint(pattern)),
                        )
                    }
                }
                Err(e) => (false, format!("正则编译失败（{e}）")),
            }
        }
        AnchorKind::FileContains => {
            let Some(keyword) = &anchor.pattern else {
                return (false, "锚点缺少关键词".to_string());
            };
            if content.contains(keyword.as_str()) {
                (true, format!("文件包含「{keyword}」"))
            } else {
                (false, format!("文件不包含「{keyword}」"))
            }
        }
        _ => unreachable!("read_and_match 只被文件内容类调用"),
    }
}

/// 文件读取（带大小上限；非 UTF-8 / 超限 / IO 错误统一诚实报错）。
fn read_limited(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("文件读取失败：{e}"))?;
    if meta.len() > MAX_ANCHOR_FILE_BYTES {
        return Err(format!(
            "文件超过 {}MB，锚点检查拒绝读取",
            MAX_ANCHOR_FILE_BYTES / (1024 * 1024)
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("文件读取失败（非 UTF-8？）：{e}"))
}

/// 路径安全·形态闸（解析期，FS 无关）：仅接受 workspace 内相对路径的
/// 字面形态。拒绝：绝对路径 / Windows 盘符与 UNC 形态 / `..` 穿越 /
/// 8.3 短名组件（`RUNNER~1` 家族教训：canonicalize 前先拦，杜绝短名
/// 别名语义绕过）。解析期拒绝 = 不安全锚点回落语义项 + 告警，不产出生
/// 效锚点（T2-3 语义：不安全的验收标准是标准自身的毛病，不惩罚执行者）。
pub fn validate_anchor_path_shape(raw: &str) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("锚点路径为空".to_string());
    }
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err(format!("锚点路径拒绝绝对路径：{raw}"));
    }
    // Windows 形态（UNC / 盘符 / 根相对 `\foo`）在**所有平台**拒绝——本闸
    // 契约是「解析期，FS 无关」（见函数头注释），集群是跨平台的：Linux
    // master 审 Windows worker 形态的锚点必须同样判不安全，不能因运行平台
    // 放空（`C:\evil` 在 Linux 上只是普通文件名字符串，is_absolute=false，
    // 此前 cfg(windows) 门控导致 Linux 全放行——2026-09-12 CI Linux 红根修，
    // remote_file_anchor_gate / parent_malicious_anchor 两测试实证）。
    if raw.starts_with("\\\\") {
        return Err(format!("锚点路径拒绝 UNC 形态：{raw}"));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!("锚点路径拒绝盘符形态：{raw}"));
    }
    if bytes.first() == Some(&b'\\') {
        // Windows 上由下方 components() 的 RootDir 臂兜住；非 Windows 平台
        // `\` 是普通字符，需在此显式拒绝根相对形态。
        return Err(format!("锚点路径拒绝根相对形态（\\foo）：{raw}"));
    }
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(c) => {
                if has_83_short_name(&c.to_string_lossy()) {
                    return Err(format!("锚点路径拒绝 8.3 短名组件：{raw}"));
                }
            }
            std::path::Component::CurDir => {}
            _ => {
                // ParentDir（`..`）/ RootDir（`\foo`）/ Prefix（`C:\`）全部拒绝。
                return Err(format!("锚点路径拒绝越界组件（`..` / 根 / 盘符）：{raw}"));
            }
        }
    }
    Ok(())
}

/// 路径安全·运行时闸（纵深防御后备闸）：形态闸之外再 canonicalize +
/// workspace 前缀校验（拦符号链接越界）。失败 = 锚点未通过（保守——
/// 运行期到达此处的都是形态合法的锚点，失败即真实环境意外）。
fn resolve_anchor_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    validate_anchor_path_shape(raw)?;
    let raw = raw.trim();
    let rel = Path::new(raw);
    let joined = root.join(rel);
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("锚点文件不存在或不可达（{raw}）：{e}"))?;
    let root_canonical = root
        .canonicalize()
        .map_err(|e| format!("workspace 根解析失败：{e}"))?;
    if !canonical.starts_with(&root_canonical) {
        return Err(format!("锚点路径越出 workspace 子树：{raw}"));
    }
    Ok(canonical)
}

/// 8.3 短名组件判定：`NAME~<数字>` 后跟组件结尾或 `.`（`RUNNER~1`、
/// `ABC~1.TXT`；`my~2notes.txt` 这类普通文件名不受影响）。
fn has_83_short_name(comp: &str) -> bool {
    let bytes = comp.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'~' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let after = &bytes[i + 2..];
            let digits = after.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == after.len() || after[digits] == b'.' {
                return true;
            }
        }
    }
    false
}

/// 锚点组全过？（空组 = 过——无锚点的单完全走旧路径，零回归面。）
pub fn all_passed(results: &[AnchorResult]) -> bool {
    results.iter().all(|r| r.passed)
}

/// 通过摘要（拼进 LLM user prompt：`## 客观锚点检查（已通过）` 段落体）。
pub fn render_anchor_summary(results: &[AnchorResult]) -> String {
    let mut out = String::new();
    for r in results {
        out.push_str(&format!("- ✅ [{}] {}\n", r.anchor.kind.as_str(), r.detail));
    }
    out
}

/// 失败明细（FAIL 短路评论体；per-anchor 逐条可读）。
pub fn render_anchor_failures(results: &[AnchorResult]) -> String {
    let mut out = String::new();
    for r in results.iter().filter(|r| !r.passed) {
        out.push_str(&format!(
            "- ❌ `{}`（{}）：{}\n",
            r.anchor.raw,
            r.anchor.kind.as_str(),
            r.detail
        ));
    }
    out
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;

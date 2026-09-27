//! 主 system prompt 段落池（pro 体系）。
//!
//! 段落纯静态（`include_str!` 编译期嵌入），无运行时插值——平台/时间等
//! 动态内容走 `loop::messages` 的每轮快照通道，保 prompt cache 前缀字节
//! 稳定。组装确定性：同一层级永远按注册表顺序渲染，同输入字节级一致
//! （单测钉死）。

// ---------------------------------------------------------------------------
// 段落池
// ---------------------------------------------------------------------------

/// 段落层级。组装顺序：Pre 段 → 人格文件段（context.rs 注入）→ Post 段 →
/// 环境段（context.rs 注入）。
///
/// - `Pre`：身份与安全基座——先于用户自定义人格文件，确保任何人格下安全
///   与身份口径都在最前。
/// - `Post`：行为准则段池——渲染在人格文件之后，用户自定义可以覆盖行为
///   细节，但不能越过安全基线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Pre,
    Post,
}

/// 单个段落。
struct Segment {
    /// 段落标识（日志与测试用）。
    #[allow(dead_code)]
    id: &'static str,
    /// 段落正文（全中文 markdown，编译期嵌入）。
    text: &'static str,
    /// 渲染层级。
    layer: Layer,
}

/// 段落池注册表（单一真相源）。确定性顺序 = 表内顺序；增删段落只改这里。
static SEGMENTS: &[Segment] = &[
    Segment {
        id: "identity_base",
        text: include_str!("segments/identity_base.md"),
        layer: Layer::Pre,
    },
    Segment {
        id: "safety_policy",
        text: include_str!("segments/safety_policy.md"),
        layer: Layer::Pre,
    },
    // —— Post 层：行为准则段池（人格文件之后渲染）——
    Segment {
        id: "output_rules",
        text: include_str!("segments/output_rules.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "communicating",
        text: include_str!("segments/communicating.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "operations",
        text: include_str!("segments/operations.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "act_dont_derive",
        text: include_str!("segments/act_dont_derive.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "delivering",
        text: include_str!("segments/delivering.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "reporting",
        text: include_str!("segments/reporting.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "autonomy",
        text: include_str!("segments/autonomy.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "code_style",
        text: include_str!("segments/code_style.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "tool_routing",
        text: include_str!("segments/tool_routing.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "json_hygiene",
        text: include_str!("segments/json_hygiene.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "plan_mode",
        text: include_str!("segments/plan_mode.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "workspace_rules",
        text: include_str!("segments/workspace_rules.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "confirm_authorize",
        text: include_str!("segments/confirm_authorize.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "injection_defense",
        text: include_str!("segments/injection_defense.md"),
        layer: Layer::Post,
    },
    Segment {
        id: "pronoun_language",
        text: include_str!("segments/pronoun_language.md"),
        layer: Layer::Post,
    },
];

/// 渲染一个层级的全部段落，段落间以空行连接；空白段落跳过。
pub fn render_layer(layer: Layer) -> String {
    let parts: Vec<&str> = SEGMENTS
        .iter()
        .filter(|s| s.layer == layer)
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// 入口形态变体（gap ⑤：入口身份）
// ---------------------------------------------------------------------------

/// 入口形态。同一 agent 从不同入口驱动时，运行语境不同（交互常驻 / 无头
/// 单任务 / 编辑器接入），身份段后追加一小节入口说明。设计红线：默认
/// `Interactive` 的补充为**空字符串**——gateway 主链路渲染字节与历史
/// 完全一致（golden 字节不变测试是安全网）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Entrance {
    /// 交互常驻（gateway Dashboard / IM 通道）：默认形态，无补充。
    #[default]
    Interactive,
    /// 无头单任务（`nemesisbot run`）：一次性交付，跑完即退。
    Headless,
    /// 编辑器接入（`nemesisbot acp`，ACP 协议）：面向编辑器协作。
    Acp,
}

impl Entrance {
    /// Pre 层尾部追加的入口说明；Interactive 恒为空（字节不变红线）。
    fn supplement(self) -> &'static str {
        match self {
            Entrance::Interactive => "",
            Entrance::Headless => {
                "## 运行形态：单任务无头模式\n你当前以无头单任务模式运行：没有交互界面，\
                 本次进程只处理一个任务，跑完即退出。因此：不要等待后续追问——必要的澄清\
                 用 question 工具一次性结构化提出，否则按最合理的默认假设继续，并在交付时\
                 明确列出假设；交付物必须自包含（结论、依据、验证结果齐全），因为这是你\
                 唯一一次开口的机会。"
            }
            Entrance::Acp => {
                "## 运行形态：编辑器接入\n你当前通过 ACP 协议接入用户的代码编辑器，\
                 在编辑器会话中与用户协作。因此：回复聚焦代码任务本身，不输出与编辑器\
                 无关的寒暄和操作指引；文件改动小步进行、保持可编译——用户会随时在\
                 编辑器里查看、修改和撤销你的产物。"
            }
        }
    }
}

/// 渲染层级 + 入口变体。`Pre` 层在安全基座之后追加入口说明（属于身份
/// 语境）；`Post` 层不受入口影响（行为准则与入口无关）。
pub fn render_layer_for(layer: Layer, entrance: Entrance) -> String {
    let base = render_layer(layer);
    match layer {
        Layer::Pre => {
            let sup = entrance.supplement().trim();
            if sup.is_empty() {
                base
            } else {
                format!("{base}\n\n{sup}")
            }
        }
        // Post 层恒等（含 Interactive：完全一致）。
        Layer::Post => base,
    }
}

// ---------------------------------------------------------------------------
// 长度预算
// ---------------------------------------------------------------------------

/// pro 模式 system prompt 软预算（UTF-8 字节数，不含工具描述——工具描述
/// 预算归 `loop::tool_defs` 侧的折叠机制管）。当前段落池 + 典型人格文件
/// 体量远低于此值；超限只说明段落池或人格文件失控，告警治理而非截断。
pub const SOFT_BUDGET_BYTES: usize = 28_000;

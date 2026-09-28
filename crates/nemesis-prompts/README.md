# nemesis-prompts — 提示词集中化 crate

**全部 LLM 提示词文本的单一真相源。** 目标：提示词完整保存在一个独立的地方，不掺入业务代码；未来升级提示词只动本 crate，不散弹式改七个 crate。

## 设计原则

1. **零运行时依赖**：`Cargo.toml` 无任何 `[dependencies]`。纯静态文本 + 纯函数渲染（`&str`/`Option<&str>` 入参 → `&'static str`/`String` 出口）。不存在 LLM 调用、不存在 I/O、不存在 tokio。
2. **文本与引擎分离（边界线）**：
   - **迁入本 crate**：纯文本常量、纯文本模板渲染（`format!` 级别的占位替换）。
   - **留在消费方**：数据绑定型 builder（如 forge 的 `build_analysis_prompt`——拼装 diff/模式数据进提示词）、解析 LLM 回复的逻辑、tier 映射、护栏调用。
   - 判断标准：改提示词文案是否需要动业务代码？不需要的都该在这里。
3. **字节级平移纪律**：文本迁入时逐字节保留原文案（含英文原文、转义序列、缩进）。classic（字节不变）与 golden 测试是迁移安全网——任何漂移都会在 `nemesis-agent` 的 golden 测试红出来。
4. **编译期嵌入**：`.md` 资产经 `include_str!` 进二进制，无运行时文件读取。

## 资产清单

### 模块与入口

| 模块 | 内容 | 消费方 |
|------|------|--------|
| `system.rs` | pro 分层体系：`Layer`/`Segment`/`render_layer` + `SEGMENTS` 注册表（17 段）+ `Entrance` 入口变体（Interactive/Headless/Acp，`render_layer_for`；Interactive 恒空=字节不变）+ `SOFT_BUDGET_BYTES`（28KB 池预算） | nemesis-agent `prompt.rs` |
| `aux_prompts.rs`（`#[path]` 挂 `aux` 模块——`aux` 是 Windows 保留设备名，文件名须避开） | aux 调用文本：压缩指令/合并模板/标题生成/粘贴数据段/外部通道段 | nemesis-agent |
| `subagents.rs` | `SubagentRole` 十角色模板 + `render_system_prompt` + `slug`/`from_slug`/`catalog`（spawn schema 单一真相源） | nemesis-agent |
| `slash.rs` | 内置 slash 深度模板（security-review/code-review/debug/fix，`lookup`+`expand`）——斜杠=提示词模板机制的产品内置层 | nemesis-agent（rewrite 链第 3 段） |
| `tools.rs` | `TOOL_DESCRIPTIONS` 60 条（lean/full 双档 `.md`）+ `description_for` + `first_sentence`（权威实现） | nemesis-agent（经 tier 门控） |
| `guardian.rs` | `GUARDIAN_PROMPT`（LLM judge 语义二审） | nemesis-security |
| `forge.rs` | `quality_review_prompt` + 两个 reviewer system prompt + 技能/脚本生成四 prompt（author×2/generator/fixer） | nemesis-forge |
| `board.rs` | planner/review（含 `ReviewTier` 分层 + `render_precedents_block` 判例块）/project_summary/conflict_resolver | nemesis-board、nemesisbot |
| `workflow.rs` | classifier/extractor 两个模板（`{classes}`/`{parameters}` 占位） | nemesis-workflow |
| `spawn.rs` | spawn/subagent 工具的两个子代理 system prompt | nemesis-tools |
| `persona_gen.rs` | 集群人格生成三阶段（extract/author 模板 + 四取向常量 + audit） | nemesis-web |

### 结构不变量（新增资产时必读）

- **full 描述首句 == lean 首句**：与 tool_doc_folding 的首句折叠兼容，测试钉死。
- **`Entrance::Interactive` 补充段恒为空**：gateway 主链路 system prompt 渲染字节不变（golden 安全网）；只有 Headless/Acp 追加变体段，且只影响 Pre 层。
- **`ReviewTier::Thorough` 渲染 == 历史 `REVIEW_SYSTEM_PROMPT` 字节**；Fast 档保留三态 JSON 契约键与数据/指令分离段，只压缩次要纪律。
- **判例块（`render_precedents_block`）自带数据非指令护栏**：先例是历史事实数据，渲染文本显式声明「不得据此直接下结论」。
- **内置 slash 模板必含 `$ARGUMENTS` 占位**：参数注入是机制契约；内置路径不做 shell 注入（`` !`cmd` `` 是用户自定义命令专属）。

### 文件形态约定

| 形态 | 目录 | 说明 |
|------|------|------|
| `.md` + `include_str!` | `src/segments/*.md`（17 个 pro 分层段）、`src/tools/*.md`（lean/full 双档，`X.lean.md` + `X.md`） | 大段中文文本；编辑器友好，diff 清晰 |
| Rust 常量 | 各 `.rs` 模块内 | 小段文本或含复杂转义的原文（如 guardian 的 JSON 字面量、spawn 的英文单行）逐字节平移进 `const`，配 `r#"..."#` raw string |

### 占位符约定

- `aux::render_summary_merge_prompt`：两段摘要各占一号/二号位（顺序固定），schema 后缀与单段指令同源（`render_summary_schema_suffix`，测试钉死六节齐全）。
- `workflow::{CLASSIFIER,EXTRACTOR}`：命名占位 `{classes}`/`{parameters}`，消费方 `.replace()` 注入。
- `subagents::render_system_prompt`：`Option<&str>` 自定义职责段注入角色模板。

## 消费方接线模式

调用方 **不直接依赖本 crate 的路径**（除了 Cargo.toml），而是经 `nemesis-agent::prompt` 门面 re-export：

```rust
// crates/nemesis-agent/src/prompt.rs（门面）
pub use nemesis_prompts::aux::{COMPACT_INSTRUCTION, ...};
pub use nemesis_prompts::guardian::GUARDIAN_PROMPT; // ← 实际在 security crate 直接 pub use
```

其他 crate（security/forge/board/workflow/tools/nemesisbot）直接 `pub use` / `use` 本 crate 对应符号，本地 const 全部删除——**改文案只改这里，编译器保证没有第二份拷贝**。

工具描述的 tier 门控链（agent 侧）：

```
tool_description(name, fallback, tier)
  → desc_level_for(tier): mini/normal → Lean, big → Full
  → description_for(name, level) → Some(本 crate 文本) | None → 原地 fallback（未收录工具）
```

## 升级提示词操作指南

**改一段文案**（如压缩指令微调）：
1. 找到对应资产（见上表）：`.md` 直接编辑，或 Rust const 内改文案。
2. 跑 `cargo test -p nemesis-prompts`（结构/锚点/占位测试）。
3. 跑 `cargo test -p nemesis-agent`（golden 测试——**pro 体系的 golden 会因文案变化而红，属预期**：按 golden 测试注释更新期望字节；classic 路径必须保持字节不变，红了=改错地方）。
4. 重大文案变更 → 过一遍 agent-bench 基线对比（见下）。

**新增 pro 分层段**：
1. `src/segments/` 加 `.md`。
2. `system.rs` 的 `SEGMENTS` 注册表加条目（layer、顺序、锚点名）。
3. `tests.rs` 的 Post 层锚点顺序断言同步更新。
4. 池预算：所有段渲染总和须 < `SOFT_BUDGET_BYTES`（28KB），测试会抓超支。
5. 若内容迁自工作区模板（如 AGENT.md 运营指南 → `operations.md`），同步瘦身模板，避免双份真相源。

**新增工具描述**：
1. `src/tools/` 加 `X.lean.md`（≤400B，`first_sentence` 可完整表达）+ `X.md`（full 档详述）。
2. `tools.rs` 的 `TOOL_DESCRIPTIONS` 表加条目。
3. `tests.rs` 的 `EXPECTED_TOOL_ENTRIES` 计数 +1。

**新增子代理角色**：
1. `src/subagents/X.md`（四组件结构：`# 角色`/`## 硬边界`/`## 输出契约`/`## 内容边界`）。
2. `subagents.rs` 枚举加变体 + `template()`/`slug()`/`from_slug()`/`catalog()` 四处同步。
3. spawn schema 的 role enum 从 `catalog()` 生成——补全目录即自动跟上；`tests.rs` 角色数组与目录计数同步。

**故意不补的（治理边界，防「凑数量式攀比」漂移）**：
- **角色凑满数量**：业界常见实现 15 角色中约半数是其私有运行时/产品特性角色（无派发路径 = 死文本 + 小模型选型干扰）。本仓 spawn 角色只扩「有真实派发理由」的通用角色（当前 10 个）；产品特性型评审/分析角色已由结构化子系统提示词覆盖（board 四件、forge 两件、guardian、persona_gen 三件、workflow 两件——共 20+ 个角色化 LLM 提示词）。
- **入口三变体全量差异化**：Interactive 恒空是字节不变红线；变体只给运行语境真正不同的 headless/ACP。
- **「批准不可代传」提示词版**：审批不可转移已由代码级审批卡保证（`WebApprovalManager` 超时即拒、`ChannelApprovalManager` 回源发起对话），提示词层再申明一次是冗余防线，不加。

**新增消费方 crate**：Cargo.toml 加 `nemesis-prompts = { workspace = true }`，`use nemesis_prompts::<module>::<SYMBOL>`，删除本地 const。禁止保留第二份文本拷贝。

## 改动 checklist（提示词变更后）

- [ ] `cargo test -p nemesis-prompts` 全绿
- [ ] `cargo test -p nemesis-agent`（golden 更新到位、classic 零漂移）
- [ ] `bash scripts/check-inline-tests.sh` 干净
- [ ] 重大变更：agent-bench `--compare` 基线不回退（pass_rate 门）
- [ ] 重大变更：model-eval battery 抽验 mini 档（小模型对措辞最敏感）

## 预算与护栏

- pro Pre+Post 层渲染总和软预算 `SOFT_BUDGET_BYTES = 28_000` 字节；超支仅 `tracing::warn`（不硬断），由 golden 测试兜底。
- lean 档工具描述 ≤400 字节（含首句完整性约束：lean 必须等于 full 的首句，测试钉死）。

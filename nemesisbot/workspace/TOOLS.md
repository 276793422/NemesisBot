# TOOLS.md - 本地笔记

技能定义工具*如何*工作。这个文件是*你的*具体信息 — 你设置独有的东西。

## 这里放什么

像这样的东西：

- 摄像头名称和位置
- SSH 主机和别名
- TTS 首选语音
- 扬声器/房间名称
- 设备昵称
- 任何特定于环境的信息

## 示例

```markdown
### 摄像头

- 客厅 → 主区域，180° 广角
- 前门 → 入口，运动触发

### SSH

- 家用服务器 → 192.168.1.100, 用户: admin

### TTS

- 首选语音："Nova"（温暖，略带英式口音）
- 默认扬声器：厨房 HomePod
```

## 为什么要分开？

技能是共享的。你的设置是你自己的。将它们分开意味着你可以更新技能而不会丢失笔记，并且可以分享技能而不会泄露你的基础设施。

---

添加任何有助于你完成工作的东西。这是你的备忘单。

## 分层指令文件（AGENTS.md / CLAUDE.md）

NemesisBot 会自动读取工作区内分层的 `AGENTS.md` 与 `CLAUDE.md`：从工作区根目录到当前会话目录，每一层的这两个文件都会被注入为分层指令。约定：同目录两者内容相同时只认 AGENTS.md；越深层的目录优先级越高；它们不覆盖系统指令与用户直接指令。给仓库写编码规范、构建命令、项目结构说明时，任选其一即可。详见 `docs/INFO/2026-09-04_agents-md-compatibility.md`。

## 并发消息（排队与 `!` 紧急插话）

上一条消息还在处理时又发新消息，行为由配置 `agents.defaults.concurrent_request_mode` 决定（默认 `queue`，Dashboard「代码开发」页或 `nemesisbot agent set concurrent-mode` 可改，重启 Agent 生效）：

- `reject`：忙时直接拒绝（旧版行为，回执「会话正在处理」）。
- `queue`（默认）：忙时自动排队，当前轮结束后继续处理，回执以 ⏳ 开头。
- `steer`：queue 之上加 `!` 前缀紧急插话——长任务跑着时发「`! 先停一下，改用 X 方案`」，消息会在 AI 下一步思考前注入（回执以 ⚡ 开头）；非紧急消息去掉 `!` 走排队。

排队容量由 `agents.defaults.queue_size`（默认 8）控制，满员时回执「排队已满」。

## todowrite 工具（多步任务清单）

多步任务（≥3 步，或用户给了清单）先用 `todowrite` 建清单，再逐步执行：

1. 开工前一次调用写全清单：待办 `pending`，正在做的**恰好一项** `in_progress`。
2. 每完成一步就再调一次 `todowrite` 更新状态（**全量提交**——每次调用替换整个清单，所有条目都要带上，包括没变的）。
3. 同一时间最多一项 `in_progress`；全部 `completed` 才算任务完成。
4. 一句话能做完的单一琐碎任务不必建清单。

示例（任务：重构解析器并测试）：

```json
{"todos": [{"content": "读取现有解析器", "status": "completed"}, {"content": "重构 token 化逻辑", "status": "in_progress"}, {"content": "补充单元测试", "status": "pending"}]}
```

## 后台进程三件套（background_start / background_output / background_kill）

长跑命令（dev server、watcher、耗时构建）用后台三件套，别用 `exec` 干等超时：

1. `background_start {"command": "npm run dev", "cwd": "my-app"}` → 立刻返回 `job_id`（进程在网关里跑，重启 Agent 不受影响）。
2. `background_output {"job_id": 1}` 读输出；响应里有 `next_offset`，把它传回 `{"job_id": 1, "offset": <next_offset>}` 就只拿新增字节（每次最多 8KB）。`running: false` 时 `exit_code` 是退出码。
3. `background_kill {"job_id": 1}` 停止任务（整棵进程树都会被杀，dev server 拉起的子进程不留孤儿）。

注意：单任务输出最多保留 256KB（超出丢头部保尾部，`dropped_bytes` 会如实告诉你丢了多少）；同时最多 16 个任务，满员时已完成的旧任务自动腾位。

## run_checks 工具（构建/测试聚焦运行器）

跑构建、测试、lint 优先用 `run_checks`，别用 `exec`——编译/测试输出动辄几千行，全量回灌烧 context 还淹没真正的 error 行：

1. `run_checks {"scope": "all"}` 按项目类型自动映射命令（检测 Cargo.toml/package.json/go.mod/pyproject.toml/pom.xml），顺序跑 build → test → lint，**某阶段失败立即早停**（build 挂了再跑 test 只会复读同一批错误）。
2. 回灌只有三类信息：每阶段一行状态（`▶ build: exit 0 (2.1s)`）、测试统计（`passed 4 / failed 1 / skipped 0`）、**去重后的失败签名行**（error[E0308]、FAILED、AssertionError…，最多 40 行）。
3. 全量输出无条件存档到 spill 文件，回灌里带路径——要细节时用 `read_file`（offset/limit）或 `grep` 检索，别要求重跑。
4. `filter` 参数只对 test 有意义：`run_checks {"scope": "test", "filter": "my_test"}` → `cargo test my_test`（修完单个失败用这个快速重验，别全量跑）。

边界（诚实拒绝不硬猜）：python 无标准 build 命令、maven 无内置 lint——这些格子未映射，报错并提示改用 `exec` 跑项目自己的命令；工作区根认不出项目类型同理。

## 工具隐藏（agents.hidden_tools）

config.json 的 `agents.hidden_tools` 列表可以把工具从模型视野里**彻底移除**（不出现在工具清单，调用也会被拒）：

- 条目支持通配：`"mcp_*"` 隐藏全部 MCP 工具，`"*"` 隐藏一切（慎用）；精确名如 `"exec"` 只隐藏该工具。
- 双闸生效：工具清单供给侧剔除 + 调用分发侧拦截（改配置后仍带着旧清单的请求也调不动）。
- 改 config.json 即时生效（下一轮对话），无需重启；tier 档位与隐藏叠加——`model set-tier` 只调档位，被隐藏的工具任何档位都看不到。

## 消息内 @文件引用

用户消息里可以直接引用工作区文件，内容会自动内联进对话（无需再调 read_file）：

- `@src/main.rs` —— 内联整个文件（≤8KB，超出截断并注明原始大小）。
- `@src/main.rs#L10` —— 只内联第 10 行；`@src/main.rs#L10-20` 内联 10–20 行（1-based 闭区间）。
- 引用走安全 8 层管线（与 read_file 工具同权），被拒或文件不存在会诚实注记 `[文件引用失败: …]`。
- 图片扩展名（png/jpg/…）的 `@引用` 不做文本内联——它们走图片附加管线以像素形态进入对话。
- `@单词`（无路径分隔符无扩展名）视为普通提及，不会触发文件读取。
- Dashboard 输入框敲 `@` 会弹出工作区路径补全（与 fs watcher 共用忽略表，`logs/`、`target/` 等运行时目录不在列表里）。


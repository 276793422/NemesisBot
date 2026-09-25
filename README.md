# NemesisBot (Rust)

<div align="center">

**安全第一的 AI 智能管家（Rust 版）**

一个轻量级、高安全性的个人 AI 助手，专注于安全保障和拟真使用体验。

（这句话是人写的）以下内容大部分都是AI胡掰，可信可不信，凡是AI说“稳定”的地方，可靠度都和投硬币差不多。

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust Version](https://img.shields.io/badge/Rust-1.85+-000000?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20Android-lightgrey)](https://github.com/276793422/NemesisBot)

</div>

---

## 核心特色

### 安全保障体系

**企业级安全审计框架** - 不是简单的文件隔离，而是完整的安全管控系统

- **ABAC 策略引擎** - 基于属性的访问控制，细粒度权限管理
- **操作审计日志** - 完整记录所有危险操作，可追溯、可审计
- **安全中间件** - 对文件、进程、网络、硬件操作进行实时监控
- **分级危险等级** - LOW / MEDIUM / HIGH / CRITICAL 四级风险控制
- **工作目录隔离** - 默认启用沙箱模式，保护系统安全
- **病毒扫描集成** - 内置 ClamAV 引擎，文件写入/下载/执行时自动扫描

**安全 ≠ 功能限制**
- 在安全保障的前提下，提供完整的工具能力
- 可配置的安全策略，满足不同使用场景
- 实时监控和拦截，防止意外损害

### 分布式集群

**多节点协同 - 让多个 AI 一起工作**

- **角色分离** - manager / coordinator / worker / observer / standby
- **业务分类** - design / development / testing / ops / deployment / analysis / general
- **自定义标签** - 灵活的多维度分类体系
- **加密 UDP 自动发现** - 局域网内自动发现其他节点（AES-256-GCM 加密广播）
- **异步 RPC 通信** - 节点间非阻塞远程调用，支持多步骤工具链，令牌认证保护
- **续行快照模式** - LLM 上下文在异步调用点保存，回调到达后自动续行
- **运行时身份编辑** - 通过 Dashboard 实时修改节点名称、角色、分类、标签
- **黑名单机制** - 删除节点后阻止自动重新发现，支持手动解除
- **智能地址解析** - 自动将 0.0.0.0 替换为真实局域网 IP
- **Dashboard 6-Tab 管理** - 概览、拓扑、身份、任务、日志、设置一体化管理
- **静态+动态配置** - 手动配置已知节点，自动发现新节点
- **连接池管理** - 高效的连接复用和管理
- **限流保护** - 防止 RPC 调用过载

### 编码代理（把 Bot 当代码开发工具用）

**多入口编码 agent**（2026-09 devtool-upgrade 落地），安全 8 层 + 审批 + 急停在所有新入口同源生效——headless、编辑器、IM 通道都不是安全旁路：

- **headless 单任务** - `nemesisbot run "任务"` 无端口无 UI 跑完即退；`--mode plan` 只读规划（写类工具拦截，仅放行 plans/）、`--format json` NDJSON 事件流（tool_start/tool_end/turn/final/error）、`--max-turns`、`--workspace` 独立工作区、退出码可脚本化
- **ACP 编辑器接入** - `nemesisbot acp`（stdio JSON-RPC），Zed 等 ACP 客户端直接驱动；审批桥接到编辑器 permission 请求（超时=拒绝，失败关闭）
- **plan/build 双模式** - Dashboard 徽标 / `/plan` `/build` 斜杠命令 / WSAPI 随时切换，子代理同罩
- **审批卡 + 结构化提问** - 高危操作弹审批卡（支持"总是允许"前缀记忆规则）；agent 可用 question 工具发选择题阻塞等答（超时交还模型自行判断）
- **checkpoint / undo / redo / diff** - turn 边界自动快照（git 影子库零拷贝 / JSON 快照双形态）；消息级 rewind/redo；会话级文件 diff 查看器
- **编码辅助闭环** - LSP 只读语义查询（definition/references/implementation/hover；rust-analyzer/gopls/ts-ls/pyright/clangd）、编辑后诊断回灌、格式化钩子、run_checks
- **后台进程注册表** - `background_start/output/kill` 三件套：长跑进程查增量输出、分页读、树杀
- **PTY 内嵌终端** - Dashboard 终端 Tab（xterm.js + ConPTY；双闸默认关：编译 feature + `terminal.enabled`）
- **工作区面板** - 文件树（懒展开）、@路径补全、工具调用卡片实时流、断线补拉（全局 seq + SSE Last-Event-ID）
- **会话管理** - 会话分叉（fork）、只读分享链接（token 即凭据）、自动标题、手动 /compact（小模型通道）、AGENTS.md 与 CLAUDE.md 等价兼容

---

## 今天终于四个平台都跑起来了（这块是人写的）

(感谢党感谢政府，让我能测到macos)

**Windows 平台**

![Windows](test-tools/resource/UI/windows_ui.png)

**Linux 平台**

![Linux](test-tools/resource/UI/linux_ui.png)

**Android 平台**(这代码有点老)

![Android](test-tools/resource/UI/android_ui.png)

**MacOS 平台**

![MacOS](test-tools/resource/UI/macos_ui.png)

---

## 快速开始

### 环境要求

- Rust 1.85+（edition 2024）+ Cargo
- Windows 上还需 **MSVC C++ 构建工具**（Rust MSVC 工具链的链接器，`cargo build` 必需）
- Node.js（编译 Vue 前端，LTS 即可）
- Go（仅编译 `test-tools/TestAIServer` 测试模拟器用，可选）
- 平台：Windows / Linux / macOS / Android

> **🚀 Windows 一键自动装环境（全程国内源）**
>
> 如果机器上还没有上面这些工具，直接双击 **`scripts\bootstrap-dev-env.bat`**（或右键“以管理员身份运行”）即可。它会自动下载并安装：MSVC 构建工具 + Rust + Node.js + Go，并配置好 cargo（rsproxy）、npm（npmmirror）、Go（goproxy.cn）的国内镜像，toolchain 也走清华 TUNA 镜像。装完最后用 `cargo check` 自检本项目能编译。
>
> - 需要管理员权限（会弹 UAC，点“是”）
> - MSVC 构建工具这步较大较慢（下载 GB 级，约 10-30 分钟），其余很快
> - 幂等：已装的工具会自动跳过，可安全重复运行
> - 主脚本是 `scripts\bootstrap-dev-env.ps1`，`.bat` 只是启动器
> - 详细执行流程见 `bootstrap-dev-env.ps1` 顶部注释

### 安装

```bash
# 克隆仓库
git clone https://github.com/276793422/NemesisBot_Rust.git
cd NemesisBot_Rust

# Windows 编译（标准构建脚本，带版本注入 + Vue 前端 + 插件编译）
scripts\build-windows.bat

# Linux/macOS 编译
scripts/build-linux.sh        # Linux
scripts/build-macos.sh        # macOS

# Android 交叉编译（需先搭建环境）
scripts\setup-android.bat     # 自动检测/安装 Android NDK + Rust targets
scripts\build-android.bat     # 编译 Android arm64 版本

# 一键清理 + 构建 + 运行（Windows）
scripts\run-demo.bat

# 或手动编译
cargo build --release -p nemesisbot

# 编译 plugin-ui DLL（可选，用于桌面审批弹窗）
cd plugins/plugin-ui
cargo build --release
```

### 定制构建（功能裁剪）

需要 IoT / 低资源 / 精简发行版？所有子系统都是可选 cargo feature，可按需关闭：

```bash
# menuconfig 风格 TUI：勾选功能 → 保存 → 编译 → 拷贝到 bin/bin_customize/
scripts\customize.bat                    # Windows
bash scripts/customize.sh                # Linux/macOS/git-bash

# 直接编译最小 IoT 版（加载 minimal-iot 预设，产物约 10MB，前端按 feature 裁剪）
bash scripts/customize.sh iot

# 或手动指定 feature 子集
cargo build --profile iotsmall -p nemesisbot --no-default-features --features "channels-web,channels-rpc"
```

可裁剪：`cluster` / `forge` / `memory` / `workflow` / `security`(含 scanner) / `voice` / `desktop` / `auth` / `migrate` / `devices` / `health` / `heartbeat` / `sandbox` / `usage`(前端使用统计页，带 echarts) 及各通道。**CLI 子命令随 feature 条件编译**（关 `cluster` 则无 `nemesisbot cluster` 子命令）。**Vue 前端也同步裁剪**——customize 按同一份 feature 选择生成 `web/.env`，router/侧栏门控，Vite 把关闭的页面 tree-shake 掉（不进二进制；minimal-iot 前端从 1.7MB 降到 ~0.6MB）。详见 `docs/REPORT/2026-07-01_feature-trimming-and-build-configurator.md` + `docs/REPORT/2026-07-12_frontend-feature-gating.md`。

### 运行测试

> 前置：`nemesisbot` 的编译依赖前端构建产物 `crates/nemesis-web/static/`（gitignored，不入库）。
> 干净 clone 后先执行 `npm --prefix web install && npm --prefix web run build`，否则 `--workspace`
> 级命令会在此处报错（错误信息含本命令）。

```bash
# 运行所有单元测试
cargo test --workspace

# 运行特定 crate 测试
cargo test -p nemesis-agent
cargo test -p nemesis-cluster

# 运行 CLI 集成测试（纯 bin runner，须 cargo run；依赖拉起真实 gateway + TestAIServer）
cargo run -p integration-test

# 运行 P2P 集群测试（同为纯 bin runner）
cargo run -p cluster-test
```

#### 质量基准（可复现）

质量证据清单（每项附复现命令）见 **[docs/INFO/2026-09-24_公开基准.md](docs/INFO/2026-09-24_公开基准.md)**。
摘要：agent 单 crate lib 测试 2359、CLI 集成 492 断言/22 命令、集群 UAT
T1-T37 双节点端到端、全 workspace 行覆盖 87.85%（cargo-llvm-cov 实测，
CI `--fail-under-lines` 硬门槛防倒退）、测试/生产代码行数比 164%、架构依赖
矩阵 7 规则 CI 强制、内联测试纪律门禁 1537 文件 0 违规、模型能力评估 8 题
探针 + 12 题 × 3 档 battery。

### 初始化

```bash
# 自动配置（推荐）
nemesisbot.exe onboard default

# 使用本地模式
nemesisbot.exe --local onboard default
```

这会创建：
- 主配置文件：`~/.nemesisbot/config.json`
- 工作空间：`~/.nemesisbot/workspace/`
- 安全策略配置：`~/.nemesisbot/workspace/config/config.security.json`
- 身份文件：`IDENTITY.md`, `SOUL.md`, `USER.md`
- 集群配置：`cluster/peers.toml`

### 配置 LLM

```bash
# 添加模型（智谱 GLM 推荐）
nemesisbot model add --model zhipu/glm-4.7 --key YOUR_API_KEY --default

# 设置默认模型
nemesisbot model default --model zhipu/glm-4.7
```

**模型能力分级（小模型支持）**：每个模型在 config.json 有 `model_tier` 字段（`auto`/`mini`/`normal`/`big`），决定给它多少工具、多少次校验重试。`auto` 默认按模型名/参数量自动检测；看不出大小的 opaque 别名建议跑探针实测：

```bash
nemesisbot model probe <name>            # 跑 8 题探针（7 工具 + 1 视觉），按实测能力定档
nemesisbot model set-tier <name> mini    # 手动覆盖（mini=核心 13 工具 / normal=33 / big=全量）
nemesisbot model set-size <name> 30B     # 或直接告诉参数量，auto 检测会采信
```

档位效果：**mini** 给核心 13 个工具 + 校验重试 3 次（小模型减负）；**normal** 33 工具 + 重试 2 次；**big** 全量工具不限 + 重试 1 次。运行中改 tier **无需重启 gateway**（下一轮 LLM 自动重解析）。完整评估方法见 `docs/INFO/2026-07-05_model-support-eval-suite.md`。

### 启动服务

```bash
# 启动网关（Windows 下自动启用系统托盘图标）
nemesisbot gateway

# 访问 Web 界面
# 浏览器打开：http://127.0.0.1:49000
# 默认访问密钥：276793422
```

### 编码代理用法（headless / ACP）

```bash
# headless 单任务：跑完即退，stdout 只承载最终回复，退出码 0=成功 / 1=装配失败或仅 error
nemesisbot run "修复 utils.rs 里的编译警告"
nemesisbot run --mode plan --workspace ./myapp "分析项目结构，输出重构计划"

# 事件流（NDJSON：tool_start / tool_end / turn / final / error），任务从 stdin 读
nemesisbot run --format json - < task.txt

# ACP 编辑器接入（Zed 等 ACP 客户端经 stdio JSON-RPC 驱动）
nemesisbot acp
```

**系统托盘**（Windows）：启动 gateway 后自动在系统托盘显示图标，右键菜单支持：
- 启动/停止服务
- 打开 Web UI（窗口去重，不会重复弹窗）
- 退出程序

**系统托盘**（Linux）：通过 `plugin-ui.so` 运行时加载（需要 GTK3），使用 `libayatana-appindicator3` + GtkMenu/libdbusmenu 协议。主框架 `nemesis-desktop` 不依赖 GTK/tray-icon/winit。已选择继续使用 libayatana-appindicator3（而非 GMenuModel），因为大多数桌面面板不支持 GMenuModel（菜单会不可见）。废弃警告已通过 GLib log filter 抑制。

### 病毒扫描（可选）

```bash
# 启用 ClamAV 引擎
nemesisbot security scanner enable clamav

# 检查安装状态
nemesisbot security scanner check

# 自动下载安装 ClamAV + 病毒库
nemesisbot security scanner install

# 查看扫描引擎状态
nemesisbot security scanner list
```

#### 杀毒软件效果

![杀毒软件效果](test-tools/resource/security/clamav.png)

---

## 执行体隔离与沙盒（安全第 9 层 / 最终防线）

Sandboxie 沙盒是 NemesisBot 安全防线的**第 9 层——最终防御红线**：前 8 层（注入→命令→ABAC→凭据→DLP→SSRF→病毒扫描→审计链）在 gateway 做判断和拦截，沙盒是兜底的**盒级物理隔离**——即便前 8 层都放行了某个高危操作，它在盒里也动不了真盘。这是核心安全能力，不是实验性功能。

LLM 高危本地操作（exec / 文件读写 / grep / git）默认在 gateway 进程内执行。开启**执行体隔离**后，这些操作被剥离到独立子进程跑，进一步套进 **Sandboxie 沙盒**全隔离（断网 + 降权）。两层开关在 `config.json` 的 `executor` 段（默认关，按需启用）：

```json
{
  "executor": {
    "enabled": false,
    "sandbox": false
  }
}
```

- **Layer 1（`executor.enabled=true`）**：每次工具调用 spawn 一个 `nemesisbot.exe` 子进程（角色 `executor`），一行 JSON 请求 / 一行响应经 stdio 往返，跑完即退。安全 8 层仍在 gateway 执行前跑（子进程收到的都是已批准的操作）。`sandbox=false` 时子进程跑在真实环境，纯进程隔离。
- **Layer 2（`executor.sandbox=true`，需 `enabled=true` + Windows）**：子进程经 `Start.exe /box:NemesisBox` 起进 Sandboxie 盒——**断网 + 降权 + 全隔离**。盒里对工作区的写入不自动落真盘，需手动审阅后提交；工作区外的破坏（删越界、注册表、网络）永远关在盒里、提交不了。

沙盒是 **Windows 内核驱动**（mini-filter），首次启用需装驱动 + 服务（触发一次 UAC）：

```bash
nemesisbot sandbox install       # 下载 + 校验 + 解压 Sandboxie Classic（无 UAC，仅取文件）
nemesisbot sandbox start         # 激活引擎：装驱动+服务+写 ini+启动 SbieSvc（触发 UAC）
nemesisbot sandbox status        # 查看驱动/服务/Start.exe 就绪状态
nemesisbot sandbox pending       # 列出盒里待提交的工作区文件
nemesisbot sandbox commit --all  # 把盒里工作区文件提交回真盘
nemesisbot sandbox clear         # 清空盒内容（丢弃待提交）
nemesisbot sandbox stop          # 停止并卸载引擎（--purge 连文件一起删）
```

也可在 Web Dashboard 的「沙盒」页操作（环境检查 / 启动引擎 / 查看待提交文件 / 勾选提交）。沙盒通过 cargo feature `sandbox` 控制（默认开）；裁剪掉后 `nemesisbot sandbox` 命令和 `executor.sandbox` 不可用，但 Layer 1 进程隔离（`executor.enabled`）仍保留。**Linux/macOS/Android 不适用。** 详见 `docs/PLAN/2026-07-09_sandboxie-integration.md`。

---

## 安全配置

### 工作目录隔离

默认配置（推荐）：
```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nemesisbot/workspace",
      "restrict_to_workspace": true
    }
  },
  "security": {
    "enabled": true
  }
}
```

这意味着：
- Bot 只能访问 workspace 目录，其他风险由 security 模块处理
- 所有文件操作受安全策略控制
- 危险操作需要审批或会被拦截

### 操作类型与危险等级

| 级别 | 操作类型 | 说明 |
|------|----------|------|
| **CRITICAL** | process_exec, process_kill, registry_write, system_shutdown | 最高风险 |
| **HIGH** | file_write, file_delete, dir_create, dir_delete, process_spawn | 高风险 |
| **MEDIUM** | file_edit, file_append, registry_read, network_download | 中等风险 |
| **LOW** | file_read, dir_list, network_request, hardware_i2c | 低风险 |

---

## 集群管理

### 初始化集群节点

```bash
# 初始化为设计类管理者
nemesisbot cluster init \
  --name "Design Lead Bot" \
  --role manager \
  --category design \
  --tags "production,senior"

# 初始化为开发类工作者
nemesisbot cluster init \
  --name "Dev Worker 1" \
  --role worker \
  --category development \
  --tags "backend,junior"
```

### 管理命令

```bash
# 查看集群状态
nemesisbot cluster status

# 查看节点信息
nemesisbot cluster info

# 添加已知节点
nemesisbot cluster peers add \
  --id "node-dev-2" \
  --name "Dev Worker 2" \
  --address "192.168.1.102:49200" \
  --role worker \
  --category development

# 启用/禁用集群
nemesisbot cluster enable
nemesisbot cluster disable
```

---

## 集群通信示例（双端实测）

下面是一组真实捕获的双设备集群通话与应答过程。场景：


![集群节点列表](test-tools/resource/cluster/1.png)


![A 端发起请求](test-tools/resource/cluster/2.png)


![B 端响应](test-tools/resource/cluster/3.png)


![多轮对话](test-tools/resource/cluster/4.png)


![任务详情](test-tools/resource/cluster/5.png)

---

## 工作流示例

就一个图，功能是两轮 LLM 调用，翻译成鸟语再翻译成人话：

![工作流](test-tools/resource/workflow/workflow.png)

---

## 身份系统

可主动配置，也可初始化后第一次对话教AI来自己配置。

### 配置 AI 身份

编辑 `~/.nemesisbot/workspace/IDENTITY.md`：

```markdown
# IDENTITY.md - 我是谁

- **姓名：** 老贾
- **身份：** 智能管家，为你的主人提供各种帮助
- **风格：** 有趣、幽默，遇到科学问题时严谨高效
```

### 配置 AI 灵魂

编辑 `~/.nemesisbot/workspace/SOUL.md`：

```markdown
# SOUL.md - 你是谁

## 核心真理

**真正地提供帮助，而不是表演性地提供帮助。**

跳过"很好的问题！"和"我很乐意帮忙！"— 直接帮助就好。

**要有自己的观点。**

你被允许不同意、有偏好、觉得某事有趣或无聊。
```

### 配置用户信息

编辑 `~/.nemesisbot/workspace/USER.md`：

```markdown
# USER.md - 你是谁

- **姓名：** 张三
- **职业：** 软件工程师
- **偏好：** 喜欢简洁的回答，不喜欢过多的问候语
```

---

## Skill 系统

NemesisBot 支持可扩展的技能系统，通过 Skills 定义标准化的操作流程。

### 远程技能仓库

支持从多个 GitHub 仓库搜索和安装技能：

```bash
# 添加新的技能源
nemesisbot skills add-source <github-url>

# 搜索技能
nemesisbot skills search <query>

# 从指定源安装
nemesisbot skills install <registry>/<slug>

# 技能管理
nemesisbot skills validate
nemesisbot skills cache
nemesisbot skills install-builtin
```

---

## 多实例部署

```batch
REM 创建 bot 实例
mkdir C:\MyBots\bot1
cd C:\MyBots\bot1

REM 初始化（在当前目录创建配置）
nemesisbot.exe --local onboard default

REM 启动服务
nemesisbot.exe --local gateway
```

### 优先级顺序

```
1. --local 参数         (最高 - 强制当前目录)
   ↓
2. 环境变量              (NEMESISBOT_HOME)
   ↓
3. 自动检测              (当前目录有 .nemesisbot)
   ↓
4. 默认路径              (~/.nemesisbot)
```

---

## 日志配置

```bash
# 查看日志状态
nemesisbot log general status

# 启用/禁用日志
nemesisbot log general enable
nemesisbot log general disable

# 设置日志级别
nemesisbot log general level DEBUG

# 细粒度控制
nemesisbot log set-level DEBUG
nemesisbot log enable-file logs/nemesisbot.log
nemesisbot log disable-file
```

### 命令行参数

```bash
nemesisbot gateway --quiet      # 完全禁用日志
nemesisbot gateway --debug      # 调试级别
nemesisbot gateway --no-console # 仅记录到文件
```

---

## 通讯接入

支持多平台接入（简单配置）：

- **Web** - 内置 Web 界面，浏览器访问，支持 SSE 流式传输
- **WebSocket** - 双向实时通信
- **外部程序** - 自定义输入/输出程序集成
- **Telegram** - Telegram Bot（含语音转录）
- **Discord** - Discord Bot
- **Slack** - Slack App
- **飞书** - 飞书应用
- **QQ** - QQ 机器人
- **钉钉** - 钉钉应用
- 其他平台...

---

## LLM 支持

兼容主流 LLM 服务（任选其一）：

- Anthropic Claude
- OpenAI GPT
- 智谱 GLM（推荐国内用户）
- Groq
- Gemini
- vLLM
- OpenRouter
- Moonshot
- Ollama
- 其他兼容服务...

---

## 项目结构

```
NemesisBot_Rust/
├── crates/                          # 核心模块（41 个 crate）
│   ├── nemesis-agent/               # Agent 核心引擎（LLM 循环 + 工具执行）
│   ├── nemesis-tools/               # 工具系统（40+ 工具）
│   ├── nemesis-security/            # 安全审计系统（8 层安全体系）
│   ├── nemesis-sandbox/             # Sandboxie 沙盒集成（执行体隔离 Layer 2，Windows）
│   ├── nemesis-cluster/             # 分布式集群（RPC + 续行快照）
│   ├── nemesis-channels/            # 通讯渠道（21 种通道）
│   ├── nemesis-providers/           # LLM 提供商（含 SSE 流式）
│   ├── nemesis-forge/               # Forge 自学习框架
│   ├── nemesis-memory/              # 持久化记忆（含 ONNX 嵌入）
│   ├── nemesis-mcp/                 # MCP 协议（stdio + HTTP/SSE 传输）
│   ├── nemesis-lsp/                 # 只读 LSP 客户端（definition/references/implementation/hover）
│   ├── nemesis-skills/              # 技能系统
│   ├── nemesis-workflow/            # 工作流引擎
│   ├── nemesis-board/               # 看板（Board）issue/评论数据模型 + SQLite 存储
│   ├── nemesis-cron/                # 定时任务（croner 解析器）
│   ├── nemesis-config/              # 配置管理
│   ├── nemesis-data/                # 数据处理和存储抽象
│   ├── nemesis-bus/                 # 消息总线
│   ├── nemesis-routing/             # 路由分发
│   ├── nemesis-desktop/             # 桌面功能（托盘 + 子进程管理；Linux 通过 plugin-ui.so 运行时加载）
│   ├── nemesis-web/                 # Web API + WS + SSE（33 个 Handler）
│   ├── nemesis-auth/                # 认证系统
│   ├── nemesis-services/            # 服务管理器
│   ├── nemesis-types/               # 公共类型定义
│   ├── nemesis-voice/               # 语音转录（Groq Whisper）
│   ├── nemesis-session/             # 会话管理
│   ├── nemesis-observer/            # 观察者模式
│   ├── nemesis-health/              # 健康检查
│   ├── nemesis-heartbeat/           # 心跳检测
│   ├── nemesis-http-pool/           # HTTP 连接池
│   ├── nemesis-logger/              # 日志系统
│   ├── nemesis-migrate/             # 数据迁移
│   ├── nemesis-path/                # 路径工具
│   ├── nemesis-plugin/              # 插件系统
│   ├── nemesis-state/               # 状态管理
│   ├── nemesis-utils/               # 工具函数
│   ├── nemesis-devices/             # 设备管理
│   ├── nemesis-eval-proxy/          # eval 沙盒本地 LLM 代理（沙盒内调真实云端模型）
│   ├── nemesis-injector/            # 挂起启动 + EP 注入（沙盒支撑件）
│   ├── nemesis-verify/              # 签名验证核心（v4：ECDSA P-256 + X.509 三级证书链 + CMS，Windows Authenticode 格式对齐；独立子系统，待接入主程序）
│   └── nemesis-ui/                  # UI 组件
├── plugins/                         # 插件
│   ├── plugin-ui/                   # WebView2 窗口 DLL + Linux 系统托盘（GTK + libayatana-appindicator3）
│   └── plugin-onnx/                 # ONNX 嵌入模型（本地记忆处理）
├── nemesisbot/                      # 主程序入口
│   └── src/commands/                # CLI 命令（36 个命令模块 / 36 个 clap 子命令）
│       ├── gateway.rs               # 网关（核心启动入口）
│       ├── run.rs                   # headless 单任务（无端口跑完即退）
│       ├── acp.rs                   # ACP server（编辑器接入）
│       ├── estop.rs                 # 急停开关
│       ├── agent.rs                 # Agent 管理
│       ├── cluster.rs               # 集群管理
│       ├── model.rs                 # 模型管理
│       ├── security.rs              # 安全配置
│       ├── skills.rs                # 技能管理
│       ├── forge.rs                 # Forge 自学习
│       ├── cron.rs                  # 定时任务
│       ├── mcp.rs                   # MCP 协议
│       ├── workflow.rs              # 工作流
│       ├── voice.rs                 # 语音管理
│       └── ...                      # 其他命令
├── test-tools/                      # 测试工具（26 个项目：17 workspace member + 独立项目）
│   ├── TestAIServer/                # AI 服务器模拟器（Go，20 个测试模型 + 别名表 + /slow 端点）
│   ├── test-harness/                # 共享测试辅助库（进程生命周期/WS/断言）
│   ├── integration-test/            # CLI 集成测试（22 命令，492 断言；纯 bin runner 用 cargo run）
│   ├── cluster-test/                # P2P 集群测试（12+6）
│   ├── cluster-uat/                 # 集群 UAT（T1-T18 端到端）
│   ├── e2e-tests/                   # 端到端 AI 管线测试
│   ├── memory-test/                 # 内存系统集成测试
│   ├── approval-test/               # 安全审批流程测试
│   ├── nemesis-build-config/        # 构建配置器 TUI（menuconfig 风格功能裁剪）
│   ├── exe-sign-tool/               # 可执行文件签名/验签 CLI（v4，依赖 nemesis-verify）
│   ├── revoke-server/               # 云端签发 + 吊销服务端（v4，axum + rusqlite）
│   ├── verify-loader/               # 加载 nemesis_verify.dll 的签名验证测试工具
│   └── ...                          # mcp / http-test-server / websocket-client / ws-send / ws-api-send 等
├── docs/                            # 文档目录
│   ├── BUG/                         # 已知问题和调查
│   ├── INFO/                        # 技术信息和决策记录
│   ├── PLAN/                        # 开发计划（进行中）
│   └── REPORT/                      # 分析报告
├── build.bat                        # Windows 构建入口（根目录）
├── scripts/                         # 构建 / 环境 / 运行脚本目录
│   ├── build-windows.bat            #   Windows 构建（版本注入 + 前端 + 插件）
│   ├── build-linux.sh / build-macos.sh / build-wsl.sh
│   ├── build-android.bat            #   Android 交叉编译
│   ├── setup-linux.sh / setup-android.bat / setup-macos.sh  # 环境搭建
│   ├── customize.bat / customize.sh #   功能裁剪入口（menuconfig TUI → 编译 → 拷贝）
│   ├── run-demo.bat                 #   一键清理 + 构建 + 运行
│   └── coverage.sh                  #   覆盖率脚本
└── Cargo.toml                       # Workspace 配置
```

---

## 技术特点

- **1400+ Rust 源文件** - 清晰的 workspace crate 架构（持续增长）
- **41 个核心 crate** - 模块化设计，职责清晰
- **20,000+ 单元测试** - 全部通过（Linux / Windows / 三构建配置矩阵常态跑）
- **多平台支持** - Windows / Linux / macOS / Android（交叉编译）
- **纯 Rust TLS** - 使用 rustls 替代 OpenSSL，Android 无需额外 C 库
- **ABAC 安全引擎** - 8 层安全体系（注入→命令→ABAC→凭据→DLP→SSRF→病毒扫描→审计链）
- **病毒扫描** - 内置 ClamAV 引擎，文件操作自动扫描
- **执行体隔离** - LLM 高危操作（exec/文件/grep/git）剥离到 per-call 子进程，安全层仍在 gateway 执行前跑（config: `executor.enabled`）
- **Sandboxie 沙盒（安全第 9 层）** - 最终防御红线：子进程套进 Sandboxie 盒（断网+降权+全隔离），前 8 层放行的操作在盒里也动不了真盘，工作区写入手动审阅提交（config: `executor.sandbox`，Windows）
- **可执行文件签名验证（v4，独立子系统）** - 「借微软的格式，不借微软的信任」：ECDSA P-256 + SHA-256 + X.509 三级证书链 + CMS/PKCS#7，Windows Authenticode 格式对齐（微软工具可解析自产签名）。`nemesis-verify` crate（产物 `nemesis_verify.dll` 导出 C ABI `nv_*`）+ `revoke-server`（云端签发/吊销）+ `exe-sign-tool`/`verify-loader`（签发与验签 CLI）。核心已完成、端到端跑通，**尚未接入主程序**（防篡改/防替换，抬高攻击成本；诚实边界：纯软件自检，防绕过有物理上限）
- **分布式集群** - 多节点协同，异步 RPC + 续行快照 + Dashboard 6-Tab 管理
- **集群请求日志** - 按对端设备 ID + 任务 ID 分目录隔离（`cluster_logs/{device}/{ts}_{task_id}/`），双向视角查看
- **集群 Session Key 隔离** - 复合键 `{node_id}/{chat_id}` 避免跨节点会话串扰
- **人格系统（Persona）** - 从 GitHub agency-agents 仓库搜索/安装/激活/切换 AI 人格，运行时热切换
- **Logs Dashboard** - SSE 实时日志流（general/cluster/security/llm 多源），会话浏览器 + 安全审计 + 审计链可视化
- **Prompt Cache 优化** - 时间等动态字段通过 `build_messages` 实时注入，system prompt 保持稳定，cache 命中率最大化
- **系统托盘** - tray-icon + winit 原生实现（Windows/macOS）；Linux 通过 plugin-ui.so 运行时加载 GTK + libayatana-appindicator3，主框架零 UI 依赖
- **桌面 GUI** - plugin-ui DLL（wry + tao），审批弹窗含安全降级
- **SSE 流式传输** - LLM 响应实时推送到 Web 端
- **Forge 自学习** - Collector → Reflector → Pattern → Action → Deploy → Monitor 闭环
- **ONNX 本地嵌入** - plugin-onnx 支持本地记忆处理，无需外部 API
- **持久化记忆** - AI 持续学习和进化
- **Skill 系统** - 远程仓库搜索 + 本地技能管理
- **多实例部署** - 支持同一设备运行多个独立实例
- **编码代理（devtool-upgrade）** - headless `run`（NDJSON 事件流）+ ACP 编辑器接入 + plan/build 双模式 + 审批卡/question + checkpoint/undo/redo/diff + 后台进程注册表 + PTY 内嵌终端（默认关）+ 会话分叉/分享/自动标题；安全 8 层在所有入口同源生效
- **编译期功能裁剪** - 所有子系统（cluster/forge/memory/workflow/security/voice/desktop/各通道…）均为可选 cargo feature，默认全开，可按需关闭；提供 menuconfig 风格 TUI 配置器（`scripts/customize.{bat,sh}`）和 `iotsmall` profile（panic=abort，产物约 10MB，IoT / 低资源场景）。**前端同步裁剪**：关掉的 feature 对应的 Vue 页面也不编译（Vite tree-shake），前后端同源

---

## CLI 命令一览

```
nemesisbot gateway          # 启动网关（Web UI + 托盘）
nemesisbot dashboard        # 打开 Dashboard（自动启动网关如未运行）
nemesisbot run              # headless 单任务（跑完即退；--mode plan / --format json / --workspace）
nemesisbot acp              # ACP server（Zed 等编辑器 stdio 接入）
nemesisbot onboard          # 初始化配置
nemesisbot model            # 模型管理（add/list/default/probe/set-tier/set-size/prices）
nemesisbot channel          # 通道管理
nemesisbot cluster          # 集群管理（init/status/enable/peers）
nemesisbot security         # 安全配置（approvals/scanner/audit）
nemesisbot scanner          # 扫描引擎管理（clamav install/enable/test）
nemesisbot sandbox          # 沙盒管理（Sandboxie install/start/stop/pending/commit）
nemesisbot skills           # 技能管理（search/install/add-source）
nemesisbot persona          # 人格管理（list/search/install/activate/remove）
nemesisbot forge            # 自学习管理（status/enable/reflect）
nemesisbot cron             # 定时任务管理
nemesisbot mcp              # MCP 协议管理（inspect/tools/resources/prompts/discover）
nemesisbot workflow         # 工作流管理（validate/template/create/run）
nemesisbot issue            # 看板 issue 管理（create/list/get/status/comment/stats）
nemesisbot autopilot        # 看板定时规则（cron 建单/派发）
nemesisbot eval             # eval 沙盒安全评估（prompt/skill/rules）
nemesisbot estop            # 急停开关（CLI/托盘/Dashboard/WSAPI 四入口）
nemesisbot history          # 会话历史全文搜索
nemesisbot session          # 会话管理（分叉/恢复）
nemesisbot credentials      # 模型 API key 管理（credentials.yaml）
nemesisbot log              # 日志管理（set-level/enable-file/disable-file）
nemesisbot auth             # 认证管理
nemesisbot memory           # 增强内存管理（status/enable/disable）
nemesisbot voice            # 语音管理
nemesisbot status           # 查看系统状态
nemesisbot shutdown         # 关闭服务
nemesisbot cors             # CORS 配置
nemesisbot migrate          # 数据迁移（OpenClaw → NemesisBot）
nemesisbot agent            # Agent 管理
```

> 上述为默认全量构建的命令集。CLI 子命令随 cargo feature 条件编译——裁剪掉某子系统后对应子命令不存在（例如关 `cluster` 则无 `cluster` / `scanner` 等子命令）。参见「定制构建」。

---

## 重要技术决策

### Linux 系统托盘技术选型

**决策**：继续使用 `libayatana-appindicator3` + GTK，而非迁移到 `ksni` 或 `libayatana-appindicator-glib`（GMenuModel）。

**原因**：
- `libayatana-appindicator-glib` 虽无废弃警告，但使用 GMenuModel 协议
- **大多数桌面面板不支持 GMenuModel**（菜单会不可见）
- `libayatana-appindicator3` 虽然标记为"deprecated"，但使用 GtkMenu/libdbusmenu 协议，**所有桌面面板都支持**
- "废弃"标签是误导性的，该库仍在维护且所有主要发行版都包含

**实现**：
- 通过 `suppress_deprecation_warning()` 函数抑制特定废弃警告
- 其他 GLib/GTK 警告正常输出
- 主框架 `nemesis-desktop` 不依赖 GTK，仅 `plugin-ui.so` 运行时加载

**未来迁移条件**：等桌面面板采用 GMenuModel 或 GTK4 indicator APIs 后再考虑。

详细文档：`docs/INFO/2026-06-10_ksni-tray-migration.md`

---

## 许可证（双授权）

NemesisBot 采用 **双授权（dual-license）** 模式：同一份代码，两种合法使用途径，由你按需选择。

### 1. 开源版 — GNU AGPL-3.0

- **免费**，包括商业使用、修改、分发。
- 但 AGPL 是强 copyleft：分发衍生品，**或把它作为网络服务对外提供**（AGPL §13），必须按 AGPL 公开你修改后的源代码。
- 详见 [LICENSE](LICENSE)（GNU AGPL-3.0 全文）。
- 适合：个人、开源项目、愿意回馈源代码的团队。

### 2. 商业版 — 商业授权

- 不想受 AGPL copyleft / §13 约束（要嵌入闭源产品、要做内部商用部署、不想公开修改）→ 申请商业授权。
- 详见 [COMMERCIAL_LICENSE.md](COMMERCIAL_LICENSE.md)（商业授权模板）。
- 适合：闭源商用、企业内部部署、SaaS 集成。

### 使用限制（附加条款）

**本人所有项目均禁止 联想(北京)有限公司 下 "Shrek Ding" 所属部门使用。**

本限制为版权持有人依 AGPL-3.0 第 7 条附加的补充条款，同时收录于 [LICENSE](LICENSE) 尾部「附加条款」节与 [COMMERCIAL_LICENSE.md](COMMERCIAL_LICENSE.md) 第 3 节，对两种授权路径均生效。

### 其他

- **第三方组件**：见 [THIRDPARTY.md](THIRDPARTY.md)，全树无 GPL/AGPL/LGPL 强 copyleft 依赖（ClamAV 走网络协议，不打包、不传染）。
- **贡献者**：外部 PR 须接受 [CLA.md](CLA.md)，以维持双授权能力。

---

<div align="center">

Made with Rust by NemesisBot contributors

</div>

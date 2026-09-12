# AGENTS.md

本文件为 AI 编码助手在此代码库中工作时提供指导。

---

## 关键警告

**Windows 平台后台进程管理**：
- **严格禁止**使用 `start /B`、`cmd /c start`、`start` 等命令
- **严格禁止**使用任何会弹窗或创建新窗口的命令
- 使用 PowerShell `Start-Process -WindowStyle Hidden`
- 使用 Bash 后台符号 `&` 配合 `run_in_background: true` 参数
- 优先使用项目辅助脚本（setup-env/cleanup-env）

**原因**：`start /B` 等命令会弹窗报错，如果没有人工干预会导致流程永久卡住。

**Git 工作流纪律（重要）**：
- **不要随意创建新分支**：除非用户明确要求，不主动 `git checkout -b` / `git branch` 建分支。
- **不要随意提交代码**：除非用户明确要求，不主动 `git commit` / `git push`；改完代码默认留在工作区，等用户决定是否提交。
- **改变仓库状态的操作先问**：建分支、提交、推送、删分支、强推、`reset --hard` 等操作，若非用户明确指示，**先向用户询问再执行**。
- 用户明确指示（如“提交”、“合并到 main”、“删掉这个分支”）时按指示执行，不算“随意”。

---

## 构建和测试命令

### 构建项目

```bash
# 标准构建（Release）
cargo build --release -p nemesisbot

# 开发构建（更快编译，用于调试）
cargo build -p nemesisbot

# 使用构建脚本（Windows，带版本注入 + 插件编译）
scripts\build-windows.bat
scripts\build-windows.bat --clean          # 清理后构建
scripts\build-windows.bat --skip-plugin    # 跳过 plugin-ui.dll 编译

# 使用构建脚本（Linux/macOS）
scripts/build-linux.sh

# 使用构建脚本（Android 交叉编译）
scripts\build-android.bat                  # 默认 arm64-v8a, API 36
scripts\build-android.bat --clean          # 清理后构建
scripts\build-android.bat --target x86_64  # 指定架构
scripts\build-android.bat --api 33         # 指定 API level
```

构建脚本会自动：
- 从 git tag 提取版本号，如果没有则使用 `0.0.0.1`
- 提取 git commit hash
- 通过环境变量注入版本信息
- 构建 Vue 前端（`web/` 目录，输出到 `crates/nemesis-web/static/`）
- 编译 plugin-ui.dll（除非 `--skip-plugin`）
- 输出到对应目录：
  - Windows：`bin/bin_windows/` 目录
  - Linux：`bin/bin_linux/` 目录
  - Android：`bin/bin_android/<arch>/` 目录
- 检测 node_modules 平台不匹配（WSL 编译后回到 Windows 时自动重装 npm 依赖）

### 快速构建并运行

```bash
# 一键清理 + 构建 + 运行（Windows）
scripts\run-demo.bat
```

### Android 交叉编译环境搭建

```bash
# 1. 检测/安装 Android 依赖（NDK、SDK、Rust targets、cargo-ndk）
scripts\setup-android.bat              # 自动检测 + 安装
scripts\setup-android.bat --dry-run    # 仅检测，不安装

# 2. 编译 Android 版本
scripts\build-android.bat              # 默认 arm64-v8a, API 36
scripts\build-android.bat --clean
scripts\build-android.bat --target armeabi-v7a
scripts\build-android.bat --api 33

# 3. 手动安装前提（如果 setup-android.bat 不够用）
cargo install cargo-ndk
rustup target add aarch64-linux-android
# 设置环境变量：ANDROID_NDK_HOME 指向 NDK 路径
```

Android 交叉编译注意事项：
- 使用 `reqwest` 的 `rustls-tls` feature（不依赖 OpenSSL）
- `nemesis-desktop` 中的系统托盘功能通过 `#[cfg(not(target_os = "android"))]` 排除
- API level 通过 `CARGO_NDK_PLATFORM` 环境变量控制（build-android.bat 自动设置）
- 中间产物输出到 `target/target_android/`，最终产物输出到 `bin/bin_android/<arch>/`

### Linux 环境搭建

```bash
# 检测/安装 Linux 依赖（OpenSSL、Rust、Node.js）
scripts/setup-linux.sh              # 自动检测 + 安装
scripts/setup-linux.sh --dry-run    # 仅检测，不安装
```

注意：Linux 下系统托盘功能通过 `plugin-ui.so` 运行时加载（需要 GTK3），使用 `libayatana-appindicator3` + GtkMenu/libdbusmenu 协议。已选择继续使用 libayatana-appindicator3（而非 GMenuModel），因为大多数桌面面板不支持 GMenuModel（菜单会不可见）。废弃警告已通过 GLib log filter 抑制。`nemesis-desktop` 本身不依赖 GTK/tray-icon/winit。

### 定制构建（功能裁剪 / menuconfig）

NemesisBot 支持编译期按功能裁剪，服务于 IoT / 低资源 / 定制发行版场景。所有可选子系统都是 cargo feature（默认全开 = 全量构建），关闭后整包不参与编译。**CLI 命令也随 feature 条件编译**（如关 `cluster` 则 `nemesisbot cluster` 子命令不存在）。

```bash
# 一键入口：TUI 勾选 → 保存 → 编译 → 拷贝（推荐）
scripts\customize.bat                    # Windows
bash scripts/customize.sh                # Linux/macOS/git-bash

# 模式化用法（customize.sh；.bat 同）
bash scripts/customize.sh config         # 只开 TUI 配置，不编译
bash scripts/customize.sh build          # 只编译（用已有 .config，无则全量默认）
bash scripts/customize.sh iot            # 加载 minimal-iot 预设 → 编译最小 IoT 版
bash scripts/customize.sh desktop        # 加载 desktop 预设 → 编译
bash scripts/customize.sh <preset>       # 加载任意预设 → 编译
```

关键组件：
- **TUI 配置器**：`nemesis-build-config`（ratatui，menuconfig 风格勾选），workspace member（独立 binary，不是 nemesisbot 的依赖）
- **feature 清单**：`scripts/customize/features.toml`（单一真相源，每个 `id` 必须对应 `nemesisbot/Cargo.toml` 的 `[features]` 条目）
- **前端同步裁剪**：customize 还按同一份 feature 选择裁 Vue 前端——`nemesis-build-config export --frontend-env` 从 `.config` 生成 `web/.env`（`VITE_FEATURE_<ID>=<bool>`），router/侧栏按 `import.meta.env.VITE_FEATURE_X !== 'false'` 门控，Vite tree-shake 掉关闭的页面（不进二进制）。全量构建清掉 .env（默认包含全部视图）。映射：cluster→ClusterView、forge→ForgeView、workflow→WorkflowView、memory→MemoryView、security→Security+ScannerView、sandbox→SandboxView、usage→UsageView（+echarts）
- **预设**：`scripts/customize/profiles/{minimal-iot,desktop}.config`
- **配置文件**：项目根 `.config`（仿 Linux 内核），存勾选状态 + build-profile
- **产物**：`bin/bin_customize/nemesisbot(.exe)`（与 `bin/bin_windows` 等一致）

可裁剪 feature（默认全开，用 `--no-default-features --features "..."` 收窄）：
- 子系统：`migrate`、`devices`、`desktop`、`health`、`heartbeat`、`auth`、`voice`、`cluster`、`forge`、`memory`、`workflow`、`security`（含 scanner）、`sandbox`（Sandboxie 集成，Windows）
- 前端闸（cargo no-op，纯裁 Vue 页面）：`usage`（使用统计页，带 echarts ~558KB；关掉则页面+echarts 从前端 tree-shake 掉）
- 通道（穿透到 `nemesis-channels`）：默认开 `channels-web`/`channels-webhook`/`channels-rpc`，其余（telegram/discord/slack/feishu/dingtalk/email/matrix/irc/signal/mastodon/bluesky/whatsapp/onebot/tencent/...）默认关

构建 profile：
- `release`（默认）：`panic=unwind`，保留系统托盘线程的 `catch_unwind` 优雅降级
- `iotsmall`：`panic=abort` + `opt-level="z"` + `lto=true` + `codegen-units=1`，产物约 10MB（无托盘 + Vue 前端按 feature 裁剪，IoT 用）。用 `cargo build --profile iotsmall` 直接触发

**裁剪完成度**：Class A 叶子（desktop 等）、Class B 双层（cluster/voice）、Class C 内核（forge/memory/workflow/security）四个 phase 全部完成，无过渡期 pin；外加**前端同步裁剪**（cargo feature → Vue 页面门控，Vite tree-shake）。详见 `docs/REPORT/2026-07-01_feature-trimming-and-build-configurator.md` + `docs/REPORT/2026-07-12_frontend-feature-gating.md`。

### 运行测试

```bash
# 运行某个 crate 的所有测试
cargo test -p nemesis-agent
cargo test -p nemesis-channels
cargo test -p nemesis-cluster

# 注意（feature-gated 测试）：nemesis-web 的多数测试 mod 有
# cfg(all(test, feature=...)) 守卫，单测该 crate 时带 feature 才能跑到全部测试：
#   cargo test -p nemesis-web --features "cluster,workflow,memory,security,forge,voice,sandbox"
# （voice/sandbox 不带的话 voice_extra_tests / sandbox 测试会静默跳过；不带 feature
#   只能跑基础 327 个；--workspace 全量时 feature 统一自动生效。）

# 运行 workspace 全部测试
cargo test --workspace

# 使用 release 模式运行测试（更快执行）
cargo test --workspace --release

# 运行单个测试
cargo test -p nemesis-agent test_function_name

# 运行特定 crate 的测试（详细输出）
cargo test -p nemesis-security -v

# 集成测试（需要先编译测试工具）
cargo test -p integration-test
cargo test -p cluster-test

# E2E 测试（需要先启动 TestAIServer）
cargo test -p nemesisbot-tests   # 注意：包名是 nemesisbot-tests（目录 test-tools/e2e-tests/）；[[test]] 目标 e2e_ai_flow / integration_pipeline / v_batch

# 内联测试防回归检查（2026-08-25 固化；写完任何新代码后跑一次）
bash scripts/check-inline-tests.sh   # 退出码 0=干净；1=生产文件存在内联测试
# 四形态：#[cfg(test)] mod X{body} / 裸 #[test] / 裸 #[tokio::test] / #[cfg(all(test,…))] mod X{body}
# 合法：`#[cfg(test)] mod tests;` 声明 + 独立测试文件（tests.rs / *_tests.rs / tests/ 目录 / [[test]] path 声明）
```

### 运行应用程序

```bash
# 启动网关（Web UI）
cargo run --release -p nemesisbot -- gateway
# 或使用编译后的二进制
nemesisbot.exe gateway

# 使用本地模式（配置在 ./.nemesisbot 而不是 ~/.nemesisbot）
nemesisbot.exe --local gateway

# 集群管理
nemesisbot.exe cluster status
nemesisbot.exe cluster init --name "机器人名称" --role worker --category development
nemesisbot.exe cluster enable

# 模型管理
nemesisbot.exe model add --model zhipu/glm-4.7 --key YOUR_KEY --default
# 模型能力分级（小模型支持，详见下方"模型能力分级"小节）
nemesisbot.exe model probe <name>                          # 7 题探针实测能力 → 写 tier
nemesisbot.exe model set-tier <name> auto|mini|normal|big  # 手动设档（覆盖自动检测）
nemesisbot.exe model set-size <name> 30B                   # 设参数量，auto 检测会用上
nemesisbot.exe model set-real-name <name> "Qwen3-30B"      # opaque 别名设真名

# Scanner 管理（病毒扫描）
nemesisbot.exe scanner list                          # 列出所有引擎
nemesisbot.exe scanner check                         # 详细状态检查（多列表格）
nemesisbot.exe scanner install                       # 批量安装所有 pending 引擎
nemesisbot.exe scanner add clamav --url <URL>        # 添加引擎
nemesisbot.exe scanner clamav install [--force] [--url URL] [--dir DIR]
nemesisbot.exe scanner clamav enable                 # 启用（需已安装）
nemesisbot.exe scanner clamav disable                # 禁用
nemesisbot.exe scanner clamav update                 # 更新病毒库
nemesisbot.exe scanner clamav test <path>            # 测试扫描
nemesisbot.exe scanner clamav info                   # 引擎详情

# 沙盒（Sandboxie 集成，Windows；执行体隔离 Layer 2）
nemesisbot.exe sandbox install                       # 下载+校验+解压 Sandboxie Classic（无 UAC，仅取文件）
nemesisbot.exe sandbox start                         # 激活引擎：装驱动+服务+写 ini+启动 SbieSvc（触发 UAC）
nemesisbot.exe sandbox status                        # 驱动/服务/Start.exe 就绪状态
nemesisbot.exe sandbox pending                       # 列出盒里待提交的工作区文件（只工作区子树）
nemesisbot.exe sandbox commit --all                  # 提交盒里工作区文件回真盘
nemesisbot.exe sandbox commit <needle>...            # 按真路径子串选择性提交
nemesisbot.exe sandbox clear [--force]               # 清空盒内容（丢弃待提交；--force 跳过询问）
nemesisbot.exe sandbox kill                          # 强杀所有 NemesisEvalBox_* 内进程并弃盒内容（引擎保持运行；eval 失控时用）
nemesisbot.exe sandbox kill --box-name NemesisBox    # 强杀指定盒（如 executor 主盒）
nemesisbot.exe sandbox stop [--purge]                # 停止+卸载引擎（--purge 连文件删）

# eval 沙盒安全评估（提示词/技能行为检测；规则驱动三分类：有风险/安全/未知）
nemesisbot.exe eval prompt "..."                     # 沙盒内跑 agent 评估提示词行为，自动出结论
nemesisbot.exe eval prompt "..." --fail-on-risk      # 结论=有风险 → 退码 2（脚本/CI 用）
nemesisbot.exe eval prompt "..." --observe-secs 300  # 观察时长（默认 1800s 硬熔断）
nemesisbot.exe eval skill <name>                     # 评估已安装技能（workspace→global→builtin 解析）
nemesisbot.exe eval rules new                        # 【推荐】交互式规则向导：回答几个问题即可，无需写 JSON
nemesisbot.exe eval rules list                       # 规则管理（跨平台）：列出/种子默认集（每条含匹配条件摘要）
nemesisbot.exe eval rules show/add/edit/remove/enable/disable/reset  # 规则增删改查（add/edit 用 --file 传 JSON）
# 产物：报告目录 <workspace>/logs/eval/<ts>_<kind>/（7 件套 + assessment.json）；规则文件 <workspace>/config/eval_rules.json（改即生效）

# 程序化集成（三方自动化执行）
# 退码：0=安全或未知（用 assessment.json 区分）；1=命令错误（环境/配置）；2=有风险（需 --fail-on-risk）
# 机器可读：<报告目录>/assessment.json 的 conclusion(risk/safe/unknown) + matched_rules[]（含 evidence 原文）
# 注意：必须串行执行（并发 eval 被互斥锁拒绝，退码 1）；只认退码/JSON，别解析 stdout（人读格式）
# CMD 示例：nemesisbot.exe eval prompt "%P%" --fail-on-risk >nul 2>&1 && echo PASSED || if %ERRORLEVEL%==2 (echo BLOCKED) else (echo EVAL_ERROR)

# 配置管理
nemesisbot.exe onboard default --local   # 使用默认配置，在当前目录初始化
nemesisbot.exe onboard default           # 使用默认配置根据默认流程初始化
nemesisbot.exe log config                # 配置日志详细级别

# 急停开关（Kill Switch）——一键冻结全部 agent 活动
# 四入口操作同一个 Arc<EstopState>：CLI / 托盘菜单 / Dashboard 按钮 / WSAPI
nemesisbot.exe estop                     # 触发急停（冻结 agent loop + 拒绝工具调用 + 中途打断 LLM；跨进程走 /api/internal）
nemesisbot.exe estop --release           # 释放急停，下一条消息起恢复
nemesisbot.exe estop --status            # 查询当前急停状态
# 托盘：右键菜单「⛔ 急停」/「✓ 释放急停」（Win/mac/Linux）
# Dashboard：侧栏底部急停按钮（WSAPI estop.trigger/release/status）
# 状态挂 SharedResources.estop，跨 agent 重启自动保持；见 docs/REPORT/2026-07-13_estop-kill-switch-phase0.md

# Forge 自学习
nemesisbot.exe forge status
nemesisbot.exe forge enable
nemesisbot.exe forge reflect
nemesisbot.exe forge list
nemesisbot.exe forge disable

# 增强内存
nemesisbot.exe memory status
nemesisbot.exe memory enable
nemesisbot.exe memory disable

# 工作流
nemesisbot.exe workflow list
nemesisbot.exe workflow template <name>
nemesisbot.exe workflow run <name>

# 数据迁移（OpenClaw → NemesisBot）
nemesisbot.exe migrate --dry-run         # 预览迁移内容
nemesisbot.exe migrate                   # 执行迁移
nemesisbot.exe migrate --config-only     # 仅迁移配置
nemesisbot.exe migrate --workspace-only  # 仅迁移工作空间
```

---

## 测试工具

位置：`test-tools/`

所有 Rust 测试工具都是 workspace members（`TestAIServer` 为 Go 项目，`mcp`/`examples`/`plugin-onnx-test` 为独立项目），可用 `cargo build/test -p <name>` 操作 workspace 内的工具。

### TestAIServer（Go AI 服务器模拟器）

`test-tools/TestAIServer/` — **Go 项目**，提供 8 个硬编码测试模型。

```bash
# 构建（需要 Go 环境）
cd test-tools/TestAIServer && go build -o testaiserver.exe

# 启动（默认端口 8080）
./testaiserver.exe

# 配合 nemesisbot 使用
nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default
```

测试模型及用途：
- `testai-1.1`：基础对话，固定回复
- `testai-1.2`：流式回复测试
- `testai-1.3`：延迟模拟（0s/30s/300s）
- `testai-2.0`：工具调用测试
- `testai-3.0`：多轮对话
- `testai-4.2/4.3`：复杂场景
- `testai-5.0`：文件操作安全测试（`<FILE_OP>` 标签）

### test-harness（共享测试辅助库）

`test-tools/test-harness/` — 被其他测试工具依赖的公共库。

提供的能力：
- 隔离的临时工作空间管理
- AI Server 和 Gateway 进程生命周期管理（启动/停止/健康检查）
- WebSocket 客户端封装（消息协议支持）
- CLI 命令执行和输出捕获
- HTTP 健康检查轮询
- 断言辅助工具

关键常量：AI_SERVER_PORT=8080, WEB_PORT=49000, HEALTH_PORT=18790

### integration-test（CLI 集成测试）

`test-tools/integration-test/` — 298 个断言，覆盖 22 个命令约 180 个子命令。

```bash
cargo test -p integration-test
```

### cluster-test（P2P 集群测试）

`test-tools/cluster-test/` — 参照 Go 版本的 12+6 个集群测试。

```bash
cargo test -p cluster-test
```

### cluster-uat（集群用户验收测试）

`test-tools/cluster-uat/` — 集群功能 UAT 测试，包含 workdir 工作空间。14 个端到端测试（T1-T14），含 session_log 持久化回归（T14 用 PEER_CHAT marker 验证 cluster_continuation 续行回复写入 session_log）。

```bash
cargo test -p cluster-uat
```

### e2e-tests（端到端 AI 管线测试）

`test-tools/e2e-tests/` — 完整 AI 管线测试（消息 → Agent → LLM → 工具 → 响应）。需要 TestAIServer 运行在 18080 端口。

```bash
# 先启动 AI Server
./testaiserver.exe --port 18080

# 运行测试
cargo test -p nemesisbot-tests   # 注意：包名是 nemesisbot-tests（目录 test-tools/e2e-tests/）；[[test]] 目标 e2e_ai_flow / integration_pipeline / v_batch
```

### http-test-server（HTTP 测试服务器）

`test-tools/http-test-server/` — 基于 axum 的 HTTP 服务器，模拟 OAuth、Webhook、Channel 交互。

### websocket-client（WebSocket 客户端）

`test-tools/websocket-client/` — 连接到 `ws://127.0.0.1:49001/ws`，测试 Bot 的 WebSocket 功能。

### ws-send（WebSocket 发送工具）

`test-tools/ws-send/` — 轻量 WebSocket 消息发送工具。

### mcp（MCP 测试）

`test-tools/mcp/` — MCP 协议测试。

### bus-benchmarks（消息总线性能测试）

`test-tools/bus-benchmarks/` — 消息总线性能基准测试。

### approval-test（审批流程测试）

`test-tools/approval-test/` — 安全审批流程测试。

### examples（外部通道示例）

`test-tools/examples/external/` — External channel 输入输出示例（input.bat/py、output.bat/py）。

### memory-test（内存系统测试）

`test-tools/memory-test/` — 内存系统（向量存储、嵌入管线）集成测试。

```bash
cargo test -p memory-test
```

### plugin-onnx-test（ONNX 插件测试）

`test-tools/plugin-onnx-test/` — ONNX 插件 E2E 测试（需要 plugin_onnx.dll + 模型文件）。

### nemesis-build-config（构建配置器 / 功能裁剪 TUI）

`test-tools/nemesis-build-config/` — menuconfig 风格的构建配置器（ratatui + crossterm），**workspace member**（独立 binary，不进 nemesisbot 主二进制）。

- 读取 `scripts/customize/features.toml`（feature 清单，单一真相源）
- 勾选状态写入项目根 `.config`（仿 Linux 内核）
- 支持加载预设（`scripts/customize/profiles/{minimal-iot,desktop}.config`）
- `export --features` / `export --profile` 供 `scripts/customize.{bat,sh}` 拼接 cargo 命令
- `export --frontend-env` 生成 `web/.env`（`VITE_FEATURE_<ID>=<bool>`）驱动前端同步裁剪
- 详见上方「定制构建（功能裁剪 / menuconfig）」小节 + `docs/REPORT/2026-07-01_feature-trimming-and-build-configurator.md` + `docs/REPORT/2026-07-12_frontend-feature-gating.md`

### nemesis-verify（签名验证核心 crate，v4 = Windows Authenticode 格式对齐）

`crates/nemesis-verify/` — 签名验证体系核心（**v4**：「借微软的格式，不借微软的信任」——ECDSA P-256 + SHA-256（RFC 6979）+ X.509 v3 三级证书链 + CMS/PKCS#7 SignedData（SPC_INDIRECT_DATA）+ PE Certificate Table）。**workspace member**（`lib` 供 Rust 依赖 + `cdylib` 产物 `nemesis_verify.dll`/`libnemesis_verify.so`/`.dylib`，C ABI 导出 `nv_*`）。**当前尚未接入 nemesisbot 主程序**——独立子系统。微软工具（Explorer「数字签名」选项卡 / signtool / Get-AuthenticodeSignature）可解析自产签名；默认态（零安装）微软验证失败且**唯一失败 = CERT_E_UNTRUSTEDROOT**（D7 失败面质量线，`scripts/test-sig-win-trust.sh` 矩阵化）；opt-in = 用户显式装自签根（per-user，弹框确认）→ 微软全绿；**自方 verifier 锚定编译期根，两态恒 Valid**。v3 NMBSIG envelope 已整体退役（S5-3）——v3 签名文件在 v4 管线 = NoSignature。

- 模块：`codec`（PE/ELF/Raw 多态 + L 上界）/ `pe`（authenticode byte-range digest：排除 CheckSum 字段 + Security Directory 表项/证书数据；Certificate Table 读写，已签名 PE 二次追表诚实拒绝——多签名走 CMS 嵌套 SPC_NESTED_SIGNATURE）/ `elf` / `envelope`（CMS SignedData 构造/解析 + ELF/raw v4 footer 载体 magic `NMBSIG\x04\x00` + 嵌套签名枚举）/ `crypto`（P-256 keypair/sign/verify + key_fp=SHA-256(65B SEC1)，RFC 6979 确定性）/ `cert`（X.509 解析 + AKI/SKI 链排序 + 有效期 + codeSigning EKU + 根指纹锚定）/ `keygen`（root 自签 30y → 发行锚 CA → leaf codeSigning EKU 三级生成 + `keys.json` v2 hex DER 含 root_cert 公钥部分）/ `revocation`（CRL 四维 + OCSP 单条 fallback + strict/soft-fail）/ `verify`（九态管线 + **`sign_content_v4` 签发单一入口**（PE→证书表/ELF+raw→footer 自动分派）+ `v4_content_digest` 摘要单一真相源（签发与吊销记账同源））/ `view`（离线查看签名 + 证书链，不下结论）/ `c_abi`（`nv_*` 接口）
- `VerifyOutcome` 九态：`Valid`/`NoSignature`/`Tampered`/`SignatureInvalid`/`UnsupportedVersion`/`Malformed`/`Untrusted`/`Revoked`/`Expired`（**只验证、处置策略由调用方决定**）
- 吊销四维度（`RevDim`）：`key_fp`（密钥级）/ `sig_hash`（签名级）/ `file_hash`（文件级）/ `publisher`（发布者级）
- C ABI：`nv_verify_target` / `nv_verify_current_exe`（DLL 自验入口）/ `nv_self_verify`；根锚 = 根证书 SHA-256 指纹，编译期固化（build 时 `NEMESIS_BUILD_ROOT_ANCHOR` 注入）优先，运行时 `NEMESIS_ROOT_ANCHOR` fallback（v3 的 `NEMESIS_BUILD_ROOT_PUBKEY`/`NEMESIS_ROOT_PUBKEY` 已作废）；两者皆缺 = 拒绝装配
- e2e 脚本：`scripts/test-sig-e2e.sh`（自方全链路 12 步：签验/篡改/服务端签发/view 链/吊销/OCSP/固化/二次追表诚实拒绝/DLL 自验）+ `scripts/test-sig-win-trust.sh`（微软工具默认态矩阵：signtool 唯一错误 UNTRUSTEDROOT / Get-AuthenticodeSignature UnknownError+0109 / 篡改 HashMismatch / 自方 Valid）
- 状态：v4 全量完成（P0-P6，`docs/REPORT/2026-09-12_authenticode-v4-goal.md` 总控）；后续 = 接入 nemesisbot / 根密钥物理保护 / DLL 自身防 patch。**诚实边界**：防绕过有纯软件物理上限（无内核强制 + Secure Boot）；D1 密码学（ECDSA P-256 + SHA-256 + X.509/CMS）与 Authenticode 平级。
- 详见 `docs/REPORT/2026-09-12_authenticode-v4-goal.md`（v4 总控 + 开发记录）+ `docs/PLAN/2026-07-20_signature-strength-hardening.md`（v2/v3 历史架构）

### exe-sign-tool（可执行文件签名、验证 CLI）

`test-tools/exe-sign-tool/` — PE/ELF/Raw 可执行文件签名/验签 CLI（**v4**：ECDSA P-256 + X.509 三级证书链 + CMS）。**workspace member**（纯 bin，lib 直接验签——依赖 `nemesis-verify`，不加载 DLL）。

- 子命令：`keygen --out <keys.json>`（生成 root/发行锚/leaf 私钥 + X.509 链，打印 root anchor） / `sign --keys <keys.json> --target <F> [--out <F>]`（leaf 签，PE→证书表 / ELF/raw→v4 footer；**缺省 `--out` = `{target}.signed` 新文件非原地**） / `verify --keys <keys.json> --target <F> [--revocation-url <URL>]`（lib 直接验签；`--revocation-url` 是 **base URL**，客户端自拼 `GET {base}/v1/crl`）
- v4 协议：CMS 明文 body + 证书集含全部三级证书（含根——缺根微软报 CHAINING 非 UNTRUSTEDROOT）；v3 → v4 是**破坏性升级**，旧签名全部作废（v3 文件 → NoSignature）。
- 详见 `docs/REPORT/2026-09-12_authenticode-v4-goal.md` + `docs/REPORT/2026-07-18_exe-self-signature_最终报告.md`

### revoke-server（云端签发 + 吊销服务端）

`test-tools/revoke-server/` — v4 架构的**云端签发 + 吊销服务端**（axum + rusqlite）。**workspace member**。扮演 Authenticode 里的 CA + CRL/OCSP 分发点。

- 启动：`revoke-server --keys-file keys.json [--init-keys] [--bind 127.0.0.1:7878] [--db-url revoke.db] [--admin-token ...]`；首次 `--init-keys` 生成密钥体系（v2 keys.json）到 `--keys-file` 并打印 root anchor（**不退出**，继续 bind+serve——后台启动读输出后 kill），后续 `--keys-file` 加载
- API：`POST /v1/verify`（验签）| `GET /v1/crl`（吊销列表）| `GET /v1/trusted-keys` | `POST /v1/sign`（X.509+CMS 签发，记账三值同源 `v4_content_digest`/key_fp/`latest_sig_hash`）| `POST /v1/admin/revoke`（四维度吊销）| `POST /v1/admin/trusted-key` | `POST /v1/admin/user` | `GET /v1/audit` | `GET /v1/signatures` | `GET /v1/health`
- Web UI：嵌入式单页（`web/index.html` + `web/admin.html`，登录 + CRL/吊销/trusted-keys/审计）
- 详见 `docs/REPORT/2026-09-12_authenticode-v4-goal.md` + `docs/PLAN/2026-07-20_cloud-signing-service.md`

### verify-loader（DLL 签名验证测试工具）

`test-tools/verify-loader/` — 加载 `nemesis_verify` DLL 验证目标文件的测试工具（验证 C ABI `nv_*` 四通路）。**workspace member**。

- 子命令：`gen-keys <out>`（生成密钥体系，打印 root anchor + 落 `root_anchor.hex` 到 cwd） / `sign <keys> <target> <out>`（v4 签，PE→证书表 / ELF/raw→footer） / `verify [--keys] <dll> <target>`（调 `nv_verify_target`，Valid 时打印 signed_at/key_fp/pubkey） / `verify-self [--keys] <dll>`（调 `nv_verify_current_exe` 验**本进程 exe**——验签名副本须先 `sign` 出副本再运行它） / `view <dll> <target>`（列签名数 + 信任链，离线不下结论；**输出无 key_fp=**，key_fp 只在 verify 输出） / `verify-dll <dll>`（调 `nv_self_verify` 验 DLL 自身，防替换）
- `--keys` 自动注入根锚（设 `NEMESIS_ROOT_ANCHOR` = 根证书 SHA-256 指纹 hex，DLL 内部读）；e2e 前显式 `cargo build -p` 各 bin（**cargo test 不重链 [[bin]] 目标**）

---

## Skills 系统

位置：`Skills/` 目录。每个 Skill 包含 `SKILL.md` 定义文件，AI 加载后严格遵循定义的流程。

### automated-testing（自动化测试）

`Skills/automated-testing/` — 完整的 E2E 自动化测试流程。

- **8 阶段**：预检 → 环境准备 → 本地初始化 → AI 配置 → 启动 Bot → 执行测试 → 清理 → 分析
- **辅助脚本**：`scripts/setup-env.sh`（Bash）、`scripts/setup-env.ps1`（PowerShell）
- **清理脚本**：`scripts/cleanup-env.sh`、`scripts/cleanup-env.ps1`
- 使用 TestAIServer 作为模拟 AI 后端
- 支持 WebSocket 通信测试
- 详见：`Skills/automated-testing/SKILL.md`

### scanner-e2e-test（Scanner 端到端测试）

`Skills/scanner-e2e-test/` — ClamAV 完整生命周期测试。

- **5 阶段**：配置 → 下载 → 安装 → 扫描 → 拦截
- 使用 EICAR 测试文件验证病毒检测
- 端口分配：8080（AI）、49000（Bot）、3310（ClamAV）、9999（本地回退）
- 详见：`Skills/scanner-e2e-test/SKILL.md`

### approval-popup-test（审批弹窗测试）

`Skills/approval-popup-test/` — 安全审批弹窗的 E2E 测试。

- 测试安全中间件的审批工作流
- WebSocket 消息触发 `<FILE_OP>` 标签
- 用户交互测试（批准/拒绝）
- plugin-ui.dll 集成

### build-project（构建流程）

`Skills/build-project/` — 标准化构建流程。

- **5 阶段**：准备 → 信息收集 → 构建 → 验证 → 报告
- 自动从 git tag 提取版本号
- 环境变量注入（`NEMESISBOT_VERSION` 等）
- 文件大小检查和编译验证

### structured-development（结构化开发）

`Skills/structured-development/` — 10 阶段完整开发生命周期。

- 严格的开发前研究阶段
- 开发计划创建和任务分解
- 多级测试（单元 → 集成 → 系统 → 回归）
- **关键**：必须确保项目编译成功
- 文档命名规范：`docs/PLAN/` 和 `docs/REPORT/`

### desktop-automation（桌面自动化）

`Skills/desktop-automation/` — Windows 桌面窗口操作。

- 基于 `window-mcp.exe` 的 MCP 服务器
- 窗口枚举和浏览器检测（Chrome/Edge/Firefox）
- 截图捕获，JPEG 格式输出

### wsl-operations（WSL 操作）

`Skills/wsl-operations/` — WSL 环境操作和管理。

- 通过 `wsl bash -lic` 执行命令
- 系统监控（CPU、内存、磁盘、网络）
- Windows ↔ WSL 路径自动转换
- 脚本：`scripts/` 目录

### dump-analyze（Dump 分析）

`Skills/dump-analyze/` — Windows 崩溃 Dump 文件分析。

- 使用 CDB（Console Debugger）
- 异常代码和调用栈提取
- 符号解析（PDB + Microsoft 公共符号）
- 脚本：`scripts/` 目录

### memory-uat（内存系统 UAT）

`Skills/memory-uat/` — 内存系统用户验收测试。

- 向量存储功能验证
- 嵌入管线测试
- 增强内存开关测试

### plugin-onnx-test（ONNX 插件测试）

`Skills/plugin-onnx-test/` — ONNX 插件 E2E 测试。

- 插件加载和初始化验证
- 嵌入推理功能测试
- 维度推断测试

### problem-analysis（问题分析方法论）

`Skills/problem-analysis/` — 系统化的问题根因分析**纪律清单**（不是测试，是调试方法论）。遇到 bug、异常行为、结果与预期不符、"为什么不工作"、或陷入"反复假设-修改-失败"循环时，**先读这个 Skill 再动手**。核心纪律：证据分级（推断≠结论，斩钉截铁下结论前必须有直接完整证据）、追完整数据流（先确认日志/证据是哪一层，别在不完整的中间层日志上打转）、"只一个路径失败=该路径特有代码问题"、一次只改一个变量、先查自己代码再怀疑外部（模型/服务）、不给自己找台阶。详见 `Skills/problem-analysis/SKILL.md`。

### bug-fix（改 BUG 纪律）— 必须使用

`Skills/bug-fix/` — 改 bug / 修 bug / fix bug / 改既有功能行为的**工程纪律清单**，和 `problem-analysis` 互补（那个管"找根因"，这个管"改对、改全、不引入新 bug"）。**要修任何 bug 或改任何既有行为时，动手前先读这个 Skill**。核心纪律：动代码前先定**真相源和不变量**、**读代码定位根因到具体位置**（不猜，无法确认就提疑问求助）、评估影响、借鉴同类功能；改时维持**单一真相源**、清理**对称彻底**；改完**横向扫同类一并修**。**构建/测试冒出的任何报错（哪怕看似不是本次引入的）都要逐个追根因、如实反馈，不得用“跟本次改动无关”搪塞**。杜绝症状补丁 / 拆东墙补西墙 / 改一个 bug 出一个新 bug。详见 `Skills/bug-fix/SKILL.md`。

### model-eval（模型能力评估）

`Skills/model-eval/` — **标准化的 LLM 工具调用支持评估**。接入新模型 / 横向对比 / agent 改动回归时用。流程：`model probe`（7 题客观探针）+ **12 题 battery × mini/normal/big 三档**（基础 B / 中级 I / 复杂 C，每 4 题一批、批间清 session）+ 评分表 → tier 推荐。**必须走 WebSocket**（`/api/chat/stream` 是裸代理不能评估）；**必须分批跑**（12 题连跑触发压缩通知污染回复）。脚本 `scripts/run_battery.py` + `scripts/prompts.json`。基线参考 `docs/INFO/2026-07-05_model-support-eval-suite.md`。详见 `Skills/model-eval/SKILL.md`。

### 远程 Registry

远程技能搜索和安装：
- 配置文件：`workspace/config/config.skills.json`
- CLI 命令：
  ```bash
  nemesisbot skills add-source <github-url>  # 自动探测仓库结构并添加为新源
  nemesisbot skills search <query>           # 并发搜索所有源，合并结果
  nemesisbot skills install <registry>/<slug> # 从指定源安装
  ```

---

## 架构概览

NemesisBot 是一个具有安全控制的分布式 AI 代理系统。架构围绕消息总线展开，将从各种通道来的入站消息路由到 Agent 引擎，然后将出站响应通过通道路由回去。本项目是从 Go 版本 1:1 复刻的 Rust 实现。

### 核心消息流

```
入站路径:
Channel (rpc/web/discord/feishu 等)
  -> ChannelManager.register()
  -> bus.publish_inbound(InboundMessage)
  -> AgentLoop 通过订阅接收
  -> Agent 执行（LLM + 工具）
  -> bus.publish_outbound(OutboundMessage)

出站路径:
bus.publish_outbound()
  -> ChannelManager.dispatch_outbound() tokio 任务
  -> 按名称找到匹配的通道
  -> channel.send(ctx, OutboundMessage)
  -> 通道投递到外部服务
```

**核心类型**（crates/nemesis-types/src/）：
- `InboundMessage`：Channel、SenderID、ChatID、Content、Media、SessionKey、CorrelationID
- `OutboundMessage`：Channel、ChatID、Content
- `CorrelationID`：用于 RPC 请求-响应匹配

### Workspace 结构（38 crates + 2 plugins + 16 workspace test-tools（含 nemesis-build-config） + 13 skills + web frontend）

```
web/                                       # Vue 3 前端项目（Vite MPA 模式）
├── package.json                           # 依赖：vue, vue-router, pinia, marked, highlight.js
├── vite.config.ts                         # MPA 双入口，输出到 crates/nemesis-web/static/
├── index.html                             # Dashboard HTML 入口
├── chat/index.html                        # Standalone Chat HTML 入口
├── public/fonts/                          # 字体资源
├── src/
│   ├── main.ts                            # Dashboard 入口（Router + Pinia）
│   ├── App.vue                            # 根组件（认证 + 路由）
│   ├── styles/                            # CSS 样式（theme, components, layout）
│   ├── composables/                       # 组合式函数（WebSocket, SSE, Theme, Toast, WSAPI）
│   ├── stores/                            # Pinia 状态（auth, app, chat, system）
│   ├── components/                        # 共享组件（Sidebar, ChatPanel, AuthOverlay, AppLayout, ToastContainer）
│   ├── views/                             # 25 个页面组件（见下方视图列表）
│   ├── router/index.ts                    # Vue Router（22 条路由，懒加载）
│   └── chat/main.ts                       # Standalone Chat 独立入口

crates/                                    # 38 个核心 crate
├── nemesis-types       # 共享类型定义
├── nemesis-bus         # 消息总线（中心发布/订阅）
├── nemesis-config      # 配置管理
├── nemesis-data        # 数据处理和存储抽象
├── nemesis-observer    # 观察者模式
├── nemesis-security    # 安全中间件 + ABAC + 病毒扫描（scanner/）
├── nemesis-sandbox     # Sandboxie 沙盒集成（执行体隔离 Layer 2，Windows）
├── nemesis-providers   # LLM Provider（HTTP/SSE/流式）
├── nemesis-tools       # 工具实现（32+ 工具）
├── nemesis-memory      # 对话记忆、向量存储和嵌入
├── nemesis-workflow    # 工作流引擎（DAG）
├── nemesis-skills      # 技能系统（远程 Registry + 本地）
├── nemesis-mcp         # MCP 协议实现（stdio + HTTP/SSE 传输）
├── nemesis-lsp         # 只读 LSP 客户端（definition/references/implementation/hover；rust-analyzer/gopls/ts-ls/pyright/clangd，stdio；L1/U19）
├── nemesis-agent       # Agent 核心（loop、instance、context、loop_tools、loop_executor、loop_continuation）
├── nemesis-channels    # 通道实现（21 个通道：web/rpc/telegram/discord/feishu/dingtalk/bluesky/email/...）
├── nemesis-forge       # 自学习框架（Forge）
├── nemesis-cluster     # 集群编排 + 续行快照
├── nemesis-services    # 服务管理器（BotService 生命周期）
├── nemesis-desktop     # 桌面集成（系统托盘；Linux 通过 plugin-ui.so 运行时加载 GTK + libayatana-appindicator3，主框架不依赖 GTK）
├── nemesis-web         # Web 服务器（HTTP + WebSocket + SSE，20 个 Handler）
├── nemesis-auth        # 认证（OAuth + PKCE）
├── nemesis-cron        # Cron 调度（基于 croner crate）
├── nemesis-devices     # 设备管理
├── nemesis-health      # 健康监控
├── nemesis-heartbeat   # 心跳系统
├── nemesis-http-pool   # HTTP 连接池
├── nemesis-logger      # 日志系统
├── nemesis-migrate     # 数据迁移（OpenClaw → NemesisBot）
├── nemesis-path        # 路径工具（PathManager）
├── nemesis-plugin      # 插件系统（HostServices vtable）
├── nemesis-routing     # 路由系统
├── nemesis-session     # 会话管理
├── nemesis-state       # 状态管理
├── nemesis-utils       # 通用工具
├── nemesis-verify      # 签名验证核心（v4 = Windows Authenticode 格式对齐：ECDSA P-256 + X.509 + CMS；lib + cdylib 导出 nv_*）
├── nemesis-voice       # 语音/音频处理
└── nemesis-ui          # UI 组件

plugins/
├── plugin-onnx         # ONNX 嵌入模型（本地推理，非 workspace member）
└── plugin-ui           # WebView UI 插件 + Linux 系统托盘（GTK + libayatana-appindicator3，非 workspace member）

test-tools/                                 # 25 个测试工具/项目（16 个 workspace member（含 nemesis-build-config） + 独立项目）
├── TestAIServer/       # Go AI 服务器模拟器（8 个测试模型，非 workspace member）
├── test-harness/       # 共享测试辅助库
├── integration-test/   # CLI 集成测试（298 断言，22 命令）
├── cluster-test/       # P2P 集群测试（12+6 测试）
├── cluster-uat/        # 集群 UAT 测试
├── cluster-node/       # 集群节点模拟器
├── e2e-tests/          # 端到端 AI 管线测试
├── http-test-server/   # HTTP 测试服务器（OAuth/Webhook/Channel）
├── websocket-client/   # WebSocket 客户端
├── ws-send/            # WebSocket 发送工具
├── mcp/                # MCP 协议测试（非 workspace member）
├── bus-benchmarks/     # 消息总线性能测试
├── approval-test/      # 审批流程测试
├── memory-test/        # 内存系统测试
├── nemesis-build-config/ # menuconfig 风格构建配置器（功能裁剪 TUI）
├── exe-sign-tool/      # 可执行文件签名/验签 CLI（v4 Authenticode，依赖 nemesis-verify）
├── revoke-server/      # 云端签发 + 吊销服务端（v3，axum + rusqlite）
├── verify-loader/      # 加载 nemesis_verify.dll 验证的测试工具
├── plugin-onnx-test/   # ONNX 插件测试（非 workspace member）
├── android-shell/      # Android Shell APK 工具
├── icon-tool/          # 图标生成工具
├── persona-gen-samples/ # 集群人格生成样本（JD/简历，非 workspace member）
├── examples/external/  # External channel 示例（非 workspace member）
└── resource/           # 文档/README 引用的截图与素材（非代码）

Skills/                                     # 13 个本地 Skill
├── automated-testing/      # E2E 自动化测试（8 阶段）
├── scanner-e2e-test/       # Scanner ClamAV 测试（5 阶段）
├── approval-popup-test/    # 审批弹窗测试
├── build-project/          # 标准化构建（5 阶段）
├── structured-development/ # 结构化开发（10 阶段）
├── desktop-automation/     # 桌面窗口操作（window-mcp）
├── wsl-operations/         # WSL 环境操作
├── dump-analyze/           # 崩溃 Dump 分析
├── memory-uat/             # 内存系统 UAT 测试
├── plugin-onnx-test/       # ONNX 插件 E2E 测试
├── problem-analysis/       # 问题根因分析方法论（调试纪律清单）
├── bug-fix/                # 改 BUG 工程纪律清单（动代码前先读，和 problem-analysis 互补）
└── model-eval/             # LLM 工具调用能力评估（probe + 12 题×3 档 battery）

nemesisbot/             # 主程序入口
├── src/main.rs         # CLI 入口（clap）
├── src/common.rs       # 通用工具（路径函数、版本格式化、日志初始化）
├── src/embedded.rs     # 编译时嵌入资源（static/ + workspace/）
├── src/adapters.rs     # 适配器（HealthServer、Heartbeat、ChannelManager、AgentLoop）
├── config/             # 配置模板（编译时嵌入）
├── workspace/          # 工作空间模板（编译时嵌入，onboard 时复制）
└── src/commands/       # 25 个 CLI 命令实现
    ├── gateway.rs      # Gateway 服务（依赖注入组装点）
    ├── dashboard.rs    # 一键打开 Dashboard UI（自动启动网关）
    ├── agent.rs        # Agent 管理
    ├── cluster.rs      # 集群管理
    ├── model.rs        # 模型管理
    ├── forge.rs        # Forge 命令
    ├── channel.rs      # 通道管理
    ├── security.rs     # 安全配置
    ├── skills.rs       # 技能管理
    ├── persona.rs      # 人格管理（list/search/install/activate/remove）
    ├── cron.rs         # Cron 管理
    ├── mcp.rs          # MCP 管理
    ├── auth.rs         # 认证
    ├── scanner.rs      # 病毒扫描（子命令：install/enable/disable/update/test/info）
    ├── sandbox.rs      # 沙盒（Sandboxie：install/start/stop/status/pending/commit/clear）
    ├── workflow.rs     # 工作流
    ├── log.rs          # 日志配置
    ├── cors.rs         # CORS
    ├── migrate.rs      # 数据迁移
    ├── memory.rs       # 增强内存管理
    ├── voice.rs        # 语音管理
    ├── status.rs       # 状态查询
    ├── shutdown.rs     # 关闭
    └── test_cmd.rs     # 测试命令（hidden）
```

### 配置模板

位置：`nemesisbot/config/`（编译时嵌入，`onboard default` 会复制到工作空间）

**JSON 配置文件**：

| 文件 | 用途 |
|------|------|
| `config.default.json` | 主配置模板 |
| `config.scanner.default.json` | Scanner 配置（含 ClamAV 默认 URL） |
| `config.mcp.default.json` | MCP 配置 |
| `config.cluster.default.json` | 集群配置（Rust 多了 `llm_timeout_secs`） |
| `config.skills.default.json` | Skills 配置 |
| `config.enhanced_memory.default.json` | 增强内存配置 |
| `config.security.windows.json` | Windows 安全策略 |
| `config.security.linux.json` | Linux 安全策略 |
| `config.security.darwin.json` | macOS 安全策略 |
| `config.security.other.json` | 其他平台安全策略 |

**Markdown 人格模板**：

| 文件 | 用途 |
|------|------|
| `IDENTITY.md` | AI 身份/人设 |
| `SOUL.md` | AI 核心行为原则 |
| `USER.md` | 用户偏好 |
| `AGENT.md` | Agent 行为配置 |
| `BOOT.md` | 启动配置 |
| `BOOTSTRAP.md` | 引导配置 |
| `HEARTBEAT.md` | 心跳配置 |
| `MCP.md` | MCP 说明 |
| `TOOLS.md` | 工具说明 |

### 工作空间模板

位置：`nemesisbot/workspace/`（编译时嵌入，`onboard` 时复制到用户工作空间）

```
workspace/
├── IDENTITY.md, SOUL.md, USER.md...     # 人格文件（覆盖 config/ 中的模板）
├── memory/MEMORY.md                     # 内存模板
├── scripts/                             # 辅助脚本（install-clawhub-skill）
└── skills/                              # 内置技能
    ├── cluster/                         # 集群相关技能
    ├── github/                          # GitHub 技能
    ├── skill-creator/                   # 技能创建工具
    ├── summarize/                       # 摘要技能
    ├── test-skill/                      # 测试技能
    └── weather/                         # 天气技能
```

### 模块架构

**消息总线**（crates/nemesis-bus/）：
- 消息路由的中心发布/订阅系统
- 通道订阅 InboundMessage，发布 OutboundMessage
- 线程安全，支持多个并发订阅者（broadcast channel）

**通道管理器**（crates/nemesis-channels/src/manager.rs）：
- 所有通道的生命周期管理（start/stop）
- 将出站消息路由到适当的通道
- 处理消息过滤和投递
- 关键：`dispatch_outbound()` 在专用 tokio 任务中运行，监听 bus outbound receiver

**Agent 引擎**（crates/nemesis-agent/）：
- `loop.rs`：核心执行循环（AgentLoop::run）
  - 从 bus 接收 InboundMessage
  - 使用对话历史构建上下文
  - 调用 LLM 并传入工具定义
  - 执行工具（可能多次迭代）
  - 将最终响应发布到 OutboundMessage
  - 轮次治理：`max_tool_iterations` 默认 **100**（0=不限；旧默认 20 来自 OpenClaw 迁移，会话撞顶硬崩的根因，已修），到顶先给一轮 **grace round** 收尾再暂停（不硬崩，提示用户可继续）；每轮开头 `check_config_reload()` 检 config.json mtime 变化自动重解析 tier；dispatch 前跑 `args_validator`
- `turn_guard.rs`：卡住循环检测（**签名按 (tool,error)，不按 args**）— 交替循环⑥ / 退化输出⑦ / 失败循环④ / 成功循环⑤ / 文本重复⑧ / 分级 compaction⑩ + escalation 硬停（封住成本下界）；独立 `validation_failures` 计数器（成功归零，耗尽 tier 预算则停）
- `loop_tools.rs`：工具注册 + McpDiscoverTool + CliReferenceTool + McpListTool + RPCChannel 配置
- `remote_executor_tool.rs` + `executor_pipe.rs`：执行体隔离 Layer 1/2（见下方「执行体隔离 + Sandboxie 沙盒」）
- `args_validator.rs`：工具 args schema 校验 + 编辑距离 autofix + 多余字段忽略（详见「模型能力分级」Phase 2）
- `loop_executor.rs`：LLM 调用执行引擎（流式/非流式）⚠️ **legacy**（生产不实例化，只剩类型定义被 loop.rs 用；改 agent 行为去 `loop.rs`）
  - `build_messages()` 实时注入时间/dynamic 字段（不放 system prompt）— 优化 prompt cache 命中率
- `loop_continuation.rs`：集群续行处理（快照加载/恢复/续行 LLM）
- `instance.rs`：Agent 实例管理
- `memory.rs`：对话记忆和上下文
- `context.rs`：请求上下文处理
- `request_logger.rs`：主 Agent LLM 请求日志内核（被 ClusterRequestLoggerObserver 共享）
- `request_logger_observer.rs`：主 Agent 的事件 → 文件转换 Observer

**工具**（crates/nemesis-tools/ + crates/nemesis-agent/src/loop_tools.rs）：
- 32+ 个工具实现：filesystem, shell, async_shell, web (web_search/web_fetch), edit, spawn, browser, hardware,
  cluster_rpc, cron, sleep, skills_ops (skills_list/skills_info/find_skills/install_skill/skill_manage),
  bootstrap, subagent, memory (search/store/forget/list), skills,
  mcp_discover（stdio + HTTP 发现）, cli_reference（CLI 命令按需查询）, mcp_list（已注册 MCP 工具列表）,
  screen_capture（屏幕截图）, desktop_automation（桌面自动化, 基于 window-mcp）,
  grep（代码搜索）, git（仓库操作）, workflow_run（触发已注册工作流）, forge_bridge（Forge 桥接）,
  claude_code / codex_delegate（CLI 委派，U13，config 默认关 + PATH 探测注册）,
  lsp（只读语义代码查询 definition/references/implementation/hover，U19/L1，config 默认关 + 语言服务器 PATH 探测注册；由 nemesis-lsp crate 驱动真实语言服务器）
  + types/registry/executor/message 基础设施

**执行体隔离 + Sandboxie 沙盒**（`crates/nemesis-agent/src/remote_executor_tool.rs` + `executor_pipe.rs` + `crates/nemesis-sandbox/`）：

LLM 高危本地操作（`MOVE_TOOLS` = exec / 8 个 file 工具 / grep / git）可从 gateway 进程内剥离到独立子进程，进一步套进 Sandboxie 盒。两层开关在 `config.json` 的 `executor` 段（`ExecutorSeparationConfig`，`crates/nemesis-config/src/lib.rs:159`，`#[serde(default)]`，默认全关）：

```json
"executor": { "enabled": false, "sandbox": false }
```

- **Layer 1（`enabled=true`）**：`agent_factory.rs` 把 `MOVE_TOOLS` 替换成 `RemoteExecutorTool`（schema 从本地同源 impl 抄，字节不变→不破 prompt cache）。每次工具调用 spawn 一个 `nemesisbot.exe` 子进程（per-call，跑完即退）+ env `NEMESISBOT_ROLE=executor` + `NEMESISBOT_EXECUTOR_WORKSPACE=<abs path>`；`main.rs` 顶部在 `Cli::parse` 前检测 ROLE 短路进 `exec_worker::run`（不装配 web/channels/cluster/security/...）。协议：一行 JSON `{tool,args,context}` 请求 / 一行 `{result,error}` 响应，stdio 往返。**安全 8 层仍在 gateway `handle_tool_call`（loop.rs dispatch）执行前跑**——子进程收到的都是已批准操作（判断在 gateway，子进程是哑执行；沙盒是**第 9 层最终防御红线**——盒级物理隔离兜底，不在子进程重复判断）。
- **Layer 2（`sandbox=true`，需 `enabled=true` + `sandbox` feature + Windows）**：子进程经 `Start.exe /box:NemesisBox` 起进 Sandboxie 盒，transport 换 Windows 具名管道 `\\.\pipe\NemesisBox_<id>`（`executor_pipe.rs`；Start.exe 不转发 stdio → 必须用管道跨盒）。盒 ini（`<home>/workspace/tools/sandboxie/Sandboxie.ini`）：`Enabled=y` + `AllowNetworkAccess=n` + `DropAdminRights=y` + `OpenPipePath=\Device\NamedPipe\NemesisBox_*`，`FileRootPath` 指 `<home>/workspace/tools/sandboxie/box/NemesisBox`。**全隔离**：盒里对工作区的写入不自动落真盘（Sandboxie 无"写穿真盘"指令），需 `sandbox pending`（**只枚举工作区子树**——工作区外的破坏不进列表也提交不了，安全命门）+ `sandbox commit` 手动提交回真盘；exec 越界 / 注册表 / 网络破坏永远关在盒里。
- **优雅降级**：`sandbox=true` 但 Sandboxie 未就绪（Start.exe 缺或 SbieSvc 未跑）→ 回退 Layer 1（无盒）+ warn，不崩（gateway 不被 SbieSvc 状态绑架）；`sandbox` feature 编译期裁掉 → sandbox=true 忽略 + warn（仍走 Layer 1）。
- **nemesis-sandbox crate**（Windows-only 运行时，非 Windows 编译为 stub 返回 Err 保 `cargo check --workspace` 绿）：自动下载官方 Sandboxie Classic release（tag `v1.17.9` / Classic `5.72.9`）+ SHA-256 校验 + 捆绑 7z 解压到 `<home>/workspace/tools/sandboxie/runtime/`；KmdUtil install/start/stop/delete 驱动 `SbieDrv`（mini-filter altitude `86900`）+ 服务 `SbieSvc`；借 `elevation`（ShellExecuteW runas）自重启提权子跑 KmdUtil，parent 轮询 `service_state` 确认副作用；`IniPath` 重定向到 home 下，**不落 `C:\Windows`**；install-once-resident（动态装卸驱动有 BSOD 风险，启用时装一次常驻，卸载只在显式 stop/uninstall）。
- 关联文件：`nemesisbot/src/exec_worker.rs`（child 角色）、`nemesisbot/src/commands/sandbox.rs`（CLI）、`crates/nemesis-web/src/handlers/sandbox.rs` + `web/src/views/SandboxView.vue`（Dashboard 沙盒页）。计划文档：`docs/PLAN/2026-07-08_executor-separation.md`（Layer 1）+ `docs/PLAN/2026-07-09_sandboxie-integration.md`（Layer 2）。

**通道**（crates/nemesis-channels/）：
- 每个通道实现 `Channel` trait
- 21 个通道：web, websocket, webhook_inbound, rpc, telegram, discord, feishu, dingtalk, email, bluesky,
  slack, whatsapp, qq, matrix, irc, signal, mastodon, onebot, external, maixcam, line
- `base.rs`：BaseChannel 提供通用功能
- `rpc_channel.rs`：用于 RPC/集群通信的特殊通道
  - 通过 CorrelationID 前缀匹配响应：`[rpc:correlation_id] content`
  - 对 peer_chat 至关重要：响应必须有 correlation ID 前缀

**集群/RPC**（crates/nemesis-cluster/）：

**集群三个配置文件职责**（去歧义后）：
- `workspace/config/config.cluster.json`：**用户系统参数层**。只含系统参数：`enabled`、`port`、`rpc_port`、`broadcast_interval`、`llm_timeout_secs`、`token`（UDP 发现层）。**不含**节点身份字段。
- `workspace/cluster/peers.toml`：**静态配置层**。含本节点身份 `[node]` 段（id/name/role/category/tags/capabilities/address）+ 可选的静态已知 peers `[peers.X]` 表。**不含** `[cluster]` 段。
- `workspace/cluster/state.toml`：**运行时状态层**。只含发现的远程节点列表 `[[discovered]]` + `last_sync`。**不含** `[cluster]` 或 `[local_node]` 段。
- 两个 token 是**不同用途**：config.cluster.json 的 `token`（UDP 发现层加密）和 peers.toml 的 `[node]` 段不含 token（RPC 鉴权 token 在 cluster 内部从 config 读，不在 peers.toml）。

- `cluster.rs`：主集群编排
  - `node_name`/`role`/`category`/`tags` 使用 `parking_lot::RwLock` 支持运行时修改
  - `removed_peers: RwLock<HashSet<String>>` 黑名单，阻止已删除节点被重新发现
  - `unban_node()` 解除黑名单，允许节点重新加入
  - `start()` 中自动将 `0.0.0.0` 替换为真实 IP（优先非回环地址）
  - 所有 getter 返回 owned 类型，内部使用 `.read().clone()` 模式
- `discovery/`：加密 UDP 自动发现
  - `ClusterCallbacks` trait 集成 discovery 服务
  - `known_nodes` 缓存 + 黑名单检查
- `continuation_store.rs`：续行快照持久化存储
  - 快照存储在 `{workspace}/cluster/rpc_cache/{taskID}.json`
  - 支持内存+磁盘双写，启动时可从磁盘恢复
- `task_manager.rs`：异步任务状态管理（回调驱动）
- `rpc/client.rs`：调用远程节点的 RPC 客户端（超时 60 分钟）
- `rpc/server.rs`：处理传入请求的 RPC 服务器
- `rpc/peer_chat_handler.rs`：处理 peer_chat（超时 59 分钟）
  - `source_node_id` 读 `rpc_meta.from`（不再读 `payload._source.node_id`）
  - `sender_id` 是复合键 `format!("{}/{}", source_node_id, chat_id)` — 跨节点会话隔离
- `transport/`：TCP 连接池和帧处理
- `cluster_log_reader.rs`：集群日志读取（UTC 时间自动转本地时间）

**集群请求日志 ClusterRequestLogger**（`nemesisbot/src/cluster_request_logger_observer.rs`）：
- 按对端设备 ID + 任务 ID 分目录隔离：
  ```
  workspace/logs/cluster_logs/{device_id}/{ts_ms}_{task_id}/
  ├── 00.request.md
  ├── 01.AI.Request.raw.json
  ├── 02.AI.Response.raw.json
  └── ... (多轮 LLM 自动递增)
  ```
- 共享 `nemesis-agent/src/request_logger.rs` 内核（文件写入逻辑、JSON envelope 格式）
- 集群 Agent 拥有独立 `nemesis_observer::Manager`，与主 Agent 事件分发隔离
- 旧 `cluster_{date}.log` 事件流保留不动，两套机制并存
- 详见 `docs/REPORT/2026-06-15_cluster-request-logger.md`

**集群 Dashboard**（web/src/components/cluster/）：
- 6 个 Tab 页：概览、拓扑、身份、任务、日志、设置
- `ClusterIdentity.vue`：运行时身份编辑（名称、角色、分类、标签）+ 人格文件预览
- `ClusterNodes.vue`：节点列表（在线/离线状态、本节点标记、Ping/移除操作）
- `ClusterTopology.vue` + `TopologyCanvas.vue`：拓扑可视化
- WSAPI 命令：`node.update_identity`、`identity.get_files`、`nodes.list`（含 isLocal）、`nodes.ping`、`nodes.remove`

**安全 8 层 pipeline**（`crates/nemesis-security/src/pipeline.rs::execute`，工具调用前按序跑，任一层拦截即终止）：
- ① 注入检测（injection）→ ② 命令守卫（command）→ ③ ABAC（auditor 策略引擎）→ ④ 凭据扫描（credential）→ ⑤ DLP → ⑥ SSRF → ⑦ 病毒扫描（scanner/）→ ⑧ 审计链（Merkle 防篡改 append-only）
- `middleware.rs`：拦截危险操作（文件、进程、注册表、网络）
- `auditor.rs`：ABAC 策略引擎（基于属性的访问控制，Layer 3）
- 附加机制（不在 8 层编号内）：`guardian`（LLM judge，CRITICAL 操作过规则层后做语义二审，防 prompt 注入；execute 同步返回 allow 后 agent loop 异步 await）+ `approval`（高危操作人工审批弹窗）
- **第 9 层**：Sandboxie 沙盒——盒级物理隔离兜底（见上方「执行体隔离 + Sandboxie 沙盒」）
- 四个风险级别：LOW / MEDIUM / HIGH / CRITICAL
- 可通过配置禁用（`security.enabled = false`）
- `scanner/`：病毒扫描引擎（ScanChain、ClamAV）
- **完整九层参考**（每层模块/默认开关/风险分级/代码锚点）：`docs/INFO/2026-07-10_security-9-layers.md`

**Forge 自学习**（crates/nemesis-forge/）：
- **概述**：基于 Read -> Execute -> Reflect -> Write 核心循环
- **子系统**：Collector + Reflector + Factory + Registry + Syncer + Sanitizer + Bridge
- **Phase 6 闭环学习**：Pattern -> Action -> Deploy -> Monitor -> Feedback
- 主开关：`config.json` 的 `forge.enabled`（默认 false）

**服务管理**（crates/nemesis-services/）：
- `bot_service.rs`：BotService 生命周期管理
- 组装所有组件：Agent、Channels、Security、Forge、Cron、Web
- Gateway 依赖注入在 `nemesisbot/src/commands/gateway.rs` 中完成

**Web 服务**（crates/nemesis-web/）：
- HTTP API + WebSocket + SSE 流式
- `/api/chat/stream` POST 端点（SSE 响应）
- WebSocket 三级协议：`{type: "message", module: "chat", cmd: "send", data: {content: "..."}}`
- 前端由 Vue 3 + Vite MPA 构建（源码在 `web/`，构建输出到 `static/`）
- Dashboard 入口：`index.html`（Vue Router 管理 22 个路由页面；`views/` 下共 25 个 .vue 组件）
- Standalone Chat 入口：`chat/index.html`（独立聊天页面）
- **20 个 Handler**（`handlers/` 目录）：
  - agent, channels, cluster, config, forge, identity, logs, mcp, memory,
  - models, persona, sandbox, scanner, security, skills, system, tasks, tools, voice, workflow
- **其他核心模块**（顶层）：
  - `api_handlers.rs` — API 路由注册
  - `server.rs` — Axum 服务器 + 静态文件服务
  - `websocket_handler.rs` — WebSocket 连接处理
  - `ws_router.rs` — WebSocket 消息路由
  - `sse_chat.rs` — SSE 聊天流
  - `sse_log_layer.rs` — SSE 日志桥（tracing 事件 → EventHub，桥到 Logs Dashboard）
  - `cors.rs` — CORS 中间件
  - `events.rs` — 事件系统
  - `session.rs` — 会话管理
  - `history.rs` — 聊天历史
  - `protocol.rs` — 消息协议

**Logs Dashboard**（`web/src/views/LogsView.vue` + 5 个子组件）：
- 4 个 Tab：实时事件流（SSE）/ 会话浏览器（对话历史 + 本地 LLM 调用 + 集群任务）/ 安全审计 / 审计链
- 实时事件流走 `SseLogLayer`（tracing target → source 字段映射），支持 general/cluster/security/llm 4 源
- 后端命令：`logs.security`、`logs.chain_list`、`logs.chain_verify`、`logs.session_list`、`logs.session_detail`、`logs.requests`、`logs.request_detail`、`logs.cluster_task_list`、`logs.cluster_task_detail`
- 详见 `docs/REPORT/2026-06-17_logs-dashboard-phase-a-b-complete.md`

**人格系统 Persona**（`crates/nemesis-web/src/handlers/persona.rs` + `nemesisbot/src/commands/persona.rs`）：
- 远程仓库：`agency-agents`（274 个 .md 文件，自动跳过 README/scripts/examples/integrations）
- 转换引擎：`.md` 文件 → 工作空间人格目录（IDENTITY.md / SOUL.md / USER.md / AGENT.md）
- CLI 命令：`nemesisbot persona list/search/install/activate/remove/current/restore`
- WSAPI 命令：`persona.list`、`persona.search`、`persona.install`、`persona.activate`、`persona.remove`、`persona.current`
- 激活后 AgentInstance 用新 system_prompt 重启（不重启进程）
- 详见 `docs/INFO/2026-06-12_persona-system-implementation.md`

### 关键配置位置

**Web 嵌入资源**（nemesisbot/src/embedded.rs）：
- `EMBEDDED_STATIC`：`include_dir!` 在编译时嵌入 `crates/nemesis-web/static/` 目录
- `EMBEDDED_WORKSPACE`：`include_dir!` 在编译时嵌入 `nemesisbot/workspace/` 目录
- Vite 构建输出结构：`index.html`、`chat/index.html`、`assets/*.js`、`assets/*.css`、`fonts/`
- 运行时优先使用 exe 旁边的 `static/` 目录，否则从内存直接服务（零磁盘 IO）
- `extract_workspace_templates()` — onboard 时提取工作空间模板（不覆盖已有文件）
- `extract_workspace_templates_overwrite()` — 强制覆盖提取
```rust
// RPC Client (rpc/client.rs)               - 60 分钟（最外层 TCP 连接超时）
// PeerChat Handler (peer_chat_handler.rs)   - 59 分钟（B 端 LLM 处理超时）
// RPCChannel (loop_tools.rs)                - 24 小时（B 端安全网）
let config = RpcChannelConfig {
    message_bus: msg_bus,
    request_timeout: Duration::from_secs(24 * 3600),
    cleanup_interval: Duration::from_secs(30),
};
```

**重要**：RPC Client (60min) > PeerChat (59min) 确保同步调用层正确超时。RPCChannel 设为 24 小时是因为它作为 B 端 LLM 的安全网，不应比外层先超时。

---

## 模型能力分级（小模型支持）

不同大小的 LLM 用同一套工具配置效果差异很大：小模型被 41 个工具搞乱选型、参数 schema 经常出错；大模型能稳定使用全量工具。NemesisBot 按模型能力**分级供给**，配套校验/修复层兜底。完整设计 + 12 题×3档基准数据见 `docs/REPORT/2026-07-05_small-model-tool-robustness.md` + `docs/INFO/2026-07-05_model-support-eval-suite.md`。

### Tier 与 config

每个模型在 config.json 的 `model_list[]` 条目有 `model_tier` 字段：`auto`（默认）/ `mini` / `normal` / `big`。`model add` 自动写 `auto`。

**解析优先级**（`resolve_active_tier`，`crates/nemesis-types/src/capability.rs`）：`model_tier != auto` 直接采信（用户最大）→ 否则按 `model_size_b` → 名字抠 `NB`/关键词 → 默认 **big**（最安全，不饿着强模型）。opaque 别名（如 `astron-code-latest`）认不出 → 默认 big，建议跑 `model probe`。

| 档 | 工具数（`tier_allowed_tools`）| validation 重试预算 | 适用 |
|---|---|---|---|
| mini | 核心 13 | 3 | 9–35B 小模型 |
| normal | ~26 | 2 | 70–120B |
| big | 全量 42 | 1 | 200B+ / 云端旗舰 |

### 运行时切换无需重启

`AgentLoop` 持有 `tier`（RwLock 缓存）+ `config_path`，config.json 是**唯一真相源**（无快照表）。两条路径自动重解析：
- **切模型**（`/model`、`set_provider_and_model`）→ `refresh_active_tier()` 立刻读 config.json。
- **config.json 磁盘变化**（dashboard 加模型、CLI `model set-tier`）→ 每轮 LLM 开头的 `check_config_reload()`（mtime 检测）发现 → 重解析。

### Phase 2 校验 + 自动修复（`crates/nemesis-agent/src/args_validator.rs`）

工具调度前（`loop.rs` dispatch）校验 args（required/type/enum）：
- **Valid** → 执行。
- 字段名是某真字段的**近邻 typo**（编辑距离 ≤2，如 `patch→path`）→ **自动修**后执行。
- **纯多余字段**（无近邻，如多传 `encoding`）→ **忽略**（对齐 JSON Schema 默认；不拦停强模型）。
- 缺必填/类型错/enum 错 → **回灌结构化错误**当 tool 结果，模型下轮自纠。
- **歧义 typo**（如 `xat` 同时近邻 `bat`/`cat`）→ 不猜，回灌。

独立 `validation_failures` 计数器（成功归零，失败 +1），耗尽 tier 预算则停（防小模型烧满 max_turns）。

### Phase 1 时间/平台注入（`loop.rs::build_messages`）

每次 LLM 调用前，在最新 user 消息**前**注入临时 system 消息：`# Current Time` + `# Environment`（platform/shell/time_cmd）。前缀字节不变 → 保 prompt cache 命中；模型读到时间就不必再调 `exec date`（这是"问几点了卡 60s"bug 的根因修复）。

### Phase 4b 探针（`crates/nemesis-agent/src/probe.rs`）

`model probe <name>` 跑 7 个固定任务（exec/read_file/create_dir/grep/write_file/edit_file/cluster_rpc），三维打分（format/selection/schema）→ 映射 tier，写回 config。**只 CLI 显式触发，绝不在对话中途跑**。

### 关键模块

- `crates/nemesis-types/src/capability.rs`：ModelTier / detect_tier / resolve_active_tier / tier_allowed_tools。
- `crates/nemesis-agent/src/args_validator.rs`：schema 校验 + 编辑距离 autofix + 多余字段忽略。
- `crates/nemesis-agent/src/probe.rs`：7 题探针 + 打分。
- `loop.rs`：tier 缓存 + tool_defs 过滤 + dispatch 校验注入 + 时间注入 + check_config_reload。
- `agent_factory.rs`：启动解析 tier + `set_config_path`。

### 已知边界

- `loop_executor.rs` 的 `AgentLoopExecutor` 是 **legacy**（生产不实例化，只剩类型定义被 loop.rs 用）。改 agent 行为去 `loop.rs`，别改 loop_executor（其顶部有 ⚠️ STATUS 注释）。
- 评估新模型用 `docs/INFO/2026-07-05_model-support-eval-suite.md` 的 10 题参考集 + 评分模板；`/api/chat/stream` 是裸 LLM 代理（不走 agent 循环），评估必须走 WebSocket。

---

## 关键模式和约定

### 通道 Correlation ID 模式

对于 RPC/集群通信，响应必须包含 correlation ID 前缀：

```rust
// 正确格式
let content = format!("[rpc:{}] 实际响应内容", correlation_id);

// RpcChannel::send() 提取 correlation_id 并路由到待处理的请求
// 如果缺少前缀，响应会丢失
```

### 续行快照模式（Phase 2）

当 LLM 调用 `cluster_rpc` 工具时的非阻塞流程：

```
A 端发起（非阻塞）:
1. LLM 调用 cluster_rpc -> 工具返回 AsyncResult(taskID)
2. AgentLoop 保存续行快照:
   - 内存: continuations[taskID] = {messages, toolCallID, channel, chatID, session_key}
   - 磁盘: {workspace}/cluster/rpc_cache/{taskID}.json（session_key 字段 #[serde(default)] 兼容旧快照）
3. LLM 生成 "已发送请求" -> 发送给用户 -> 当前轮次结束

B 端处理:
4. B 立即返回 ACK -> A 解除 TCP 连接
5. B 异步处理 LLM -> 完成后回调 A 的 peer_chat_callback

A 端接收回调（续行）:
6. CallbackHandler -> TaskManager.complete_callback -> on_task_complete(taskID)
7. Cluster.handle_task_complete -> bus.publish_inbound("system", "cluster_continuation:{taskID}")
8. AgentLoop.process_message 拦截 cluster_continuation 前缀
9. handle_cluster_continuation(taskID, session_store):
   - 加载续行快照（save barrier）
   - 追加真实工具结果到 messages
   - 续行 LLM 调用（支持多步骤工具链继续执行）
   - 持久化最终回复（关键：避免绕过正常路径）:
     * chat_log::append_chat_log(session_key, "assistant", final_content)
     * session_store.get_or_create(session_key) + add_message + save
     * session_key 为空时跳过（兼容旧磁盘快照）
   - 发送最终响应给用户
```

**续行回复持久化要点**：
- `handle_cluster_continuation` 必须接收 `Option<&SessionStore>` 参数，4 个调用点（loop.rs inline + spawned、loop_executor.rs、loop.rs wrapper）通过 `.map(|v| v.as_ref())` 把 `Option<&Arc<SessionStore>>` 转换
- `ContinuationData` / `ContinuationSnapshot` 加 `session_key: String` 字段
- `save_continuation` 签名带 `session_key: &str`，3 个调用点（loop_executor.rs、loop.rs spawned、loop_continuation.rs 递归）
- load 路径让 session_key 跟着快照走：`wait_for_continuation`（3 处）、`try_load_from_disk`、`recover_to_manager`
- 单测：`crates/nemesis-agent/src/loop_continuation/tests.rs` 中 3 个 session_log 写入测试 + 4 个 session_key 流转测试
- 集成测试：`test-tools/cluster-uat/` 的 T14 用 PEER_CHAT marker 端到端验证 session_log 持久化

### 工作空间和配置

**路径优先级**：
1. `--local` 标志（强制使用 ./.nemesisbot）
2. 环境变量 `NEMESISBOT_HOME`
3. 自动检测（如果当前目录存在 .nemesisbot）
4. exe 目录检测（如果 nemesisbot.exe 所在目录存在 .nemesisbot）
5. 默认：`~/.nemesisbot`

**关键文件**：
- `IDENTITY.md`：AI 人设/身份
- `SOUL.md`：AI 核心行为原则
- `USER.md`：用户偏好
- `config.json`：主配置
- `cluster/peers.toml`：已知的集群对等节点

### 安全区域

**操作风险级别**：
- **CRITICAL**：process_exec、process_kill、registry_write、system_shutdown
- **HIGH**：file_write、file_delete、dir_create、dir_delete、process_spawn
- **MEDIUM**：file_edit、file_append、registry_read、network_download
- **LOW**：file_read、dir_list、network_request、hardware_i2c

**工作空间限制**：
- `restrict_to_workspace: true` 限制文件访问仅在工作区内
- 安全中间件仍可拦截工作区外的操作
- 设置为 false 以获得完整系统访问（不推荐）

---

## 重要说明

### Windows PowerShell 兼容性

项目对 Windows PowerShell 的 `curl` 别名有特殊处理（会重定向到 `Invoke-WebRequest`）：
- 工具会自动将 `curl` 替换为 `curl.exe`
- 这对 Windows 上的外部工具执行至关重要

### Windows 后台进程管理

**严格禁止的命令**：
- `start /B` - 会弹窗报错，无人干预时永久卡住
- `cmd /c start` - 会创建新窗口，导致流程阻塞
- `start` - Windows 批处理命令，不适合后台运行

**推荐方法**：

**方法 1：使用项目辅助脚本（推荐）**
```powershell
# PowerShell
.\Skills\automated-testing\scripts\setup-env.ps1
.\Skills\automated-testing\scripts\cleanup-env.ps1
```
```bash
# Bash
bash Skills/automated-testing/scripts/setup-env.sh
bash Skills/automated-testing/scripts/cleanup-env.sh
```

**方法 2：PowerShell Start-Process**
```powershell
Start-Process -FilePath "./nemesisbot.exe" -ArgumentList "gateway" -WindowStyle Hidden
```

**方法 3：Bash 后台运行**
```bash
# 使用 Bash 工具的 run_in_background 参数
./nemesisbot.exe gateway > nemesisbot.log 2>&1 &
```

**进程管理**：
```bash
# Windows 停止进程
taskkill //F //IM nemesisbot.exe

# 查找进程 PID
tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}'
```

### 多实例部署

使用 `--local` 标志运行多个独立的 bot 实例：
```batch
mkdir C:\Bots\bot1
cd C:\Bots\bot1
nemesisbot.exe --local gateway
```

每个实例获得自己的 `.nemesisbot/` 目录，而不是使用 `~/.nemesisbot`。

---

## 文件组织参考

**入口点**：`nemesisbot/src/main.rs` - CLI 命令路由（clap）
**CLI 命令**：`nemesisbot/src/commands/` - 24 个命令实现
**配置模板**：`nemesisbot/config/` - 默认配置 + 人格模板（编译时嵌入）
**工作空间模板**：`nemesisbot/workspace/` - 工作空间初始文件（编译时嵌入）
**Web 前端**：`web/` - Vue 3 + Vite MPA 项目，构建输出到 `crates/nemesis-web/static/`

**核心模块**：
- `crates/nemesis-agent/src/loop.rs` - 主执行循环（**理解 agent 流程的起点**）
- `crates/nemesis-bus/` - 消息总线
- `crates/nemesis-channels/src/manager.rs` - 通道生命周期和路由
- `crates/nemesis-cluster/` - 集群编排和续行快照
  - `cluster.rs` - 主编排、bus 注入、handle_task_complete
  - `continuation_store.rs` - 续行快照持久化存储
  - `task_manager.rs` - 异步任务状态 + on_task_complete 回调
- `crates/nemesis-cluster/src/rpc/` - 集群通信的 RPC 客户端/服务器
- `crates/nemesis-security/` - 安全中间件和 ABAC
- `crates/nemesis-security/src/scanner/` - 病毒扫描引擎（ScanChain、ClamAV）
- `crates/nemesis-lsp/` - 只读 LSP 客户端（definition/references/implementation/hover 四操作；manager=会话生命周期，registry=语言→服务器映射+PATH 探测，proto=帧/JSON-RPC/URI；agent 侧工具在 `nemesis-agent/src/loop_tools/lsp_tool.rs`，config `agents.lsp_tool` 默认关）
- `crates/nemesis-forge/` - 自学习框架
- `crates/nemesis-services/` - 服务管理器（BotService 生命周期）
- `crates/nemesis-web/` - Web 服务器
- `crates/nemesis-desktop/` - 桌面集成（Linux 通过 plugin-ui.so 运行时加载 GTK + libayatana-appindicator3，主框架不依赖 GTK）
- `plugins/plugin-onnx/` - ONNX 嵌入模型
- `plugins/plugin-ui/` - WebView UI 插件 + Linux 系统托盘（GTK + libayatana-appindicator3）

**文档**：
- `docs/BUG/` - 已知问题和调查，已知问题的分析，文件创建到这里，每个文件记录一个 BUG 或一个文件记录多个 BUG
- `docs/INFO/` - 技术信息，项目技术信息，文件创建到这里
- `docs/PLAN/` - 规划文档，新的开发规划，文件创建到这里，每个文件记录一个开发计划，便于完成计划后归档
- `docs/REPORT/` - 分析报告，开发过程中各种报告，文件创建到这里

### 文档操作说明

- 文件目录内的所有文件格式均为 markdown 格式。
- 文档内的文件名字均以日期开头，文件名格式为：YYYY-MM-DD_[正常文件名].md。
- `docs/BUG/` 目录只存放现有存在的 BUG 。
- 若 BUG 修复完成，则删除 BUG 信息，并添加文件到 `docs/REPORT/` 目录，标记 BUG 修复并记录报告。
- `docs/PLAN/` 目录只存放现在还存在的开发计划，包括进行中、暂停的。
- 若开发计划已经完成，则归档到其他目录中，如 `docs/INFO/` 或 `docs/REPORT/` 中，同时删除 `docs/PLAN/` 目录中的原始文件。

---

## 安全配置注意事项

**工作区隔离是默认且推荐的配置**：
- Bot 只能访问 workspace 目录
- 所有文件操作受安全策略控制
- 危险操作需要审批或会被拦截

**禁用安全模块**（不推荐）：
```json
{
  "security": {
    "enabled": false
  }
}
```
这会移除所有安全检查，Bot 可以访问整个系统。

---

## 与 Go 版本的对应关系

本项目是从 Go 版本 1:1 复刻。主要映射：

| Go | Rust |
|---|---|
| `module/bus/` | `crates/nemesis-bus/` |
| `module/agent/loop.go` | `crates/nemesis-agent/src/loop.rs` |
| `module/channels/manager.go` | `crates/nemesis-channels/src/manager.rs` |
| `module/cluster/` | `crates/nemesis-cluster/` |
| `module/security/` | `crates/nemesis-security/` |
| `module/forge/` | `crates/nemesis-forge/` |
| `module/services/` | `crates/nemesis-services/` |
| `module/desktop/` | `crates/nemesis-desktop/` + `plugins/plugin-ui/`（Linux 使用 GTK + libayatana-appindicator3） |
| `module/web/` | `crates/nemesis-web/` + `web/`（Vue 前端） |
| `web/static/index.html`（Alpine.js） | `web/`（Vue 3 + Vite MPA） |
| `web/static/js/` | `web/src/`（Vue 组件 + TypeScript） |
| `web/static/css/` | `web/src/styles/` |
| `nemesisbot/main.go` | `nemesisbot/src/main.rs` |
| `nemesisbot/command/` | `nemesisbot/src/commands/` |
| `go test ./module/...` | `cargo test -p <crate>` |
| `build.bat` (ldflags) | `scripts/build-windows.bat` / `scripts/build-linux.sh` (env vars) |
| `test/TestAIServer/` | `test-tools/TestAIServer/` (Go 项目，直接复用) |
| `test/mcp/` | `test-tools/mcp/` |

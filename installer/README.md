# NemesisBot 自研安装器（installer/）

自包含的安装器独立 workspace：Windows + Linux 双平台安装包（NBSETUP1 单文件格式），
零外部安装框架依赖（无 NSIS / WiX / makeself），打包工具、验签、释放引擎全部自研。
总控报告：`docs/REPORT/2026-11-09_自研安装器交付报告.md`（原计划已完成归档，
原件已删；2026-11-10 系统集成收编 bot 后的分工见下方「系统集成分工」）。

## 隔离契约（用户拍板「就是要隔离的，不能耦合」）

- **独立 Cargo workspace**：本目录【不是】根 workspace 成员——member 会在根
  `Cargo.toml` 留引用，删目录 = 全仓 cargo 命令报错。先例：`plugins/plugin-wasm`。
- **依赖方向单向**：`core → ../../crates/nemesis-verify`（path dep 只读外部源码）；
  `windows/linux → core`。外部项目对本目录**零反向引用**。
- **整体可删**：删除 `installer/` 目录对外部仓库零影响（根锁零污染——egui/ruzstd
  等依赖只进本目录 `Cargo.lock`）。
- **产物自留**：构建脚本产物全部落 `installer/dist/`（已 gitignore）。

## 系统集成分工（2026-11-10 收编 bot，用户拍板「安装器搬文件、bot 做集成」）

快捷方式（开始菜单/桌面 .lnk）、ARP 卸载面板、开机自启（HKCU Run / systemd
user unit）、Linux 三件套（符号链接 / XDG .desktop / PATH 哨兵）、根证书库——
这些系统集成的**唯一实现住 bot**（`crates/nemesis-sysint`，根 workspace 成员，
CLI 入口 `nemesisbot system register/unregister/status/cert`）。安装器**零第二
实现**，只在文件落位后 spawn bot、卸载前 spawn 注销：

```
安装链：… 引擎原子提交 → spawn <dir>/nemesisbot system register --dir <dir> …
卸载链：spawn <dir>/nemesisbot system unregister --dir <dir>（bot 还在盘上）
        → 引擎按 install.json 删文件 → 卸载副本摘除 / 自删
```

- **恒 spawn**：`--no-shortcuts` 等旗标透传给 bot，各项注册与否由 bot 侧裁决；
- **失败降级**：spawn 非零退出 / spawn 失败 = 注记（notes）附 stderr 尾巴，
  绝不回滚已落位产物、绝不炸安装（快捷方式缺席是诚实留白）；
- **顺序即契约**：注销必须先于引擎删除（删完再调就是给死二进制发指令），
  测试注入执行器时断言 unregister 调用时 install.json 仍在场；
- **证书独立**：`system cert` 不在 register/unregister 流程内，默认永不安装
  （详见 bot 侧 `system cert install/remove/status`）。

## 目录结构

```
installer/
├── Cargo.toml            # 独立 workspace（members: core/windows/linux）
├── build-windows.bat     # Windows 一键构建（bot → 安装器 → 打包 → dist/）
├── build-linux.sh        # Linux 一键构建（同形态）
├── core/                 # 共享核心（setup_core lib + pack 打包工具 bin）
│   └── src/
│       ├── format.rs     # NBSETUP1 布局 + 56B 尾注（魔数 NBSETUP1）
│       ├── pkgfile.rs    # 读包/检视/区域定位/路径清洗/完整性
│       ├── manifest.rs   # manifest blob（JSON：版本/平台/文件清单+sha256+mode）
│       ├── sig.rs        # v4 验签包装 → 绿/黄/红三色分级
│       ├── install.rs    # 安装引擎：版本裁决 → staging → 原子提交 → 卸载
│       ├── fsutil.rs     # 韧性文件操作（删除重试/剪空目录）
│       ├── procs.rs      # 路径级进程强停编排（scope 过滤 + kill + 复核重查）
│       ├── console.rs    # Windows 控制台回接（AttachConsole；GUI 子系统旗标面）
│       ├── version.rs    # 版本比较（降级裁决）
│       └── bin/pack.rs   # 打包 CLI：keygen / pack / inspect / sign
├── windows/              # Windows 安装器（setup-windows，bin: nbsetup-win.exe）
│   └── src/{main.rs, gui.rs, ops.rs, ops/tests.rs}
└── linux/                # Linux 安装器（setup-linux，bin: nbsetup-linux）
    └── src/{main.rs, ops.rs, ops/tests.rs}
```

系统集成实现（`nemesisbot system …` 的真实执行体）在主仓
`crates/nemesis-sysint/`，不在本目录。

## NBSETUP1 包格式（单文件安装包）

```
[骨架: 安装器自身字节 (0, payload_offset)]
[zstd 压缩的 tar 载荷 (payload_offset, +payload_len)]     ← sha256 对 manifest
[manifest blob（JSON，可签）]
[56B 尾注: 各段长度 + 版本 + 魔数 "NBSETUP1"]              ← 尾部反向扫描 + 位置交叉校验
```

- 安装包本体 = 「骨架 + 尾部数据」：直接运行即安装器（读自身文件定位载荷）；
- 打包排除编译中间物（pdb/lib/exp/ilk/obj）、测试工具目录（`tests/`——构建脚本
  stage 进 `bin/bin_*/` 供开发流取用，发行包不携带）、皮肤目录（`skins/`——
  支持在线下载，发行包不预置）与桌面/编辑器杂物（Thumbs.db/desktop.ini/
  .DS_Store/.gitignore）；必需文件缺席 loud 拒绝打包（windows 必须有
  `nemesisbot.exe` / linux 必须有 `nemesisbot`）；
- 文件收集显式排序（NTFS/ext4 目录序是平台实现细节）。

## 快速开始

```bat
:: Windows（本机开发机）
cd installer
build-windows.bat                          :: 全量（主程序 + 安装器 + 打包）
build-windows.bat --skip-bot               :: 复用既有 bin\bin_windows
build-windows.bat --skip-bot --sign keys.json --anchor <root_anchor_hex>

:: 产物
dist\NemesisBotSetup-<版本>-x64.exe        :: 双击 = GUI 向导；--silent = 静默
```

```bash
# Linux
cd installer
./build-linux.sh                           # 产物 dist/nemesisbot-setup-<ver>-x64.run
```

版本号与主仓构建脚本同源（`git describe --tags --abbrev=0`，缺省 `0.0.0.1`）。

## CI 产物（nightly release，2026-11-11 接入）

`daily-release.yml` 在 build job 双平台腿各产出一个安装包（**纯新增**，压缩包
三主包与全部附属包不变），与本地脚本同一打包/签名链：

| 资产 | 载荷 | 骨架构建 |
|------|------|----------|
| `nightly-windows-setup.exe` | CI `staging/`（已 v4 签名的二进制 + plugins + certs/root_cert.der + signatures.json + version.txt + share/icon.png） | `-p setup-windows -p setup-core` |
| `nightly-linux-setup.run` | 同上（Linux 形态） | `-p setup-linux -p setup-core`（setup-windows 是 win32 依赖，Linux 全量编必炸） |

- 编排脚本：`scripts/ci-sign-installer.sh`（打包 + 两层签名 + 检视 + 外壳验签
  fail-the-build），镜像 `ci-sign-artifacts.sh` 纪律（同一 NMB_ISSUING_* Secrets
  → 同一 mint-leaf 当日叶 → 临时 keys.json 用完即删；Secrets 缺席 = 未签黄色包
  照常产出，fork 零门槛）；
- 信任锚：骨架构建自动吃 `NEMESIS_BUILD_ROOT_ANCHOR`（GITHUB_ENV 注入，与 bot
  二进制同机制同值）；
- 载荷里二进制的 v4 签名不受整包外壳签名影响（容器签 ≠ 文件签，释放按 manifest
  逐字节还原，装完盘上文件验签 Valid）；
- 版本 = `git describe --tags --abbrev=0`（可溯源，与本地同源）。


## 签名工作流（v4 信任链，与主仓签名体系同源）

两层独立信任链（计划 §4/§4a）：**manifest blob 签名**（防载荷被换，`pack --sign`
产出，`inspect` 分级消费）+ **整包外壳签名**（防安装器程序本体被改，`pack sign`
产出，安装器启动外壳自检消费）。

```bash
# ① 生成密钥体系（keys.json + root anchor）
./target/release/pack keygen --out keys.json     # 打印 root_anchor hex

# ② 打包时签 manifest blob（载荷防换；缺省不签 = 黄色提示形态）
./target/release/pack pack --payload ../bin/bin_windows \
    --skeleton target/release/nbsetup-win.exe \
    --out dist/NemesisBotSetup.exe --version 1.2.0 --platform windows \
    --sign keys.json

# ③ 整包外壳签名（安装器启动自检绿级；build 脚本 --sign 时自动执行此步）
./target/release/pack sign dist/NemesisBotSetup.exe --keys keys.json

# ④ 运行侧信任锚（二选一）
#    编译期：build 脚本 --anchor <hex>（NEMESIS_BUILD_ROOT_ANCHOR 固化进二进制）
#    运行期：安装器环境带 NEMESIS_ROOT_ANCHOR=<hex>
#    两者皆缺 = 空锚集 → 一切签名判 Untrusted（红色级）
```

安装器启动/install 时的两层自检：**外壳自检**（验自身 exe 字节，PE 证书表 /
ELF v4 footer 自动分派）与 **manifest 分级**（inspect）各跑各的；任一层红色级
都需要知情确认（GUI 勾选 / 静默 `--accept-signature-risk`，缺 = 退出码 5）。

## 安装器使用

| 形态 | Windows | Linux |
|------|---------|-------|
| 交互 | 双击（egui 图形向导：目录选择/签名横幅/进度/完成页） | 终端问答向导（缺省值回车确认） |
| 静默 | `--silent [--dir D] [--autostart] [--launch] ...` | 同左 |
| 卸载 | 安装目录内 `uninstall.exe`（GUI 确认 = 路径级强停后卸载；`--uninstall --dir D` = 静默闸形态） | 安装目录内 `uninstall`（终端确认 = 路径级强停；`--uninstall` = 静默闸） |

公共旗标（两平台一致）：

```
--silent                    静默安装
--uninstall                 卸载模式（缺省按自身所在目录；--dir 可指定）
--dir <DIR>                 安装目录（缺省 %LOCALAPPDATA%\NemesisBot /
                            ~/.local/share/NemesisBot，尊重 XDG_DATA_HOME）
--autostart                 开机自启（HKCU Run / systemd user unit，best-effort）
--launch                    装完启动 Dashboard
--accept-signature-risk     签名红色级知情继续（缺 = 退出码 5）
--force                     允许版本降级（缺 = 拒绝，退出码 1）
--no-shortcuts              跳过快捷方式三件套（旗标透传 bot，bot 侧裁决：
                            Windows=开始菜单/桌面 .lnk；Linux=symlink/.desktop/PATH）
--desktop-shortcut          额外建桌面快捷方式（Windows 透传 --desktop；
                            Linux 平台为 no-op 接受）
--package <FILE>            安装包路径（缺省 = 本可执行文件自身）
```

退出码契约（`ops::exit_code_of` 单点映射，两平台一致）：

| 码 | 含义 |
|----|------|
| 0 | 成功 |
| 1 | 通用失败（含降级被拒） |
| 2 | 完整性校验失败（包损坏/篡改，不可越过） |
| 3 | 用户取消 |
| 4 | 磁盘空间不足（载荷原大 × 1.2 余量） |
| 5 | 签名红色级且未经知情确认 |
| 6 | bot 正在运行（静默安装/卸载闸拒绝；确认卸载强停后仍有存活同域） |

### 安装语义要点

- **staging 先行**：全部释放进 `<target>/.nbsetup-stage-*`，逐文件 sha256 校验全过
  后原子 rename 提交；中途失败 = 目标目录零污染（本次新建的目录一并摘除）；
- **install.json 最后落盘**：提交成功哨兵 + 卸载清单真相源（无标记 = 下次按全新装）；
- **升级跳过 `workspace/` 前缀**（v1 载荷不含 workspace——用户数据住产品默认 home
  `~/.nemesisbot`，安装器永不触碰；此规则是载荷未来带回 workspace 模板时的前向保险）；
- **运行实例闸**：安装/卸载前检测运行中的 nemesisbot（tasklist / pgrep），有实例
  = 拒绝（退出码 6）——防运行中文件被换；
- **卸载双形态**（2026-11-10 用户拍板）：
  - **静默 `--uninstall`（含 ARP 面板路径）**：保持全局实例闸——任一 nemesisbot
    进程在场 = 拒绝（退出码 6），脚本可消费的诚实契约；
  - **向导确认卸载**（GUI 确认框 / 终端 `y` 确认）：**路径级强制停止**——枚举
    全部 nemesisbot 进程取镜像路径（Windows Toolhelp+QueryFullProcessImageNameW /
    Linux `/proc/<pid>/exe` readlink），只杀落在本安装目录子树内的实例
    （kill 10×300ms 复核重查，仍存活 = 退出码 6 不动文件）；**他处部署/开发
    checkout 的同名进程绝不误杀**。确认链顺序 = 强停 → spawn unregister →
    引擎删除 → 副本摘除；
- **系统集成 = spawn bot**（2026-11-10 收编，见「系统集成分工」）：快捷方式/ARP/
  自启/Linux 三件套全部由 `nemesisbot system register` 执行，安装器恒 spawn 并把
  失败降为注记；
- **卸载**：先 spawn `system unregister`（bot 还在盘上——顺序即契约）→ 按
  install.json 清单韧性删除 + 剪空目录；Windows `uninstall.exe` 副本延迟自删
  （`del` + `rd` 空目录）；Linux 显式摘 `uninstall` 副本；**PATH 注记按 bot 设计
  刻意保留**（自动摘 ~/.profile 有误伤面，哨兵块无副作用）。

## 测试

```bash
cd installer
cargo test                              # 三 crate 全量（Windows 主机 95 用例 =
                                        # core 64 + linux 跨平台 10 + windows 21；
                                        # Linux 对端 78 = core 61 + linux 17，
                                        # unix 门控 3 例与 linux-only 7 例在对端）
cargo fmt --all -- --check
cargo clippy --tests                    # 0 warnings 基线
bash scripts/check-inline-tests.sh      # 仓库根执行：内联测试纪律
```

⚠️ Linux 主机注意：workspace 三成员里 `setup-windows` 是 win32 依赖（winreg/
windows-sys/egui win32），Linux 上全量 `cargo build`/`cargo test` 会在该成员
编译失败（预期形态，非损坏）——Linux 侧一律定 Target：`cargo build/test -p
setup-linux -p setup-core`（build-linux.sh 已按此形态构建；注意 pack 是
setup-core 包的 [[bin]]，`-p pack` 不存在——包名解析失败实录）。

- **core（64，含 unix 门控 3 在对端）**：格式/尾注往返、manifest 校验、路径清洗、完整性篡改检测、
  版本裁决四分支、staging 零残留、升级跳 workspace、卸载清单——含**真实往返**
  （构造包 → 释放 → 逐字节比对）；异常面：载荷翻 1 字节 = Integrity fail-closed
  目标零落盘；**提交窗口中途失败**（升级撞独占锁定目标，win32 share_mode 真实
  锁）= staging 清零 + 旧 install.json 原样 + 被锁文件字节未破坏 + 重试自愈
  （锁释放后重装成功账本翻新）；unix 执行位回归 ×2（pack 输出/外壳签名保位、
  卸载副本 755）；**强停编排**（procs）：子树 scope 边界（兄弟目录/祖先/不可读
  路径不归属）、Windows 大小写+尾分隔符归一、只杀 scope 内 PID、无目标不杀、
  存活报 `BotsSurvive`、复核重查收敛；
- **windows（21）**：退出码映射（含 BotsSurvive→6）、磁盘余量、tasklist 解析、spawn 契约
  （SystemCall 形状对齐 bot CLI 旗标）、假执行器全生命周期（成功/闸拦/失败降级
  + **注销顺序契约**）、spawn 失败降级注记、`/S` 精确令牌矩阵（`/sILENT` 不触发）、
  旗标解析矩阵；**活体强停**（Toolhelp 真枚举 + taskkill 真杀 ping.exe 副本，
  快照可见性轮询防竞态）；确认卸载链注入测试（强停先于注销、存活拦停整链零删除）；
- **linux（17；Windows 主机跑跨平台 10，IO 面 7 只在 Linux）**：纯构造器与 spawn 契约跨平台可跑；
  IO 面（statvfs/pgrep//proc 扫描/SIGKILL 活体/真实 spawn 链）`cfg(target_os)` 只在
  Linux 编译执行（远程盒/CI 覆盖，Windows 上诚实缺席）；确认卸载链注入测试 +
  活体强停（单用途二进制副本改名——多调用二进制按 argv[0] 分派拒跑，优先
  python3、sleep 兜底、立即退出诚实 SKIP）。

系统集成（nemesis-sysint）的测试在主仓该 crate 内（内容构造器跨平台 +
Windows 注册表/证书文件层 + Linux facade 全链路，见主仓 README/CLAUDE.md）。

## 诚实边界

- **两层签名可独立缺省**：只跑 `pack --sign`（manifest blob）不跑 `pack sign`
  （整包）= inspect 绿但外壳自检黄（安装器本体未签）；反之亦然。build 脚本
  `--sign` 两步都做；
- **卸载副本未签名**：uninstall.exe = 骨架字节段副本，签名在包尾 EOF 区不随副本
  走——副本运行时外壳自检黄色「无签名」，属预期形态；
- **外壳自检的绿级需要信任锚**：整包已签 + 锚缺席（两个 env 都没配）= Untrusted
  红级——锚配置属部署配置（见签名工作流 ④）；
- **载荷篡改的拦截在安装引擎不在 inspect**：inspect 只验 manifest blob（快速，
  不读载荷）；payload sha256 对 manifest 在释放前硬校验（退出码 2，不可越过）；
  整包外壳签名（`pack sign`）覆盖载荷区，红级由启动自检拦截；
- **GUI 子系统 + 控制台回接**：release 骨架 = `windows_subsystem="windows"`
  （双击零参数**无控制台黑框**，2026-11-10 用户诉求）；`--silent`/`--uninstall`/
  `--help` 在 main 最前面 `AttachConsole(ATTACH_PARENT_PROCESS)` 回接父控制台，
  stdout/stderr 与退出码契约不变（重定向形态不需要 attach——句柄经
  STARTUPINFO 直传；父无控制台 = attach 静默失败，exit code 仍可脚本消费）。
  debug 构建保持控制台子系统（开发态 println 可见）；
- **静默卸载保持全局闸**（2026-11-10 用户拍板的分工）：`--uninstall` 与 ARP 面板
  UninstallString 走静默形态——任一 nemesisbot 进程在场（含他处部署/开发
  checkout）= 退出码 6 拒绝；只有向导确认卸载走路径级强停。手工卸载他处部署
  需先停实例（或改走 `--dir` 指定 + 先手动停该实例）；
- **路径级强停的可见性边界**：镜像路径取不到的进程（Windows 跨界提权 /
  Linux 他用户）按「不归属任何目录」处理——不可见即不可杀，其残留由用户
  手工处置；Linux 不杀进程组（pgid 可能与用户 shell 作业共享）——bot 子进程
  若也是 nemesisbot 形态由查询面覆盖，异名子进程依赖 tree-kill（Windows /T）；
- **用户数据不归安装器管**：`~/.nemesisbot` 产品数据目录永不触碰（卸载只删
  安装目录清单内文件）；
- **系统集成失败不回滚**：spawn `system register/unregister` 失败（bot 缺席/
  崩溃/退出非零）= 注记留白继续——已落位文件绝不因集成失败撤销；代价是
  快捷方式/ARP 可能缺席（`system status` 可事后核查补注册）；
- **Linux 卸载保留 PATH 注记**（bot 设计）：`~/.profile` 哨兵块不自动摘除；
- **Linux IO 测试需 Linux**：符号链接/systemctl/pgrep/statvfs 用例只在 Linux
  编译执行（远程盒/CI 覆盖；Windows 开发机上诚实缺席）；
- **GUI 向导无自动化测试**：egui 交互流程靠形态拆分（ops 链全量单测 + GUI 薄壳）
  覆盖；headless 环境无法驱动；
- **红级确认继续 = 残余风险面**：攻击者可改验证逻辑重打包；纯软件无内核强制下
  不存在绝对拦截（与签名体系既有声明一致）；
- **v1 不做 CRL 联网拉取**（安装场景离线优先）；Revoked 态当前管线不可达，
  保留变体仅为完备。

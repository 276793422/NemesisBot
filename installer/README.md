# NemesisBot 自研安装器（installer/）

自包含的安装器独立 workspace：Windows + Linux 双平台安装包（NBSETUP1 单文件格式），
零外部安装框架依赖（无 NSIS / WiX / makeself），打包工具、验签、释放引擎全部自研。
总控计划：`docs/PLAN/2026-11-09_自研安装器计划.md`。

## 隔离契约（用户拍板「就是要隔离的，不能耦合」）

- **独立 Cargo workspace**：本目录【不是】根 workspace 成员——member 会在根
  `Cargo.toml` 留引用，删目录 = 全仓 cargo 命令报错。先例：`plugins/plugin-wasm`。
- **依赖方向单向**：`core → ../../crates/nemesis-verify`（path dep 只读外部源码）；
  `windows/linux → core`。外部项目对本目录**零反向引用**。
- **整体可删**：删除 `installer/` 目录对外部仓库零影响（根锁零污染——egui/ruzstd
  等依赖只进本目录 `Cargo.lock`）。
- **产物自留**：构建脚本产物全部落 `installer/dist/`（已 gitignore）。

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
│       ├── version.rs    # 版本比较（降级裁决）
│       └── bin/pack.rs   # 打包 CLI：keygen / pack / inspect / sign
├── windows/              # Windows 安装器（setup-windows，bin: nbsetup-win.exe）
│   └── src/{main.rs, gui.rs, ops.rs, ops/tests.rs}
└── linux/                # Linux 安装器（setup-linux，bin: nbsetup-linux）
    └── src/{main.rs, ops.rs, ops/tests.rs}
```

## NBSETUP1 包格式（单文件安装包）

```
[骨架: 安装器自身字节 (0, payload_offset)]
[zstd 压缩的 tar 载荷 (payload_offset, +payload_len)]     ← sha256 对 manifest
[manifest blob（JSON，可签）]
[56B 尾注: 各段长度 + 版本 + 魔数 "NBSETUP1"]              ← 尾部反向扫描 + 位置交叉校验
```

- 安装包本体 = 「骨架 + 尾部数据」：直接运行即安装器（读自身文件定位载荷）；
- 打包排除编译中间物（pdb/lib/exp/ilk/obj）；必需文件缺席 loud 拒绝打包
  （windows 必须有 `nemesisbot.exe` / linux 必须有 `nemesisbot`）；
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
| 卸载 | 安装目录内 `uninstall.exe`（或 `--uninstall --dir D`） | 安装目录内 `uninstall` |

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
--no-shortcuts              不建快捷方式（开始菜单/.lnk；符号链接/.desktop/PATH 注记）
--desktop-shortcut          额外建桌面快捷方式（Linux 平台为 no-op 接受）
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
| 6 | 检测到 bot 正在运行（安装/卸载都拒绝） |

### 安装语义要点

- **staging 先行**：全部释放进 `<target>/.nbsetup-stage-*`，逐文件 sha256 校验全过
  后原子 rename 提交；中途失败 = 目标目录零污染（本次新建的目录一并摘除）；
- **install.json 最后落盘**：提交成功哨兵 + 卸载清单真相源（无标记 = 下次按全新装）；
- **升级跳过 `workspace/` 前缀**（v1 载荷不含 workspace——用户数据住产品默认 home
  `~/.nemesisbot`，安装器永不触碰；此规则是载荷未来带回 workspace 模板时的前向保险）；
- **运行实例闸**：安装/卸载前检测运行中的 nemesisbot（tasklist / pgrep），有实例
  = 拒绝（退出码 6）——防运行中文件被换；
- **快捷方式**：Windows = 开始菜单 .lnk（恒建）+ 桌面 .lnk（可选），参数固定
  `dashboard`；Linux = `~/.local/bin/nemesisbot` 符号链接 + XDG `.desktop` +
  `~/.profile` PATH 注记（哨兵注释块，幂等）；
- **ARP 注册表**：Windows 写 HKCU 卸载面板键（控制面板「应用」可见，零 UAC）；
- **卸载**：按 install.json 清单韧性删除 + 剪空目录 + 摘注册表/快捷方式；
  Windows `uninstall.exe` 副本延迟自删（`del` + `rd` 空目录）；Linux 显式摘
  `uninstall` 副本；**PATH 注记刻意保留**（自动摘 ~/.profile 有误伤面，哨兵块无副作用）。

## 测试

```bash
cd installer
cargo test                              # 三 crate 全量（81 用例）
cargo fmt --all -- --check
cargo clippy --tests                    # 0 warnings 基线
bash scripts/check-inline-tests.sh      # 仓库根执行：内联测试纪律
```

- **core（53）**：格式/尾注往返、manifest 校验、路径清洗、完整性篡改检测、
  版本裁决四分支、staging 零残留、升级跳 workspace、卸载清单——含**真实往返**
  （构造包 → 释放 → 逐字节比对）；
- **windows（15）**：退出码映射、磁盘余量、ps 转义、真实 PowerShell .lnk 创建、
  HKCU 注册表往返（测试键位，自清理）、`/S` 精确令牌矩阵（`/sILENT` 不触发）、
  旗标解析矩阵；
- **linux（13）**：纯内容构造器（.desktop/systemd unit/PATH 片段/shell 转义）
  跨平台可跑；IO 面（statvfs/符号链接幂等/XDG 落盘/pgrep）`cfg(target_os)`
  只在 Linux 编译执行（Windows 上诚实缺席）。

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
- **控制台子系统**：nbsetup-win.exe 未设 `windows_subsystem="windows"`（双击时
  GUI 背后有瞬间控制台；保留便于静默形态 stdout 摘要与排障）；
- **用户数据不归安装器管**：`~/.nemesisbot` 产品数据目录永不触碰（卸载只删
  安装目录清单内文件）；
- **Linux 卸载保留 PATH 注记**：`~/.profile` 哨兵块不自动摘除；
- **Linux IO 测试需 Linux**：符号链接/systemctl/pgrep/statvfs 用例只在 Linux
  编译执行（远程盒/CI 覆盖；Windows 开发机上诚实缺席）；
- **GUI 向导无自动化测试**：egui 交互流程靠形态拆分（ops 链全量单测 + GUI 薄壳）
  覆盖；headless 环境无法驱动；
- **红级确认继续 = 残余风险面**：攻击者可改验证逻辑重打包；纯软件无内核强制下
  不存在绝对拦截（与签名体系既有声明一致）；
- **v1 不做 CRL 联网拉取**（安装场景离线优先）；Revoked 态当前管线不可达，
  保留变体仅为完备。

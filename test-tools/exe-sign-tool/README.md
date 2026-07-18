# exe-sign-tool

可执行文件**签名、验证工具**（PE / ELF / Raw）。用 Ed25519 给二进制打签名、ChaCha20-Poly1305 加密签名块（envelope），被保护程序内置公钥即可自验完整性（防篡改 / 防替换 / 抬高攻击成本）。**非 Authenticode**——仅服务于"程序验自己"，不追求系统级识别。

自包含独立工具（lib `exe_sign_tool` + bin `exe-sign-tool`），不依赖 nemesis-security。

---

## 功能特性

- 支持 **PE（Windows）/ ELF（Linux、Android）/ Raw（裸机、固件、任意 blob）** 三种格式
- **多签名叠加兼容**：Authenticode + 本工具签名 + 他人签名可共存，互不影响
- ChaCha20-Poly1305 加密 envelope + Ed25519 签名（双关抬攻击成本）
- 格式感知哈希：PE 排除 CheckSum / Security 目录项；ELF 多源综合算 L
- 验证导出明确状态（`Valid` / `NoSignature` / `Tampered` / `SignatureInvalid` / `UnsupportedVersion` / `Malformed`），**处置策略由调用方决定**

---

## 编译

```sh
cargo build -p exe-sign-tool            # debug
cargo build --release -p exe-sign-tool  # release（推荐；产物在 target/release/）
```

---

## 子命令

### `keygen` — 生成密钥

```sh
exe-sign-tool keygen [--out <dir>]   # --out 缺省 ./keys
```

输出 4 个文件：

| 文件 | 内容 | 机密性 |
|---|---|---|
| `exe_sign.key` | Ed25519 私钥（hex，32B） | 🔐 机密 |
| `exe_sign.pub` | Ed25519 公钥（hex，32B） | 🌍 可公开 |
| `exe_sign.sym` | ChaCha20 对称密钥（hex，32B） | 🔐 机密（sign/verify 共用） |
| `exe_sign.meta.json` | 元信息（算法/时间/公钥） | 🌍 可公开 |

> `meta.json` 故意不含机密字段（私钥与 sym 各自独立文件保管）。

### `sign` — 加签名

```sh
exe-sign-tool sign <exe> [--key-dir <dir>] [--key <keyfile>] [--sym <symfile>] [--signed-at <unix>] [--key-id <n>]
```

`<exe>` 是**位置参数**（不用 `--file`）。`--key-dir`（默认 `./keys`）自动找 `exe_sign.key` + `exe_sign.sym`；`--key`/`--sym` 可选覆盖。原地追加 **4KB envelope** 到文件末尾。

### `verify` — 验证 / 检测

```sh
exe-sign-tool verify <exe> [--key-dir <dir>] [--pub-hex <hex> | --pub-file <pubfile>] [--sym <symfile>]
```

`<exe>` 位置参数；`--key-dir`（默认 `./keys`）自动找 `exe_sign.pub` + `exe_sign.sym`。`Valid` 退出码 `0`，其余 `1`。

---

## 快速开始（一键脚本）

本目录下的 `quick-sign.bat`（Windows）/ `quick-sign.sh`（Linux、git-bash）：给指定程序加签名并自动验证检测。

```sh
# Windows（cmd）
test-tools\exe-sign-tool\quick-sign.bat C:\path\to\your.exe

# Linux / git-bash
test-tools/exe-sign-tool/quick-sign.sh /path/to/your.elf
```

脚本自动：① 首次运行 `keygen` 生成密钥（已存在则复用）② `sign` ③ `verify` 检测。密钥保存在脚本同目录的 `keys/` 子目录。

---

## 完整手动流程

```sh
# 1. 生成密钥（仅首次；默认输出到 ./keys）
exe-sign-tool keygen

# 2. 给程序加签名（默认从 ./keys 读 key+sym）
exe-sign-tool sign ./myapp.exe

# 3. 验证（默认从 ./keys 读 pub+sym）
exe-sign-tool verify ./myapp.exe
# → Valid (exit 0)；若文件被篡改 → Tampered / SignatureInvalid (exit 1)
```

### 篡改检测示例

```sh
# 签名后翻转一字节，再验证 → 失败
cp myapp.exe myapp.copy.exe
exe-sign-tool sign myapp.copy.exe
printf '\xff' | dd of=myapp.copy.exe bs=1 seek=200 count=1 conv=notrunc  # 改第 200 字节
exe-sign-tool verify myapp.copy.exe
# → Tampered(...)，exit 1
```

---

## 工作原理（简述）

1. **codec 多态**：按魔数选 `PeCodec`（`MZ`）/ `ElfCodec`（`\x7fELF`）/ `RawCodec`（兜底）
2. **多源综合算原始内容末尾 L**：PE = `max(section raw end)` + Certificate Table（`DataDirectory[4]`）交叉校验；ELF = `max(PT_LOAD, section header table)`；Raw 用 envelope 的 content_len
3. **content_hash** = SHA-256(`[0,L)`)，PE 额外排除 CheckSum + Security 目录项
4. **signed_meta**（含 content_hash + 元数据）→ Ed25519 签名
5. **envelope body** 用 ChaCha20-Poly1305 加密（AAD = footer`[0..36]`）
6. **明文 footer**（64B 锚点）+ 密文 body + padding，对齐 4KB 追加到 EOF
7. **verify**：定位 envelope → AEAD 解密 → 重算 content_hash → Ed25519 验签

详见 `docs/PLAN/2026-07-18_exe-self-signature.md`。

---

## 安全模型

- **固定内置 Ed25519 公钥** = 真正认证锚点。envelope 的 `key_fp` / `key_id` 仅作提示，**不作安全决策**。
- **SYM_KEY**（ChaCha20）= obscurity + AEAD 完整性层（内置可被静态逆向，抬高成本），非密码学强保护。
- **三态处置**：模块只返回验证状态，拒绝 / 警告 / 放行由调用方决定（建议"本该签名的程序，无签名也拒"）。
- **软件自检局限**：能防"被动篡改" + 抬高成本，但**不防主动 patch 绕过**（攻击者控制运行环境可直接 patch 掉自检调用）。防后者需 secure boot / TPM / 外部加载器预验。

---

## 限制

- macOS **Mach-O 暂不支持**（codesign 冲突 + page 对齐 + Apple Silicon 强制签名），后续阶段。
- **Raw 格式** envelope 必须在文件末尾，不支持之后追加签名（裸机场景无此需求）。
- `BUILTIN_SYM_KEY_DEFAULT` 是占位全 0，**发布前用 `keygen` 生成的 `exe_sign.sym` 替换**并固化进被保护程序。

---

## 退出码

| 命令 | `0` | `1` |
|---|---|---|
| `keygen` | 成功 | IO 错误 |
| `sign` | 签名成功 | 文件已签名 / IO 错误 |
| `verify` | `Valid` | `NoSignature` / `Tampered` / `SignatureInvalid` / ... |

---

## 相关文档

- 预研：`docs/PLAN/2026-07-18_exe-self-signature.md`
- 开发计划：`docs/PLAN/2026-07-18_exe-self-signature_开发计划.md`
- 最终报告：`docs/REPORT/2026-07-18_exe-self-signature_最终报告.md`
- 项目总览：根目录 `CLAUDE.md` 的 `exe-sign-tool` 小节

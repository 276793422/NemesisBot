# 第三方组件与许可证清单（THIRDPARTY）

> 本文件列出 NemesisBot 使用或分发的第三方组件及其许可证，用于满足上游许可证的署名/保留义务，并供二次分发者核查兼容性。本清单初版基于 2026-07-04 的依赖审计；最近一次全量复核于 **2026-10-22**（`Cargo.lock` 共 992 条 name-version 对，其中 70 条为本仓 workspace 成员，其余 922 条第三方条目全部核对：921 条直读本地 registry 许可证元数据，1 条 `webpki-root-certs` 本地未解包、经上游 .crate 清单核对）。后续依赖更新应以 `cargo license`、`go mod`、`npm license` 的实际输出为准。
>
> NemesisBot 自身以 **GNU AGPL-3.0** 发布（含版权持有人依第 7 条附加的使用限制条款，全文见 [LICENSE](LICENSE)），并提供独立的 [商业授权](COMMERCIAL_LICENSE.md)。

---

## 1. 内置资产 / 模型（须保留 LICENSE / 署名）

### 1.1 随发行包分发（编译进 static，随安装介质携带）

| 组件 | 位置 | 许可证 | 义务 |
|------|------|--------|------|
| **Inter 字体**（Regular/Medium/Bold woff2 子集） | `web/public/fonts/` | **SIL OFL-1.1**（© 2016 The Inter Project Authors） | 许可全文随字体分发（`web/public/fonts/LICENSE.Inter.txt`，2026-09-07 补齐）；OFL §1 要求版权声明与许可文本随字体保留 |
| **Geist Mono 字体**（Regular woff2 子集） | `web/public/fonts/` | **SIL OFL-1.1**（© 2023 Vercel, in collaboration with basement.studio） | 许可全文随字体分发（`web/public/fonts/LICENSE.GeistMono.txt`，2026-09-07 补齐） |

### 1.2 运行时自动下载（bot 从上游官方源下载到用户机器，**不经本发行包分发**；2026-10-22 复核定性）

| 组件 | 落位 | 许可证 | 获取方式 |
|------|------|--------|----------|
| **Kokoro TTS 模型**（multi-lang v1.0 / v1.1） | `<workspace>/tools/voice/data/tts/kokoro-multi-lang-v1_*/` | **Apache-2.0** | 首次启用 TTS 时按 voice `config.toml` 的 mirror/repo 自动下载（`auto_download` 默认 true，`crates/nemesis-voice/src/config.rs`）；Apache-2.0 §4 要求保留 NOTICE（如有） |
| **eSpeak-NG 数据**（phoneme 字典） | Kokoro 包内 `espeak-ng-data/` | 随 Kokoro 分发（eSpeak-NG 本体为 GPL，此处仅数据；以 Kokoro 上游打包为准） | 随 Kokoro 模型包一起下载 |
| **CppJieba 中文分词词库** | Kokoro 包内 `dict/` | 随 Kokoro 分发 | 随 Kokoro 模型包一起下载 |
| **sherpa-onnx 运行库**（STT/TTS/VAD 共享库） | 主程序 exe 同目录（`exe_dir` = `current_exe().parent()`，`crates/nemesis-voice/src/bootstrap.rs`） | **Apache-2.0** | 首次启用语音时从 `k2-fsa/sherpa-onnx` GitHub releases 下载 + `libloading` 动态加载（FFI 结构布局对齐 v1.13.2；AEC 共享库单独落 `<workspace>/tools/voice/aec/`） |

> **注**：Kokoro 目录下的 `LICENSE` 文件为 Apache-2.0 全文，分发时不得删除。
>
> **注（2026-09-07 补充）**：内置技能 `summarize` / `skill-creator`（`nemesisbot/workspace/skills/`）的 SKILL.md 源自社区 skills 仓库，上游许可证待复核；人格库 persona 在运行时按需从 GitHub `msitarzewski/agency-agents` 仓库拉取（不经本包分发，属包管理器式按需获取）。

---

## 2. Rust 依赖（`Cargo.lock`）

审计结论：**全树无 GPL / AGPL / LGPL 强 copyleft 依赖**，与项目 AGPL-3.0 兼容。许可证分布（2026-10-22 复核，922 条第三方条目，分桶互斥、加总 = 922）：

| 许可证 | 数量 | 性质 |
|--------|------|------|
| Apache-2.0 与 MIT 双许可（含 Zlib / BSD / ISC / CC0 / LLVM-exception 组合变体） | 588 | 宽松 |
| MIT（含 MIT × Unlicense / BSD 宽松组合，不含 Apache） | 252 | 宽松 |
| Apache-2.0 单许可（无 MIT，含 WITH LLVM-exception / AND ISC） | 25 | 宽松 |
| Unicode-3.0 | 18 | 宽松 |
| BSD / ISC / Zlib / Unlicense / BSL-1.0(Boost) / CDLA-Permissive-2.0（无 MIT/Apache，含 `webpki-root-certs`） | 35 | 宽松 |
| **MPL-2.0** | **3**：`colored`、`option-ext`、`attohttpc` | 弱 copyleft，**文件级** |
| 0BSD / CC0-1.0 | 1 | 公共领域 |

**MPL-2.0 说明**：MPL-2.0 为文件级弱 copyleft。本项目**未修改**这三个 crate 的源码，仅以编译产物链接；按 MPL-2.0 §3.3，与本项目 AGPL-3.0 组合可构成"更大作品"（Larger Work），MPL 文件的源码仍可从 crates.io 公开获取，合规义务满足。如对这三个 crate 进行修改，修改后的文件须按 MPL-2.0 开源。（`attohttpc` 经 `iroh → portmapper → igd-next`（QUIC 中继的 UPnP 打洞链路）进入默认依赖树，与 `colored`/`option-ext` 同等对待。）

**关键依赖（部分列举，均宽松许可）**：`tokio`、`axum`、`reqwest`（rustls）、`serde`、`clap`、`tracing`、`ratatui`、`ed25519-dalek`/`aes-gcm`/`sha2`、`include_dir`、`tower-http`、`quinn`、`iroh`（QUIC 中继/漫游，MIT OR Apache-2.0）、`rumqttc`（MQTT 通道，Apache-2.0）、`socket2`、`dashmap`、`parking_lot`、`chrono`。

> 唯一带 "LGPL" 字样的 `r-efi`（lock 中 5.3.0 与 6.0.0 两个版本）为 `Apache-2.0 OR LGPL-2.1-or-later OR MIT` 三选一，本项目按 MIT 选项履行，无 LGPL 义务。

### 2.1 plugins/ 目录插件（非 workspace member，不在 `Cargo.lock` 审计范围；2026-10-22 补）

| 插件 | 第三方依赖 | 许可证 |
|------|-----------|--------|
| `plugin-wasm` | `wasmtime` 48 + `wasmtime-wasi`（随发行包分发 host DLL） | Apache-2.0 WITH LLVM-exception（宽松） |
| `plugin-ui` | `wry` 0.51 + `tao` 0.33 | Apache-2.0 OR MIT / Apache-2.0（tao 在 Linux 上动态链接 GTK3，归入 §5 GTK 条目） |
| `plugin-onnx` | `ort`（ONNX Runtime）+ `ndarray` + `tokenizers` | MIT / MIT OR Apache-2.0 / Apache-2.0（均宽松） |
| `plugin-eval-monitor` | 无第三方依赖（零依赖自写代码，动态 `LoadLibrary` 访问 SbieApi） | — |

---

## 3. Go 依赖（`test-tools/TestAIServer`）

仅用于测试的 AI 服务器模拟器，依赖全部宽松：

- `github.com/gin-gonic/gin` — **MIT**
- 其余间接依赖（`bytedance/sonic`、`go-playground/validator`、`protobuf`、`golang.org/x/*` 等）均 MIT / BSD-3-Clause / Apache-2.0。

---

## 4. 前端依赖（`web/package.json`）

Vue 3 + Vite 工程，全部宽松许可：

| 组件 | 许可证 |
|------|--------|
| `vue`、`vue-router`、`pinia`、`vue-echarts`、`@vue-flow/*` | MIT |
| `codemirror`、`@codemirror/*`、`thememirror` | MIT |
| `marked`、`highlight.js`（BSD-3-Clause）、`echarts`（Apache-2.0） | MIT / BSD-3 / Apache-2.0 |
| `@xterm/xterm`、`@xterm/addon-fit`（内嵌终端）、`js-yaml` | MIT |
| `dompurify` | MPL-2.0 OR Apache-2.0（宽松双选） |
| `vite`、`vitest`、`@vitejs/plugin-vue`、`vue-tsc`、`typescript`、`jsdom`、`@vue/test-utils` | MIT / Apache-2.0 |
| `@playwright/test`（devDependencies，仅 e2e 测试） | Apache-2.0 |

---

## 5. 外部服务与运行时组件（不打包进发行包）

| 组件 | 许可证 | 集成方式 |
|------|--------|----------|
| **ClamAV**（病毒扫描） | **GPL**（ClamAV 本体） | **网络协议集成**：通过 3310 端口连接用户已安装的外部 `clamd` 守护进程。本项目**不链接** libclamav、**不分发** ClamAV 二进制 → GPL 不传染本项目。若发行版额外打包 ClamAV，需另行遵守 GPL。 |
| **Sandboxie Classic**（执行体隔离沙盒，Windows） | **GPL-3.0** | **运行时按需下载**（2026-10-22 补录）：`sandbox install` 从官方 GitHub release 下载安装包（SHA-256 校验）装到用户机器 `<workspace>/tools/sandboxie/`。本项目**不修改、不捆绑、不再分发** → GPL 不传染本项目代码。若发行版额外打包 Sandboxie，需另行遵守 GPL-3.0（含源码提供义务）。 |
| **QEMU**（VM 执行体隔离后端） | **GPL-2.0 系** | **运行时按需下载**（2026-10-22 补录）：`vm install` 从官方包下载（SHA-512 校验）装到用户机器 `<workspace>/tools/vm/runtime/qemu`。同上**不捆绑、不再分发**；若发行版额外打包 QEMU，需另行遵守 GPL。 |
| **ONNX Runtime** | MIT（通过 `ort` crate，见 §2.1） | 编译期/运行时加载，MIT 宽松 |
| GTK3 / libayatana-appindicator3（Linux 系统托盘） | LGPL-2.1 / LGPL-2.0+ | 运行时 `dlopen` 系统库，LGPL 动态链接合规 |

---

## 6. 致谢

- [NemesisBot（Go 版）](https://github.com/276793422/NemesisBot_GO) — 本 Rust 版的 1:1 重写蓝本（原地址 `276793422/NemesisBot`，2026-10-12 改名后由本仓库接管）

> 仓库内未发现直接拷贝/移植的第三方项目源代码（审计于 2026-07-04）；如发现具体文件的署名缺失，请提 Issue，我们将补正。

---

## 7. 复核命令

```bash
# Rust 依赖许可证（cargo-license 若版本停滞/报错，用兜底法：对 Cargo.lock 每条
# name-version 直读 ~/.cargo/registry/src/*/<crate>-<ver>/Cargo.toml.orig 的
# [package] license 字段——2026-10-22 全量复核即用此法）
cargo install cargo-license
cargo license --avoid-dev-deps --avoid-build-deps | sort | uniq -c | sort -rn

# Go 依赖
( cd test-tools/TestAIServer && go list -m -json all )

# npm 依赖
( cd web && npx license-checker --summary ) 2>/dev/null || ( cd web && npm ls --license )
```

---

*本清单由人工 + 工具辅助整理，如有遗漏或过时，以各组件上游原始许可证为准。*

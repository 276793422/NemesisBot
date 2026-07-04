# 第三方组件与许可证清单（THIRDPARTY）

> 本文件列出 NemesisBot 使用或分发的第三方组件及其许可证，用于满足上游许可证的署名/保留义务，并供二次分发者核查兼容性。本清单基于 2026-07-04 的依赖审计生成；后续依赖更新应以 `cargo license`、`go mod`、`npm license` 的实际输出为准。
>
> NemesisBot 自身以 **GNU AGPL-3.0** 发布（见 [LICENSE](LICENSE)），并提供独立的 [商业授权](COMMERCIAL_LICENSE.md)。

---

## 1. 内置资产 / 模型（随发行包分发，须保留 LICENSE）

| 组件 | 位置 | 许可证 | 义务 |
|------|------|--------|------|
| **Kokoro TTS 模型**（multi-lang v1.0 / v1.1） | `*/tools/voice/data/tts/kokoro-multi-lang-v1_*/` | **Apache-2.0** | 保留 LICENSE 文件与版权声明；Apache-2.0 §4 要求保留 NOTICE（如有） |
| **eSpeak-NG 数据**（phoneme 字典） | `kokoro-*/espeak-ng-data/` | 随 Kokoro 分发（eSpeak-NG 本体为 GPL，此处仅数据；以 Kokoro 上游打包为准） | 跟随 Kokoro 的打包方式 |
| **CppJieba 中文分词词库** | `kokoro-*/dict/` | 随 Kokoro 分发 | 跟随 Kokoro 的打包方式 |

> **注**：Kokoro 目录下的 `LICENSE` 文件为 Apache-2.0 全文，分发时不得删除。

---

## 2. Rust 依赖（`Cargo.lock`）

审计结论：**全树无 GPL / AGPL / LGPL 强 copyleft 依赖**，与项目 AGPL-3.0 兼容。许可证分布（`cargo license`）：

| 许可证 | 大致数量 | 性质 |
|--------|----------|------|
| Apache-2.0 OR MIT | ~360 | 宽松 |
| MIT | ~177 | 宽松 |
| Apache-2.0（及 OR 变体、含 LLVM-exception / BSD / ISC / CC0 组合） | ~60 | 宽松 |
| MIT OR Unlicense / BSD-3-Clause / BSD-2-Clause / ISC / Zlib / BSL-1.0(Boost) / Unicode-3.0 / CDLA-Permissive-2.0 | ~50 | 宽松 |
| **MPL-2.0** | **2**：`colored`、`option-ext` | 弱 copyleft，**文件级** |
| 0BSD / CC0-1.0 | 少量 | 公共领域 |

**MPL-2.0 说明**：MPL-2.0 为文件级弱 copyleft。本项目**未修改**这两个 crate 的源码，仅以编译产物链接；按 MPL-2.0 §3.3，与本项目 AGPL-3.0 组合可构成"更大作品"（Larger Work），MPL 文件的源码仍可从 crates.io 公开获取，合规义务满足。如对这两个 crate 进行修改，修改后的文件须按 MPL-2.0 开源。

**关键依赖（部分列举，均宽松许可）**：`tokio`、`axum`、`reqwest`（rustls）、`serde`、`clap`、`tracing`、`ort`（ONNX Runtime，MIT）、`ratatui`、`ed25519-dalek`/`aes-gcm`/`sha2`、`include_dir`、`tower-http`、`quinn`、`socket2`、`dashmap`、`parking_lot`、`chrono`。

> 唯一带 "LGPL" 字样的 `r-efi` 为 `Apache-2.0 OR LGPL-2.1-or-later OR MIT` 三选一，本项目按 MIT 选项履行，无 LGPL 义务。

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
| `vite`、`vitest`、`@vitejs/plugin-vue`、`vue-tsc`、`typescript`、`jsdom`、`@vue/test-utils` | MIT / Apache-2.0 |

---

## 5. 外部服务（不打包，由用户自行安装）

| 组件 | 许可证 | 集成方式 |
|------|--------|----------|
| **ClamAV**（病毒扫描） | **GPL**（ClamAV 本体） | **网络协议集成**：通过 3310 端口连接用户已安装的外部 `clamd` 守护进程。本项目**不链接** libclamav、**不分发** ClamAV 二进制 → GPL 不传染本项目。若发行版额外打包 ClamAV，需另行遵守 GPL。 |
| **ONNX Runtime** | MIT（通过 `ort` crate） | 编译期/运行时加载，MIT 宽松 |
| GTK3 / libayatana-appindicator3（Linux 系统托盘） | LGPL-2.1 / LGPL-2.0+ | 运行时 `dlopen` 系统库，LGPL 动态链接合规 |

---

## 6. 致谢

本项目借鉴了以下项目的设计理念：

- [NemesisBot（Go 版）](https://github.com/276793422/NemesisBot) — 本 Rust 版的 1:1 重写蓝本
- [OpenClaw](https://github.com/openclaw/openclaw)
- [nanobot](https://github.com/HKUDS/nanobot)
- [PicoClaw](https://github.com/sipeed/picoclaw)
- [内部专项](https://github.com/RightNow-AI/内部专项)

> 仓库内未发现直接拷贝/移植的上述项目源代码（审计于 2026-07-04）；如发现具体文件的署名缺失，请提 Issue，我们将补正。

---

## 7. 复核命令

```bash
# Rust 依赖许可证
cargo install cargo-license
cargo license --avoid-dev-deps --avoid-build-deps | sort | uniq -c | sort -rn

# Go 依赖
( cd test-tools/TestAIServer && go list -m -json all )

# npm 依赖
( cd web && npx license-checker --summary ) 2>/dev/null || ( cd web && npm ls --license )
```

---

*本清单由人工 + 工具辅助整理，如有遗漏或过时，以各组件上游原始许可证为准。*

# ONNX 插件验收测试 Skill

完整的 plugin-onnx 嵌入插件验收测试流程，覆盖从构建、单元测试、集成测试到系统测试的全链路验证。

---

## 概述

此 Skill 提供完整的 plugin-onnx ONNX 嵌入插件验收测试流程。该插件使用 `ort` crate (ONNX Runtime) 和 `tokenizers` crate (HuggingFace Tokenizer) 实现本地 ML 文本嵌入，通过 C ABI DLL 与 NemesisBot 的向量存储系统集成。

### 测试层级

| 层级 | 代号 | 位置 | 内容 |
|------|------|------|------|
| P0 | 单元测试 (UT) | `plugins/plugin-onnx/src/lib.rs` | 19 个无模型测试 + 7 个需模型测试 |
| P1 | 集成测试 (IT) | `crates/nemesis-memory/src/vector/plugin_loader.rs` | 2 个 DLL 加载 + C ABI 契约测试 |
| P2 | 系统测试 (ST) | `crates/nemesis-memory/src/vector/store.rs` | 13 个场景 + 1 个持久化测试 |
| P3 | 验收测试 (UAT) | 本 Skill | 端到端人工验收 |

### 依赖条件

- **Rust 工具链**：stable Rust (1.75+)
- **网络访问**：需要访问 `hf-mirror.com` 下载测试模型（或提前准备好模型文件）
- **磁盘空间**：约 100 MB（model.onnx ~90MB + tokenizer.json ~470KB + DLL ~23MB）
- **操作系统**：Windows / Linux / macOS

---

## 阶段 1：环境准备（自动化）

### 1.1 检查 Rust 工具链

```bash
rustc --version   # 需要 1.75+
cargo --version
```

### 1.2 构建 plugin-onnx DLL

```bash
cd plugins/plugin-onnx
cargo build --release
```

**验证**：
- Windows: `target/release/plugin_onnx.dll` 存在，约 23 MB
- Linux: `target/release/libplugin_onnx.so` 存在
- macOS: `target/release/libplugin_onnx.dylib` 存在

### 1.3 下载测试模型和分词器

```bash
# Linux/macOS/Git Bash
bash plugins/plugin-onnx/scripts/setup-test.sh

# PowerShell
powershell -File plugins/plugin-onnx/scripts/setup-test.ps1
```

**验证**：
- `plugins/plugin-onnx/test-data/model.onnx` 存在，约 90 MB
- `plugins/plugin-onnx/test-data/tokenizer.json` 存在，约 470 KB

### 1.4 导出符号验证

```bash
# Windows (dumpbin)
dumpbin /exports target/release/plugin_onnx.dll | grep embed

# Linux (nm)
nm -D target/release/libplugin_onnx.so | grep embed

# macOS (nm)
nm -gU target/release/libplugin_onnx.dylib | grep embed
```

**期望输出**：三个导出符号
- `embed_init`
- `embed`
- `embed_free`

---

## 阶段 2：P0 单元测试

### 2.1 无模型单元测试（19 个）

不需要下载模型，验证基本逻辑和错误处理。

```bash
cd plugins/plugin-onnx
cargo test
```

**验证项**：
- `test_null_text_returns_error` — 空指针返回错误
- `test_embed_before_init_returns_error` — 未初始化调用 embed
- `test_init_nonexistent_model_returns_error` — 无效模型路径
- `test_init_empty_path_returns_error` — 空路径
- `test_error_code_constants` — 错误码常量
- `test_global_state_default` — 未初始化状态
- `test_free_idempotent` — 多次 free 不 panic
- `test_derive_tokenizer_path` — 分词器路径推导
- 其他 11 个边界条件测试

### 2.2 需模型单元测试（7 个）

需要 test-data 中的模型文件，标记为 `#[ignore]`。

```bash
cd plugins/plugin-onnx
cargo test -- --ignored
```

**验证项**：
- `test_init_with_valid_model` — 模型初始化成功
- `test_embed_short_text` — 短文本嵌入
- `test_embed_returns_correct_dim` — 输出维度 = 384
- `test_embed_l2_normalized` — L2 范数 ≈ 1.0
- `test_embed_deterministic` — 相同输入相同输出
- `test_init_then_free_then_embed_fails` — 生命周期
- `test_embed_multiple_texts` — 连续多次嵌入

**P0 通过标准**：全部 26 个测试通过（19 + 7）

---

## 阶段 3：P1 集成测试

测试 DLL 通过 C ABI 与 Rust consumer（`plugin_loader.rs`）的集成。

```bash
cd <project-root>
cargo test -p nemesis-memory -- --ignored --test-threads=1 plugin_loader
```

**验证项**：
- `it_real_plugin_full_lifecycle` — 加载 → 初始化 → 嵌入 → 验证 → 关闭
  - 基础嵌入功能
  - L2 归一化验证
  - 确定性验证
  - 语义相似度（"cat" vs "dog"）
  - 不同文本产生不同向量
  - 关闭后 embed 失败
- `it_real_plugin_via_boxed_trait` — 通过 `Box<dyn EmbeddingPlugin>` trait 接口测试

**P1 通过标准**：2 个测试全部通过

**注意**：必须 `--test-threads=1` 因为 ONNX Runtime 全局状态不支持并行

---

## 阶段 4：P2 系统测试

测试 VectorStore + 真实 ONNX 插件端到端功能。

```bash
cd <project-root>
cargo test -p nemesis-memory -- --ignored --test-threads=1 store::tests::st_plugin
```

**验证项（13 个场景 + 1 个持久化测试）**：

| 场景 | 描述 | 验证内容 |
|------|------|----------|
| 1 | Store 创建 | 空存储，len=0 |
| 2 | 单条目存储 | store_entry 成功，len=1 |
| 3 | 基本查询 | "feline pets" → cat 排第一 |
| 4 | 语义排序 | 编程语言 > 香蕉 |
| 5 | 相似度分数 | 0 < score ≤ 1.0 |
| 6 | 类型过滤 | 只返回 long_term 类型 |
| 7 | 查询一致性 | 同查询同结果 |
| 8 | CRUD 生命周期 | 增删改查 + 查询验证 |
| 9 | 插件 vs 本地 | 两种嵌入方法都有效 |
| 10 | 语义相似 | "automobile" ≈ "car" |
| 11 | 维度匹配 | 384 维向量正确 |
| 12 | 批量测试 | 20 条目稳定存储查询 |
| 13 | 查询稳定性 | 5 次连续查询一致 |
| 持久化 | JSONL 往返 | 存储 → 持久化 → 加载 → 查询 |

**P2 通过标准**：`st_plugin_system_test_all_scenarios` 和 `st_plugin_persistence_roundtrip` 均通过

---

## 阶段 5：P3 验收测试（手动验证）

### 5.1 构建脚本集成验证

```bash
# Windows
build.bat

# Linux/macOS
./build.sh
```

**验证**：
- `bin/nemesisbot.exe` 生成成功
- `bin/plugins/plugin_onnx.dll`（或 .so/.dylib）复制成功
- `bin/plugins/plugin_ui.dll` 也存在（不受影响）

### 5.2 主 workspace 编译无影响

```bash
cd <project-root>
cargo build --release -p nemesisbot
```

**验证**：plugin-onnx 是独立项目，不影响主 workspace 编译

### 5.3 内存安全检查

观察所有测试运行过程中：
- 无 `STATUS_ACCESS_VIOLATION`（除了已知的 ONNX Runtime 重初始化问题，已通过单线程+单生命周期规避）
- 无内存泄漏警告
- 测试进程正常退出

### 5.4 跨平台验证（如条件允许）

- [ ] Windows: 全部测试通过
- [ ] Linux: 全部测试通过（需更新路径和 DLL 后缀）
- [ ] macOS: 全部测试通过

### 5.5 性能验证

```bash
cd plugins/plugin-onnx
cargo test --release -- --ignored test_embed_multiple_texts
```

**期望**：单次嵌入 < 100ms（首次加载除外）

---

## 阶段 6：测试报告

### 通过标准

| 层级 | 测试数 | 通过要求 |
|------|--------|----------|
| P0 UT | 26 | 全部通过 |
| P1 IT | 2 | 全部通过 |
| P2 ST | 2（含 14 场景） | 全部通过 |
| P3 UAT | 5 验收项 | 全部通过 |

### 报告格式

在 `docs/REPORT/` 创建测试报告：

```markdown
# plugin-onnx 测试报告

日期：YYYY-MM-DD
环境：Windows 11 / Linux / macOS
Rust 版本：x.xx.x

## 测试结果
- P0 UT：26/26 通过
- P1 IT：2/2 通过
- P2 ST：14/14 场景通过
- P3 UAT：5/5 通过

## 构建信息
- DLL 大小：~23 MB
- 模型大小：~90 MB (all-MiniLM-L6-v2)
- 嵌入维度：384
- 推理延迟：< 100ms/次

## 已知限制
- ONNX Runtime 全局状态不支持并行测试（需 --test-threads=1）
- ONNX Runtime 不支持安全重初始化（需单生命周期测试模式）
```

---

## 清理

```bash
# 清理测试模型（如需要）
rm -rf plugins/plugin-onnx/test-data/

# 清理构建产物
cd plugins/plugin-onnx && cargo clean
```

---

## 快速参考

```bash
# 一键运行所有测试
cd plugins/plugin-onnx && cargo test && cargo test -- --ignored
cd ../.. && cargo test -p nemesis-memory -- --ignored --test-threads=1

# 构建并验证
build.bat  # Windows
./build.sh # Linux/macOS
```

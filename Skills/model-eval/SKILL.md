---
name: model-eval
description: 标准化的 LLM 模型工具调用支持评估流程。用 12 题（基础/中级/复杂）× mini/normal/big 三档 battery + 7 题探针，测一个模型在 NemesisBot agent 框架下的工具选型、schema 合规、多步链式、错误恢复能力，输出基准对比 + tier 推荐。给新模型定档、横向对比、agent 改动回归都用这个。
---

# 模型能力评估 Skill（12 题 × 3 档基准）

标准化评估一个 LLM 在 NemesisBot agent 框架下的**工具调用支持情况**。输出每档得分 + tier 推荐基准。

## 适用场景

- ✅ 接入新模型，决定给它 mini/normal/big 哪一档
- ✅ 横向对比多个模型
- ✅ agent / 工具层改动后的回归（同模型重跑，看是否退化）
- ❌ 不评估纯文本对话质量（只评工具调用 + 注入上下文利用）

## ⚠️ 两个关键前提（避坑）

1. **必须走 WebSocket**（`ws://host:port/ws`，发 `{type:"message",module:"chat",cmd:"send",data:{content}}`）。`/api/chat/stream` 是**裸 LLM 代理**（不走 agent 循环、无工具注入、无时间注入），不能用来评估。
2. **必须分批跑**（每 4 题一批、批间清 session 重启 gateway）。12 题连跑会让会话上下文累积 → 触发"Memory threshold"压缩通知混进 WS 消息流 → 回复整体错位一格、被通知占位。

## 评估流程（5 阶段）

### 阶段 1：准备

1. 模型已 `model add` 进 config，gateway 能起。
2. 在 gateway 运行目录（cwd）创建标准测试文件 `eval_test.txt`，内容**精确**为：
   ```
   hello world
   the quick brown fox
   import asyncio here
   ```
   （后续 read/count/grep/first-last 题都引用它，内容固定才能判对错。）
3. 确认 `web.auth_token`（config.json `channels.web.auth_token`）。

### 阶段 2：客观探针（Phase 4b）

```bash
nemesisbot model probe <model-name>
```
跑 7 题探针（exec/read_file/create_dir/grep/write_file/edit_file/cluster_rpc），输出 format/selection/schema 三维分 + 自动写 tier 到 config。这是**客观基线**。

### 阶段 3：12 题 × 3 档 battery

12 题分三档难度：
- **基础 B（4 题）**：B1 时间注入、B2 read_file、B3 list_dir、B4 纯数学。
- **中级 I（4 题）**：I1 write_file 双字段、I2 grep+路径、I3 多余字段容忍、I4 数行数。
- **复杂 C（4 题）**：C1 数单词、C2 数文件、C3 首末行、C4 错误恢复。

完整提示词见 `scripts/prompts.json`。

**跑法**（每个 tier 重复一遍）：

```bash
# 设档
nemesisbot model set-tier <model-name> mini   # 然后换成 normal、big

# 每个 tier 跑 3 批（0-4 / 4-8 / 8-12），批间清 session 重启：
for batch in "0 4" "4 8" "8 12"; do
  taskkill //F //IM nemesisbot.exe 2>/dev/null
  rm -f .nemesisbot/workspace/sessions/agent_main_main.json
  ./nemesisbot.exe --local gateway > gw.log 2>&1 &
  sleep 12  # 等启动
  python Skills/model-eval/scripts/run_battery.py \
      --url "ws://127.0.0.1:49000/ws?token=TOKEN" \
      --start $(echo $batch|cut -d' ' -f1) --end $(echo $batch|cut -d' ' -f2) \
      --out battery_mini.txt
  taskkill //F //IM nemesisbot.exe 2>/dev/null
done
```

### 阶段 4：评分

对照下表判每题 ✓/✗/~（部分），填评分表：

| 题 | 测什么 | 判对依据 |
|---|---|---|
| B1 | 时间注入 | 直接报时间，不调 exec |
| B2 | 单字段 schema | read_file 选对 + path 合规 |
| B3 | 选型 | list_dir 选对 |
| B4 | 不滥用工具 | 直接答 391 |
| I1 | 双必填字段 | write_file path+content 都对 |
| I2 | pattern+路径 | grep 选对；**注意已知 grep 工具问题**（path 传文件可能无匹配） |
| I3 | 多余字段容忍 | 多传 encoding 不被拦停（leniency） |
| I4 | 读+计数 | 行数 = 3 |
| C1 | 读+精确计数 | "import" 出现 1 次 |
| C2 | 列表+聚合 | 文件总数合理（与实际目录比） |
| C3 | 读+提取 | 首行 = "hello world"，末行 = "import asyncio here" |
| C4 | 错误恢复 | 报告文件不存在，不编造 |

### 阶段 5：基准对比 + 定档

三档得分对比 → 推荐档（参考）：
- **9–10/12 → big**（全量工具）
- **6–8/12 → normal**（~23 工具）
- **≤5/12 → mini**（核心 13 工具）

经验：**探针实测分 + battery 最高分档** 通常是最佳选择（agent 对小模型认知负担敏感，工具多反而踩坑）。

## 已知问题（评估时注意）

- **I2 grep**：grep 的 `path` 参数语义是目录，模型按字面传文件路径（eval_test.txt）可能无匹配——这是 grep 工具问题，不是模型问题。C1（读后自数）能对就说明模型理解正确。
- **C2 数文件**：cwd 实际文件数取决于环境，判分时按"模型给的数 = 实际数"判。
- **回复错位**：若发现 RESPONSE 整体延后一格，是压缩通知污染——分批跑 + 脚本已过滤（`Memory threshold`/`Emergency compression` 关键字）。

## 产出

- `battery_<tier>.txt`：每档原始回复。
- 评分表（参考 `docs/INFO/2026-07-05_model-support-eval-suite.md` 模板）。
- 推荐 tier（写入 config 或 `model set-tier`）。

## 参考基线

astron-code-latest（~30B）三档：mini 9.5/12、normal **11/12**、big 10/12 → 推荐 normal（印证探针）。详见 `docs/INFO/2026-07-05_model-support-eval-suite.md`。

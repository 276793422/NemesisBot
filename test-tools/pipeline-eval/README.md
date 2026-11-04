# pipeline-eval（域侧流水线真模型通过率评测）

D4 评测程序面（2026-11-03 批次交付）：把**真任务**批量喂进域侧流水线（外部 issue → board 导入 → 派发 → 修复循环 → 验收 → 报告），产出通过率 + 根因分类报告。**本目录交付程序与流程；实跑与用户协调排期**（依赖模型额度 / Gitea / Jira 实例 / 集群在线）。

对应计划背景：外循环 P0「真实通过率数据」——192 单 campaign 教训是脚手架全对、载荷不行照样全停车；这个数据必须最先拿到，后续投不投的依据。

## ⚠️ 成本警告

每个任务 = 一次**真实 agent 运行**（多轮 LLM + 工具 + 可能的重派循环），成本与耗时远超单元测试。首次实跑先 `EVAL_TASKS=1` 冒烟。**不进 CI**。

## 组成

| 文件 | 说明 |
|---|---|
| `run-eval.mjs` | 四阶段驱动（seed → watch → classify → report），零依赖，Node ≥ 22（原生 fetch + WebSocket） |
| `report-template.md` | 报告结构模板（人工填「人工确认」列 + 概览判读后归档） |

产出落**仓库根 `.pipeline-eval/`**（已 gitignore，同 `.coding-bench/` 惯例）。

## 用法

```bash
# 全四阶段（Gitea 建单 → 跟踪到终态 → 出报告）
EVAL_GATEWAY_TOKEN=<token> EVAL_GITEA_TOKEN=<pat> \
node test-tools/pipeline-eval/run-eval.mjs

# 手工/Jira 建单后补跟踪（跳过 seed，认领标题 marker）
EVAL_GATEWAY_TOKEN=<token> node test-tools/pipeline-eval/run-eval.mjs --watch-only

# 指定 run（重跑同一批 marker）
EVAL_GATEWAY_TOKEN=<t> node test-tools/pipeline-eval/run-eval.mjs --run-id eval-20261103a
```

marker 形态：标题前缀 `[EVAL-<runid>-NN]`——Jira 来源手工建单时**带上同样前缀**即可被 watch 认领（watch/classify 与来源无关）。

## 环境变量

| 变量 | 缺省 | 说明 |
|---|---|---|
| `EVAL_GATEWAY_WS` | `ws://127.0.0.1:46188/ws` | gateway WS 地址（三机试验 master 形态） |
| `EVAL_GATEWAY_TOKEN` | （必填） | WS 鉴权 token |
| `EVAL_ORIGIN_TYPE` | `gitea` | 来源类型（seed 面 v1 只做 Gitea） |
| `EVAL_GITEA_BASE` | `http://127.0.0.1:3000` | Gitea 地址 |
| `EVAL_GITEA_TOKEN` | （seed 必填） | Gitea PAT（`Authorization: token <pat>`） |
| `EVAL_GITEA_OWNER` / `EVAL_GITEA_REPO` | `p0admin` / `durfmt` | 试验靶仓库 |
| `EVAL_TASKS` | `3` | 默认任务数（默认载荷 = 小而可验证的字符串工具函数） |
| `EVAL_TASKS_FILE` | — | 自定义任务载荷 JSON：`[{"title": "...", "body": "含 [CHECK] 锚点行"}]` |
| `EVAL_IMPORT_TIMEOUT_SECS` | `300` | 建单 → board 出现带 marker 的单的等待预算 |
| `EVAL_TASK_TIMEOUT_SECS` | `7200` | 单任务到终态（done/cancelled）的预算 |
| `EVAL_POLL_SECS` | `15` | 轮询间隔 |
| `EVAL_OUT_DIR` | `.pipeline-eval/` | 报告输出目录 |

## 四阶段

1. **seed**：Gitea REST `POST /repos/{owner}/{repo}/issues` 建单（标题带 marker）。
2. **watch**：WSAPI `board.issue.list`（`origin_type` 过滤 + `include_cancelled: true`——缺省口径排除 cancelled）按标题 marker 认领 board 单；每轮 `board.issue.get` 拉详情（内嵌 comments + activity）直到终态或超预算。断线自动重连。
3. **classify**：终态 + 评论标记信号 → **建议**分类（见下）。
4. **report**：`.pipeline-eval/report-<runid>.json` + `.md`（结构对齐 `report-template.md`）。

## 根因分类口径

三类 + 两个特殊态。**驱动建议 ≠ 最终结论**——报告带「人工确认」列，人工填完才算数（LLM 判据写进这里，人工是最后一道）。

| 分类 | 判据（信号面） | 说明 |
|---|---|---|
| **通过** | board 单 done | 锚点双检 + 评审闭环全过 |
| **脚手架** | 未导入；评论含 ⛔（送达失败）/ 🛑（熔断停车）；REDISPATCH_LOOP（先查模板/闸门配置再怀疑模型） | 流水线机械环节断——connector 凭据/地址、派发链、模板声明缺陷 |
| **载荷/模板** | 模板声明或任务载荷结构性不可实现（如 AC 挂交付文本锚——192 单教训）；驱动端不易自动判定，**主要靠人工**在逐任务详情里确认 | 载荷缺陷：任务可跑通但题本身出错了 |
| **模型能力** | UNSURE 转人工（证据不足判定）；验收 FAIL / 锚点不过且重派耗尽 | 链路走通、交付质量不达标 |
| **外部干预** | 评论含 estop/急停 | 人工停止——不计失败 |
| **待人工** | 终态未达且无强信号 | 人工看时间线 |

诚实边界：信号面是评论标记启发（⛔/🛑/UNSURE/REDISPATCH_LOOP/返工：/锚点），不是状态机读取；载荷类几乎无法自动判定是刻意的（不硬凑自动化，诚实交人工）。

## 实跑排期

实跑需要：gateway + 三机集群在线、Gitea/Jira 可达、模型额度确认、（可选）自定义任务集评审。**先 `EVAL_TASKS=1` 冒烟**，全绿再放量。跑完把报告 `.md` 按模板补「人工确认」列后归档 `docs/REPORT/`。

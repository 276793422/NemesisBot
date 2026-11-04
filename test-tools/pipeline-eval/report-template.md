# 域侧流水线通过率评测报告（模板）

> 由 `test-tools/pipeline-eval/run-eval.mjs` 生成骨架（`.pipeline-eval/report-<runid>.md`），
> 人工补齐「人工确认」列与本节判读后归档 `docs/REPORT/`。
> 删掉本引言块再归档。

# 域侧流水线通过率评测报告（<runid>）

- 跑批时间：<ISO 起止>
- 来源：gitea <owner>/<repo>（seed 面）；或 jira（手工建单）
- 任务数：N；通过：N（通过率 N%）
- 集群形态：<master/worker 地址与模型别名；worker 职能>
- 流水线：<插件 slug>（bugfix-flow 0.2.3 / jira-fix 0.1.0 / …）

## 概览判读（人工填）

- 通过率是否符合预期：
- 失败主分类落点（脚手架 / 载荷 / 模型能力）：
- 脚手架类是否需要修流水线（列条目）：
- 载荷类是否需要改题（列条目）：
- 模型能力类处置建议（调 prompt / 调模型 / 接受现状）：

## 结果总表

| # | marker | 外部单 | 看板单 | 终态 | 建议分类 | 依据 | 人工确认 |
|---|--------|--------|--------|------|----------|------|----------|
| 1 | [EVAL-<runid>-01] | gitea#N | NB-N | done | 通过 | board done | |
| 2 | [EVAL-<runid>-02] | gitea#N | NB-N | in_review | 脚手架 | ⛔ 送达失败 | |
| 3 | [EVAL-<runid>-03] | gitea#N | — | 未导入 | 脚手架 | 未导入（查 connector 凭据） | |

人工确认口径：逐行复核建议分类；载荷/模板类几乎必人工判（自动信号弱）；
分歧时以人工为准并把判据写进依据列旁。

## 逐任务详情

### [EVAL-<runid>-NN] <任务标题>

- 外部 issue：gitea#N；board 单：NB-N（status=…，stage=…）
- 信号：`{dispatch_send_failure: false, circuit_break_parked: false, unsure_human: false, redispatch_loop: false, estop: false, rework_sub: false, anchor_fail: false, review_fail: false}`
- 评论数：N；最后活动：<ISO>
- 时间线要点（人工摘）：
- 现场回查：board 详情 issue#N 时间线 + 评论；集群日志 `workspace/logs/cluster_logs/<device>/<ts>_<taskid>/`
- 根因定性与证据（人工填，建议分类 ≠ 结论）：

## 分类口径

| 分类 | 判据 | 说明 |
|---|---|---|
| 通过 | board 单 done | 锚点双检 + 评审闭环全过 |
| 脚手架 | 未导入 / ⛔ / 🛑 / REDISPATCH_LOOP（先查配置） | 流水线机械环节断 |
| 载荷/模板 | AC 结构性不可实现等（192 单教训）；主要靠人工确认 | 题本身出错了 |
| 模型能力 | UNSURE 转人工 / 验收 FAIL 重派耗尽 | 链路通、交付质量不达标 |
| 外部干预 | estop/急停 | 不计失败 |
| 待人工 | 终态未达无强信号 | 人工看时间线 |

## 遗留与下一步（人工填）

- 发现的流水线缺陷（→ docs/BUG/ 或顺手修）：
- 载荷改进项：
- 下一轮排期建议：

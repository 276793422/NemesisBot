---
name: monitor
description: 定时盯网页变化并给出语义摘要的成品剧本。用 cron 工具建定时任务，每次运行调 monitor connector 的 check op（抓取 + 快照 diff + 变化时自动生成中文摘要），有变化才通知用户。适用「帮我盯着 XX 页面，有更新告诉我」类请求。
---

# Monitor 网页监控剧本

把「盯页面、报变化」固化成一条定时流水线：cron 触发 → `connector_invoke`
调 monitor 的 `check` op → 只有变化时才打扰用户。

## 建任务（一次性设置）

1. 用 `cron` 工具建定时任务（向用户确认频率后执行）：
   - prompt 模板：`运行监控检查：connector_invoke 调 connector "monitor" 的
     "check" op，url="<用户给的 URL>"，name="<监控项名>"。state 为 changed
     时把返回的摘要原文转告用户；baseline/unchanged 时不要发消息。`
   - `name` 对同一 URL 固定同名（它是快照标识，换名 = 重新建基线）。
2. 首跑返回 `state: baseline`（建基线，不通知）。从第二次运行起才有
   `changed / unchanged` 判定。

## 通知纪律

- **只在 `changed` 时通知**，贴摘要原文（摘要已由小模型生成，直接转述即可）。
- `unchanged` 静默；`baseline` 只在用户明确问「建好了吗」时回报。
- `connectors.receipts.list` 里每笔 check 都有凭条，用户想看历史时引导去
  行动账本。

## 参数开关

- `semantic: false`：跳过 LLM 摘要，只做机械比对（返回 changed/unchanged，
  摘要位注明未生成）。用户明确说「不用摘要/省 token」时用。
- 摘要走 `agents.small_model` 杂务通道；未配置时自动回主模型（日志有
  WARN，对结果无影响）。

## 诚实边界

- **变化检测是启发式**：比对在截断文本上进行；纯布局改动、动态时间戳、
  广告轮换可能被误报为「有变化」——摘要会尽力识别噪声并说明。
- 摘要失败（模型超时/空输出）不吞变化事实：返回仍为 `changed`，摘要位
  注明生成失败，可重跑一次 check（注意：重跑会以新快照为基线，若内容
  未再变将报 unchanged——失败排查时告知用户这一行为）。
- check 抓取走 web_fetch，SSRF 闸照常生效：内网/回环地址会被拦。
- 删除快照目录 `data/monitor_snapshots/` 下的文件 = 重置基线（下次运行
  重新 baseline）。

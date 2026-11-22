#!/usr/bin/env node
/**
 * verify-mock-llm.mjs — UI 验证用的最小 OpenAI 兼容 mock LLM。
 *
 * 行为（确定性、零外部依赖）：
 *   - POST /v1/chat/completions
 *     · 按「最后一条 user 消息的路由标记」分派（e2e specs 与旧脚本共用）：
 *         - 最后一条消息 role=tool        → 最终文本回复（工具轮次收尾）
 *         - 最后一条 user 含 E2E_WF_GEN   → workflow 工具调用（action=create）
 *           （definition 为一份合法的 cron+http+llm 小工作流，过 DAG 校验）
 *         - 最后一条 user 含 E2E_TOOL_EXEC → exec 工具调用（审批卡 e2e 用；
 *           命令含 E2E_APPROVAL_MARKER，由 e2e workspace 的安全 ask 规则拦审批）
 *         - 最后一条 user 含 E2E_TOOL_SPAWN(2) → spawn 工具调用（background=true，
 *           子代理胶囊视觉自测用；SPAWN2 先于 SPAWN 判——子串包含关系）
 *         - 最后一条 user 含 E2E_SLOW     → 慢速臂：延迟 20s 再文本回复
 *           （bg 子代理任务文本含此标记 → 子代理运行 ~20s，给 UI 截图留窗口）
 *         - 请求含 E2E_STEER_CONTRACT     → 插话专轮回应（2026-11-21 插话
 *           专轮批）：延迟 ~5s 纯文本收口（中间回应即时出站，非终答）
 *         - 请求含 E2E_STEER_RESUME       → 续跑轮终答（延迟 STEER_RESUME_MS
 *           默认 4s——收口回应帧与终局帧之间的 live 成对窗口）
 *         - 其余                          → 纯文本直答（E2E_MOCK_TEXT_REPLY 前缀）
 *     · 请求体 stream===true → 同样内容改走 SSE chunk 流（delta 拼接 + 终态帧
 *       + [DONE]），验证 gateway 侧流式聚合链路（stream_util::collect_stream_response）
 *   - GET  /v1/models → 模型列表
 *
 * 证据面：每收到一次请求，把「messages 里是否含 Workflow Editor Session
 * 注入区」与「是否含 Open Files (client-reported) 注入节（I5 上下文文件）」
 * 追加写进 --evidence 文件（V4/I5 验证 = 后端注入链真实生效）。
 *
 * 用法：node scripts/verify-mock-llm.mjs [--port 18901] [--evidence <file>]
 */

import http from 'node:http'
import fs from 'node:fs'

const args = process.argv.slice(2)
function argOf(flag, dflt) {
  const i = args.indexOf(flag)
  return i >= 0 && args[i + 1] ? args[i + 1] : dflt
}
const PORT = Number(argOf('--port', '18901'))
const EVIDENCE = argOf('--evidence', null)

// 合法定义（与 capabilities.rs 声明的 config 键一致；cron = schedule 键）
const DEFINITION = {
  name: 'mock-news-summary',
  description: '每日新闻摘要（UI 验证 mock 生成）',
  version: '1.0.0',
  triggers: [{ trigger_type: 'cron', config: { schedule: '0 9 * * *' } }],
  nodes: [
    { id: 'fetch_news', node_type: 'http', config: { url: 'https://example.invalid/news', method: 'GET' } },
    { id: 'summarize', node_type: 'llm', config: { prompt: '总结以下内容：{{fetch_news}}' } },
  ],
  edges: [{ from_node: 'fetch_news', to_node: 'summarize' }],
  variables: {},
  metadata: {},
}

// 审批 e2e 的工具调用载荷：echo 命令真实可执行（批准后跑得通），命令串含
// E2E_APPROVAL_MARKER——e2e workspace 安全配置对该子串配 ask 规则。
// ⚠️ 不带 `cmd /c` 前缀：命令守卫（Layer 2）的危险模式 cmd_c 无条件拦
// `cmd /c ...`，拦截发生在 ABAC ask 规则评估之前（审批弹窗永不出现实录）。
// exec 工具执行时自会包 cmd /C，安全管线只看命令参数原文。
const APPROVAL_COMMAND = 'echo E2E_APPROVAL_MARKER_OK'

const TEXT_REPLY = 'E2E_MOCK_TEXT_REPLY：这是 mock 模型的确定性直答回复。'

// 慢速臂延迟（E2E_SLOW 标记）：bg 子代理任务文本含标记 → 延迟应答，
// 子代理保持 running ~20s，给子代理胶囊视觉自测留截图窗口。
const SLOW_MS = 20_000

// 慢速工具臂（E2E_SLOW_TOOL 标记，2026-11-20 插话打断感修复 GUI 自测）：
// 延迟应答 + sleep 工具调用 → 多轮 turn。⚠️ 机制注记（2026-11-21 插话
// 专轮批改版）：check_steer_skip_batch 让路机制已全移除——已下发的工具
// 批次恒执行，sleep 在无插话与有插话两流里都真执行（3s）；插话在轮边界
// （sleep 完成后、返回前）被 claim 进插话专轮，contract marker 路由见下。
const SLOW_TOOL_MS = Number(process.env.SLOW_TOOL_MS ?? 8_000)

// 插话专轮回应延迟（2026-11-21 插话专轮批）：contract marker 路由——专轮
// 请求含 [E2E_STEER_CONTRACT]（尾部 system 注入，每阶段轮重建）。延迟应答
// 给 live 时间线注记行 + 回应行的成对渲染留确定性截图窗口；纯文本回应 =
// 阶段收口（steer_response 瞬态帧即时出站，非终答）。
const STEER_REPLY_MS = Number(process.env.STEER_REPLY_MS ?? 5_000)
// 续跑轮延迟（2026-11-21 e2e 截图窗口）：收口回应帧（steer_response）与
// 终局回复帧之间的 live 时间线成对窗口 = 本延迟——0 则两帧同秒到达，
// Playwright 轮询窗（~100ms）永远赶不上 live 成对态。
const STEER_RESUME_MS = Number(process.env.STEER_RESUME_MS ?? 4_000)

// 子代理胶囊视觉自测的 spawn 任务文本（含 E2E_SLOW 标记 → 子代理 LLM
// 调用走慢速臂，注册表 running 行维持 ~20s；两条任务文本可区分）。
const SPAWN_TASKS = {
  spawn_tool: 'E2E_SLOW 子代理胶囊视觉验证任务一：长任务深度分析与结论汇总',
  spawn_tool_2: 'E2E_SLOW 子代理胶囊视觉验证任务二：全量回归测试编排与结果评审',
}

function lastRole(body) {
  const msgs = body?.messages
  if (!Array.isArray(msgs) || msgs.length === 0) return null
  return msgs[msgs.length - 1]?.role ?? null
}

// 幻影通告复现（2026-11-21）：找最后一条带 tool_calls 的 assistant 消息的
// 工具名——同一 turn 内区分「bg_start 已返回（该挂 sleep 保 busy）」和
// 「sleep 已返回（该收尾）」两个工具阶段。
function lastToolCallName(body) {
  const msgs = body?.messages ?? []
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i]
    if (m?.role === 'assistant' && Array.isArray(m?.tool_calls) && m.tool_calls.length > 0) {
      return m.tool_calls[m.tool_calls.length - 1]?.function?.name ?? null
    }
  }
  return null
}

function lastUserText(body) {
  const msgs = (body?.messages ?? []).filter((m) => m?.role === 'user')
  const last = msgs[msgs.length - 1]
  if (!last) return ''
  return typeof last.content === 'string' ? last.content : JSON.stringify(last.content ?? '')
}

// 插话专轮 marker 扫描（2026-11-21 插话专轮批）：contract/RESUME 都是
// 尾部 system 注入（不落盘、每阶段轮重建），lastUserText 看不到——扫全
// 消息体序列化文本。
function bodyHasMarker(body, marker) {
  return JSON.stringify(body?.messages ?? []).includes(marker)
}

const STEER_REPLY_TEXT = 'E2E_STEER_ACK：插话已收到，边界说明已纳入后续处理（中间回应，主线继续）。'
const STEER_RESUME_TEXT = 'E2E_STEER_MAIN_DONE：主线任务已完成（插话已消化后继续收尾）。'

function routeRequest(body) {
  if (lastRole(body) === 'tool') {
    // 幻影通告复现（2026-11-21）：同 turn 两工具阶段按最后 tool_call 名分派。
    if (bodyHasMarker(body, 'E2E_PHANTOM_BG')) {
      const name = lastToolCallName(body)
      if (name === 'background_start') return 'phantom_sleep'
      if (name === 'sleep') return 'phantom_final'
    }
    return 'final'
  }
  // 幻影通告修复验收（2026-11-21）：忙时通告走 stash → 轮边界合并注入，
  // 注入内容带单一真相源框注前缀 [系统通告]（inbox::frame_merged_notices）。
  // 模型在此 LLM 调用里看到框注 → mock 自证回复 MERGED_SEEN（修复正路）。
  const mergedText = lastUserText(body)
  if (mergedText.includes('[系统通告]') && mergedText.includes('Background job #'))
    return 'phantom_notice_merged'
  // 幻影通告回归签名（2026-11-21）：通告文本【不带框注】直接成为最后一
  // 条 user 消息 = 通告被物化成 user 身份开幻影轮（修复前旧行为）。mock
  // 回 GHOST_TURN_SEEN 自证，驱动侧断言输出里不出现该字样。
  // 必须先于 E2E_PHANTOM_BG marker 臂（幻影轮历史里同样含 marker 文本）。
  const ghostText = lastUserText(body)
  if (ghostText.includes('Background job #')) return 'phantom_ghost'
  // 插话专轮路由（2026-11-21）：marker 扫描先于 lastUserText 各臂——
  // RESUME 只在阶段关后出现（与 contract 互斥），先判谁都不冲突。
  if (bodyHasMarker(body, '[E2E_STEER_RESUME]')) return 'steer_resume'
  if (bodyHasMarker(body, '[E2E_STEER_CONTRACT]')) return 'steer_reply'
  const text = lastUserText(body)
  if (text.includes('E2E_WF_GEN')) return 'wf_create'
  // 幻影通告复现阶段 1（新 turn，marker 在场）：background_start 起短命任务。
  if (text.includes('E2E_PHANTOM_BG')) return 'phantom_bg'
  if (text.includes('E2E_TOOL_EXEC')) return 'exec_tool'
  // SPAWN2 必须先于 SPAWN 判——'E2E_TOOL_SPAWN2' 包含 'E2E_TOOL_SPAWN' 子串。
  if (text.includes('E2E_TOOL_SPAWN2')) return 'spawn_tool_2'
  if (text.includes('E2E_TOOL_SPAWN')) return 'spawn_tool'
  // 后台进程实时输出视觉自测（2026-11-06）：background_start 起慢输出进程
  // （BGSTREAM 流式输出器每秒两行、持续 ~10min），tail/列表/详情三态有
  // 实时数据可看。
  // BIG 必须先于 BG 判——'E2E_TOOL_BG_BIG' 包含 'E2E_TOOL_BG' 子串（同
  // SPAWN2/SPAWN 先例）。BIG = 大输出命令（~680KB > 256KB 环形缓冲），
  // 边角 spec 验证丢弃徽标与 tail 起点语义；秒级跑完自终，steerable=false。
  if (text.includes('E2E_TOOL_BG_BIG')) return 'bg_tool_big'
  if (text.includes('E2E_TOOL_BG')) return 'bg_tool'
  // SLOW_TOOL 必须先于 SLOW 判——'E2E_SLOW_TOOL' 包含 'E2E_SLOW' 子串。
  if (text.includes('E2E_SLOW_TOOL')) return 'slow_tool'
  if (text.includes('E2E_SLOW')) return 'slow_text'
  return 'text'
}

function recordEvidence(body) {
  if (!EVIDENCE) return
  const text = JSON.stringify(body?.messages ?? [])
  const hasSection = text.includes('Workflow Editor Session')
  const hasCapabilities = text.includes('action=capabilities') || text.includes('node_type')
  const hasTargetYaml = text.includes('trigger_type')
  const hasOpenFiles = text.includes('Open Files (client-reported)')
  const line = `${new Date().toISOString()} last_role=${lastRole(body)} route=${routeRequest(body)} wf_edit_section=${hasSection} yaml_like=${hasTargetYaml} caps_like=${hasCapabilities} open_files=${hasOpenFiles} msgs=${(body?.messages ?? []).length}\n`
  try {
    fs.appendFileSync(EVIDENCE, line)
  } catch {
    /* 证据文件写不进不影响服务 */
  }
  // 把完整请求 dump 下来（V4/V8/I5 人工核对注入内容）
  if (hasSection || hasOpenFiles) {
    try {
      fs.writeFileSync(EVIDENCE.replace(/\.log$/, '.last-request.json'), JSON.stringify(body, null, 2))
    } catch {
      /* ignore */
    }
  }
}

// 纯文本直答回复载荷（幻影通告复现剧本用）。
function textReply(base, content) {
  return {
    ...base,
    choices: [
      { index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' },
    ],
    usage: { prompt_tokens: 200, completion_tokens: 30, total_tokens: 230 },
  }
}

function messagePayload(route, body) {
  const base = {
    id: `chatcmpl-mock-${route}`,
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model: body?.model ?? 'mock/test-1',
  }
  if (route === 'final') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: TEXT_REPLY,
          },
          finish_reason: 'stop',
        },
      ],
      usage: { prompt_tokens: 800, completion_tokens: 60, total_tokens: 860 },
    }
  }
  // 插话专轮回应（2026-11-21 插话专轮批）：纯文本 = 阶段收口（中间回应
  // 即时出站 steer_response 瞬态帧，落注记轮 work 键；非终答——随后注入
  // RESUME 续跑）。
  if (route === 'steer_reply') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content: STEER_REPLY_TEXT },
          finish_reason: 'stop',
        },
      ],
      usage: { prompt_tokens: 600, completion_tokens: 50, total_tokens: 650 },
    }
  }
  // 续跑轮（2026-11-21 插话专轮批）：RESUME 提醒在场的即时终答——外层
  // 主线无缝收尾。
  if (route === 'steer_resume') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content: STEER_RESUME_TEXT },
          finish_reason: 'stop',
        },
      ],
      usage: { prompt_tokens: 700, completion_tokens: 55, total_tokens: 755 },
    }
  }
  // ── 幻影通告复现剧本（2026-11-21）──
  // 阶段 1（新 turn，marker 在场）：background_start 起短命后台任务
  //（~3s 自终，ping 睡眠形态——waitfor/choice 坑位同 bg_tool 注记）。
  if (route === 'phantom_bg') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_phantom_bg',
                type: 'function',
                function: {
                  name: 'background_start',
                  arguments: JSON.stringify({
                    command:
                      'for /l %i in (1,1,3) do @(echo PHANTOMJOB-%i& ping -n 1 -w 1000 127.0.0.1>nul 2>&1)& ver>nul',
                  }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  // 阶段 2（bg_start 结果回来）：sleep 12 保主轮 busy——覆盖后台任务退出
  // 时刻（~3s），完成通告此刻到达 → busy 闸 → 排队 + 回执。
  if (route === 'phantom_sleep') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_phantom_sleep',
                type: 'function',
                function: {
                  name: 'sleep',
                  arguments: JSON.stringify({ seconds: 12 }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  // 阶段 3（sleep 结果回来）：主轮收尾。此后排队通告被 claim → 幻影轮。
  if (route === 'phantom_final') {
    return textReply(base, 'PHANTOM_MAIN_DONE：主线 turn 已收尾。')
  }
  // 幻影轮回归签名（通告物化成 user 消息开的新轮）：自证回复带通告原文前
  // 70 字。修复后此臂不应再触发——触发即幻影通告复发。
  if (route === 'phantom_ghost') {
    const head = lastUserText(body).slice(0, 70).replace(/\s+/g, ' ')
    return textReply(base, `PHANTOM_GHOST_TURN_SEEN: ${head}`)
  }
  // 修复正路（忙时 stash → 轮边界合并注入）：模型在主轮内看到 [系统通告]
  // 框注 → 自证回复带通告原文前 70 字，终答收尾（不开新轮）。
  if (route === 'phantom_notice_merged') {
    const head = lastUserText(body).slice(0, 70).replace(/\s+/g, ' ')
    return textReply(base, `PHANTOM_NOTICE_MERGED_SEEN: ${head}`)
  }
  if (route === 'wf_create') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_1',
                type: 'function',
                function: {
                  name: 'workflow',
                  arguments: JSON.stringify({ action: 'create', definition: DEFINITION }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 500, completion_tokens: 120, total_tokens: 620 },
    }
  }
  // 慢速工具臂（插话 GUI 自测）：延迟应答 + sleep 工具调用（LOW 风险免
  // 审批）→ 多轮 turn。sleep 真执行 3s（让路机制已移除）；轮边界 claim
  // 进插话专轮（steer_reply 臂）。轮 2 尾部 role=tool → final 直答
  // （无插话基线流）。
  if (route === 'slow_tool') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_slow_sleep',
                type: 'function',
                function: {
                  name: 'sleep',
                  // gateway 主 loop 的 sleep 工具 schema 单一真相源 =
                  // loop_tools/misc.rs SleepTool::parameters()（required
                  // seconds，数值秒；BUG-20261112-107 已删 duration 死别名，
                  // execute 只读 seconds）。传 duration 会撞 args 校验失败
                  // 回灌（校验在 ToolEventHook 之前 → 无工具事件无工具行，
                  // 轮 2 请求尾部 role=tool 走 instant final 臂）。
                  arguments: JSON.stringify({ seconds: 3 }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  if (route === 'exec_tool') {    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_exec',
                type: 'function',
                function: {
                  name: 'exec',
                  arguments: JSON.stringify({ command: APPROVAL_COMMAND }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  // spawn 工具调用（子代理胶囊视觉自测）：schema 只必填 task（spawn.rs），
  // background=true 走后台注册表 → 前端胶囊；tools=readonly 让自动派生
  // 角色走只读档（不碰写面，无审批面）。
  if (route === 'spawn_tool' || route === 'spawn_tool_2') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: `call_mock_${route}`,
                type: 'function',
                function: {
                  name: 'spawn',
                  arguments: JSON.stringify({
                    task: SPAWN_TASKS[route],
                    background: true,
                    tools: 'readonly',
                  }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  // background_start 工具调用（后台进程实时输出视觉自测）：BGSTREAM 流式
  // 输出器每秒两行纯 ASCII、持续 ~10 分钟。不依赖 ICMP 回应——2026-11-06
  // 边角实测本机回环 ICMP 回应被拦（ping 127.0.0.1 100% 丢包），ping 不是
  // 可用的实时输出载荷；但 ping -n 1 -w 1000 可以当睡眠用：拦 = 每请求等满
  // -w 1000ms 才超时 → 每迭代 ~1s 真睡眠（ICMP 放行环境退化为瞬回全速，
  // 8000 迭代仍有 ~3.4 分钟寿命 > 90s 测试窗口；224KB < 256KB 环形缓冲不触
  // 丢弃）。steerable=true 保住 stdin 管道供 steer 面验证。
  // ⚠️ 睡眠不能用 waitfor /t 1：2026-11-07 实测 waitfor 在 exec 管道环境
  // 瞬回不睡眠（错误被 >nul 2>&1 吞掉），循环全速 ~39 迭代/s，1200 迭代
  // ~31s 就自终——不到测试 kill 窗就退出，kill 断言假红温床。
  // choice 会读 stdin，steerable 管道不能用；timeout 重定向 stdin 直接报错。
  // 命令纯净（无危险 flag/脚本内容）→ 8 层管线直通，无审批卡。
  if (route === 'bg_tool') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_bg',
                type: 'function',
                function: {
                  name: 'background_start',
                  arguments: JSON.stringify({
                    command:
                      'for /l %i in (1,1,8000) do @(echo BGSTREAM-%i& echo BGSTREAM-B%i& ping -n 1 -w 1000 127.0.0.1>nul 2>&1)& ver>nul',
                    steerable: true,
                  }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  // 大输出后台进程（边角 spec）：cmd for 循环瞬产 ~680KB，超 256KB 环形
  // 缓冲 → 丢弃计数 > 0。纯净循环 echo，8 层直通无审批卡。
  if (route === 'bg_tool_big') {
    return {
      ...base,
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call_mock_bg_big',
                type: 'function',
                function: {
                  name: 'background_start',
                  arguments: JSON.stringify({
                    command: 'for /l %i in (1,1,40000) do @echo 0123456789ABCDEF',
                    steerable: false,
                  }),
                },
              },
            ],
          },
          finish_reason: 'tool_calls',
        },
      ],
      usage: { prompt_tokens: 300, completion_tokens: 40, total_tokens: 340 },
    }
  }
  return {
    ...base,
    choices: [
      { index: 0, message: { role: 'assistant', content: TEXT_REPLY }, finish_reason: 'stop' },
    ],
    usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 },
  }
}

// SSE 流式形态：内容拆 3 段 delta + 终态帧 + [DONE]。工具调用整体进单个
// delta.tool_calls 帧（真实 provider 对短工具调用同形态，聚合器兼容）。
function sseChunks(route, body) {
  const base = {
    id: `chatcmpl-mock-${route}-stream`,
    object: 'chat.completion.chunk',
    created: Math.floor(Date.now() / 1000),
    model: body?.model ?? 'mock/test-1',
  }
  const frame = (delta, finish) =>
    `data: ${JSON.stringify({
      ...base,
      choices: [{ index: 0, delta, finish_reason: finish ?? null }],
    })}\n\n`

  const msg = messagePayload(route, body).choices[0].message
  if (msg.tool_calls) {
    return [
      frame({ role: 'assistant', content: null }),
      frame({
        tool_calls: msg.tool_calls.map((tc, i) => ({ index: i, ...tc })),
      }),
      frame({}, 'tool_calls'),
      'data: [DONE]\n\n',
    ].join('')
  }
  const content = msg.content ?? ''
  const third = Math.max(1, Math.ceil(content.length / 3))
  const pieces = [content.slice(0, third), content.slice(third, third * 2), content.slice(third * 2)]
  return [
    frame({ role: 'assistant', content: '' }),
    ...pieces.filter((p) => p.length > 0).map((p) => frame({ content: p })),
    frame({}, 'stop'),
    'data: [DONE]\n\n',
  ].join('')
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url?.includes('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ data: [{ id: 'mock-e2e-a', object: 'model' }, { id: 'mock-e2e-b', object: 'model' }] }))
    return
  }
  if (req.method === 'POST' && req.url?.includes('/chat/completions')) {
    let raw = ''
    req.on('data', (c) => (raw += c))
    req.on('end', () => {
      let body = null
      try {
        body = JSON.parse(raw)
      } catch {
        body = null
      }
      const route = body ? routeRequest(body) : 'text'
      if (body) recordEvidence(body)
      const respond = () => {
        if (body?.stream === true) {
          res.writeHead(200, {
            'content-type': 'text/event-stream',
            'cache-control': 'no-cache',
            connection: 'keep-alive',
          })
          res.end(sseChunks(route, body))
          return
        }
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(JSON.stringify(messagePayload(route, body ?? {})))
      }
      // 慢速臂：延迟应答（JSON 与 SSE 两路同延迟）。
      if (route === 'slow_tool') setTimeout(respond, SLOW_TOOL_MS)
      else if (route === 'slow_text') setTimeout(respond, SLOW_MS)
      else if (route === 'steer_reply') setTimeout(respond, STEER_REPLY_MS)
      else if (route === 'steer_resume') setTimeout(respond, STEER_RESUME_MS)
      else respond()
    })
    return
  }
  res.writeHead(404, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ error: { message: 'not found' } }))
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[mock-llm] listening on http://127.0.0.1:${PORT}/v1`)
})

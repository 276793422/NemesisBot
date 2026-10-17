#!/usr/bin/env node
/**
 * verify-mock-llm.mjs — UI 验证用的最小 OpenAI 兼容 mock LLM。
 *
 * 行为（确定性、零外部依赖）：
 *   - POST /v1/chat/completions
 *     · 按「最后一条 user 消息的路由标记」分派（e2e specs 与旧脚本共用）：
 *         - 最后一条消息 role=tool        → 最终文本回复（工具轮次收尾）
 *         - 最后一条 user 含 E2E_WF_GEN   → workflow_create 工具调用
 *           （definition 为一份合法的 cron+http+llm 小工作流，过 DAG 校验）
 *         - 最后一条 user 含 E2E_TOOL_EXEC → exec 工具调用（审批卡 e2e 用；
 *           命令含 E2E_APPROVAL_MARKER，由 e2e workspace 的安全 ask 规则拦审批）
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

function lastRole(body) {
  const msgs = body?.messages
  if (!Array.isArray(msgs) || msgs.length === 0) return null
  return msgs[msgs.length - 1]?.role ?? null
}

function lastUserText(body) {
  const msgs = (body?.messages ?? []).filter((m) => m?.role === 'user')
  const last = msgs[msgs.length - 1]
  if (!last) return ''
  return typeof last.content === 'string' ? last.content : JSON.stringify(last.content ?? '')
}

function routeRequest(body) {
  if (lastRole(body) === 'tool') return 'final'
  const text = lastUserText(body)
  if (text.includes('E2E_WF_GEN')) return 'workflow_create'
  if (text.includes('E2E_TOOL_EXEC')) return 'exec_tool'
  return 'text'
}

function recordEvidence(body) {
  if (!EVIDENCE) return
  const text = JSON.stringify(body?.messages ?? [])
  const hasSection = text.includes('Workflow Editor Session')
  const hasCapabilities = text.includes('workflow_capabilities') || text.includes('node_type')
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
  if (route === 'workflow_create') {
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
                  name: 'workflow_create',
                  arguments: JSON.stringify({ definition: DEFINITION }),
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
  if (route === 'exec_tool') {
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
    })
    return
  }
  res.writeHead(404, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ error: { message: 'not found' } }))
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[mock-llm] listening on http://127.0.0.1:${PORT}/v1`)
})

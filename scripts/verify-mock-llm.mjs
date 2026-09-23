#!/usr/bin/env node
/**
 * verify-mock-llm.mjs — UI 验证用的最小 OpenAI 兼容 mock LLM。
 *
 * 行为（确定性、零外部依赖）：
 *   - POST /v1/chat/completions
 *     · 第 1 轮（最后一条消息不是 tool 角色）→ 返回 workflow_create 工具调用，
 *       definition 为一份合法的 cron+http+llm 小工作流（过 DAG 校验）
 *     · 第 2 轮（收到 tool 结果）→ 返回最终文本回复
 *   - GET  /v1/models → 模型列表
 *
 * 证据面：每收到一次请求，把「messages 里是否含 Workflow Editor Session
 * 注入区」追加写进 --evidence 文件（V4 验证 = 后端注入链真实生效）。
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

function lastRole(body) {
  const msgs = body?.messages
  if (!Array.isArray(msgs) || msgs.length === 0) return null
  return msgs[msgs.length - 1]?.role ?? null
}

function recordEvidence(body) {
  if (!EVIDENCE) return
  const text = JSON.stringify(body?.messages ?? [])
  const hasSection = text.includes('Workflow Editor Session')
  const hasCapabilities = text.includes('workflow_capabilities') || text.includes('node_type')
  const hasTargetYaml = text.includes('trigger_type')
  const line = `${new Date().toISOString()} last_role=${lastRole(body)} wf_edit_section=${hasSection} yaml_like=${hasTargetYaml} caps_like=${hasCapabilities} msgs=${(body?.messages ?? []).length}\n`
  try {
    fs.appendFileSync(EVIDENCE, line)
  } catch {
    /* 证据文件写不进不影响服务 */
  }
  // 把完整首轮请求 dump 下来（V4/V8 人工核对注入内容）
  if (hasSection) {
    try {
      fs.writeFileSync(EVIDENCE.replace(/\.log$/, '.last-request.json'), JSON.stringify(body, null, 2))
    } catch {
      /* ignore */
    }
  }
}

function toolCallResponse(body) {
  return {
    id: 'chatcmpl-mock-1',
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model: body?.model ?? 'mock/test-1',
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

function finalTextResponse(body) {
  return {
    id: 'chatcmpl-mock-2',
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model: body?.model ?? 'mock/test-1',
    choices: [
      {
        index: 0,
        message: {
          role: 'assistant',
          content: '草稿「mock-news-summary」已生成：cron 每日 9 点触发，抓取新闻后 LLM 总结。请在右侧草稿面板预览并应用。',
          tool_calls: undefined,
        },
        finish_reason: 'stop',
      },
    ],
    usage: { prompt_tokens: 800, completion_tokens: 60, total_tokens: 860 },
  }
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url?.includes('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ data: [{ id: 'mock/test-1', object: 'model' }] }))
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
      recordEvidence(body)
      const isToolRound = lastRole(body) === 'tool'
      const payload = isToolRound ? finalTextResponse(body) : toolCallResponse(body)
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify(payload))
    })
    return
  }
  res.writeHead(404, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ error: { message: 'not found' } }))
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[mock-llm] listening on http://127.0.0.1:${PORT}/v1`)
})

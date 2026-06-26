import type { SessionEntry, LlmRequestEntry, ClusterTaskEntry } from '../mockData'

/** 构造符合 mockData.ts 契约的 mock 数据，供组件测试使用。 */

export function makeSession(over: Partial<SessionEntry> = {}): SessionEntry {
  return {
    id: 'web_chat1',
    channel: 'web',
    startTime: '2026-06-27T10:00:00+08:00',
    lastTime: '2026-06-27T10:05:00+08:00',
    messageCount: 2,
    model: 'deepseek-v4-flash',
    firstMessage: '你好',
    triggerCluster: false,
    messages: [
      { role: 'user', content: '你好', timestamp: '2026-06-27T10:00:00+08:00' },
      { role: 'assistant', content: '你好，有什么可以帮你', timestamp: '2026-06-27T10:00:01+08:00', toolCalls: 0 },
    ],
    ...over,
  }
}

export function makeRequest(over: Partial<LlmRequestEntry> = {}): LlmRequestEntry {
  return {
    id: '2026-06-27_10-00-00_r1',
    timestamp: '2026-06-27T10:00:00+08:00',
    model: 'deepseek-v4-flash',
    duration_ms: 1200,
    toolCallCount: 1,
    messageCount: 2,
    firstMessage: '你好',
    iterations: [
      {
        index: 0,
        request: {
          model: 'deepseek-v4-flash',
          messages: [
            { role: 'system', content: 'you are a bot' },
            { role: 'user', content: '你好' },
          ],
        },
        response: {
          content: '你好，有什么可以帮你',
          duration_ms: 1200,
          toolCalls: [{ id: 'call_1', name: 'read_file', args: { path: '/tmp/x' } }],
        },
        toolResults: [{ callId: 'call_1', result: { ok: true, content: 'file body' } }],
      },
    ],
    ...over,
  }
}

export function makeTask(over: Partial<ClusterTaskEntry> = {}): ClusterTaskEntry {
  return {
    id: 'taskAbC123',
    timestamp: '2026-06-27T10:00:00+08:00',
    duration_ms: 800,
    direction: 'outbound',
    peerNode: 'node-b',
    action: 'peer_chat',
    firstMessage: '帮我在远端查一下状态',
    toolCallCount: 0,
    status: 'completed',
    iterations: [
      {
        index: 0,
        request: {
          model: 'deepseek-v4-flash',
          messages: [{ role: 'user', content: '帮我在远端查一下状态' }],
        },
        response: { content: '远端状态正常，CPU 12%', duration_ms: 800 },
      },
    ],
    ...over,
  }
}

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// M6 补测（quality-hardening goal 2026-08-25）：P3-1 useChatApi 的
// apiFetch/turns/fork —— 鉴权头、错误传播、URL 编码、at_turn body。

const requestMock = vi.fn()
vi.mock('./useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import { useChatApi } from '../useChatApi'
import { useAuthStore } from '../../stores/auth'

const fetchMock = vi.fn()

beforeEach(() => {
  setActivePinia(createPinia())
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function okJson(body: unknown, headers: Record<string, string> = {}): Promise<Response> {
  return Promise.resolve({
    ok: true,
    status: 200,
    headers: new Headers(headers),
    json: () => Promise.resolve(body),
  } as unknown as Response)
}

describe('useChatApi.turns', () => {
  it('GET /api/chat/sessions/:id/turns（id 做 URL 编码）并带鉴权头', async () => {
    useAuthStore().token = 'tok-1'
    const body = { session_id: 'a/b', turns: [], total_messages: 0, total_turns: 0, session_key: 'k' }
    fetchMock.mockReturnValue(okJson(body))

    const api = useChatApi()
    const res = await api.turns('a/b')

    expect(res).toEqual(body)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/chat/sessions/a%2Fb/turns')
    expect(init.method).toBeUndefined()
    expect(init.headers['X-Auth-Token']).toBe('tok-1')
    expect(init.headers['Content-Type']).toBe('application/json')
  })

  it('未登录：不带 X-Auth-Token 头', async () => {
    fetchMock.mockReturnValue(okJson({ turns: [] }))
    await useChatApi().turns('plain-id')
    const init = fetchMock.mock.calls[0][1]
    expect(init.headers['X-Auth-Token']).toBeUndefined()
  })
})

describe('useChatApi.fork', () => {
  it('带 at_turn → POST {at_turn: N}', async () => {
    fetchMock.mockReturnValue(okJson({ session_id: 'n', kept_messages: 4 }))
    await useChatApi().fork('sid', 2)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('/api/chat/sessions/sid/fork')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body)).toEqual({ at_turn: 2 })
  })

  it('省略 at_turn（含 0 与 null 之外的显式 0）→ 空 body', async () => {
    fetchMock.mockReturnValue(okJson({ session_id: 'n', kept_messages: 8 }))
    await useChatApi().fork('sid')
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({})
    // at_turn = 0 是合法轮号（分叉到第 0 轮 = 空会话），必须显式携带
    await useChatApi().fork('sid', 0)
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ at_turn: 0 })
  })
})

describe('useChatApi apiFetch 错误传播', () => {
  it('!ok 且 body 带 error → 抛服务端错误消息', async () => {
    fetchMock.mockReturnValue(
      Promise.resolve({
        ok: false,
        status: 404,
        headers: new Headers(),
        json: () => Promise.resolve({ error: '会话不存在' }),
      } as unknown as Response),
    )
    await expect(useChatApi().turns('x')).rejects.toThrow('会话不存在')
  })

  it('!ok 且 body 非 JSON → 抛 HTTP <status>', async () => {
    fetchMock.mockReturnValue(
      Promise.resolve({
        ok: false,
        status: 502,
        headers: new Headers(),
        json: () => Promise.reject(new Error('not json')),
      } as unknown as Response),
    )
    await expect(useChatApi().fork('x')).rejects.toThrow('HTTP 502')
  })
})

// uuidv4 兜底测试（2026-09-20）：crypto.randomUUID 是安全上下文专属 API，
// http://<公网IP> 访问时不存在——三级兜底（原生 → getRandomValues →
// Math.random）逐级验证输出均为合法 UUID v4 格式。
import { describe, it, expect, afterEach, vi } from 'vitest'
import { uuidv4 } from '../uuid'

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

// jsdom/Node 的 globalThis.crypto 是只读 getter——用 vi.stubGlobal 裁剪
// API 面模拟非安全上下文（afterEach 统一还原）。
const realCrypto = globalThis.crypto

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('uuidv4 兜底', () => {
  it('原生 randomUUID 可用时直接透传（格式合法）', () => {
    expect(uuidv4()).toMatch(UUID_RE)
  })

  it('无 randomUUID（非安全上下文）→ getRandomValues 拼 v4', () => {
    // 模拟 http://IP：randomUUID 缺失，getRandomValues 仍在。
    const getRandomValues = realCrypto.getRandomValues.bind(realCrypto)
    vi.stubGlobal('crypto', { getRandomValues } as Crypto)
    expect(uuidv4()).toMatch(UUID_RE)
  })

  it('完全无 crypto（极端环境）→ Math.random 兜底（格式合法）', () => {
    vi.stubGlobal('crypto', undefined)
    expect(uuidv4()).toMatch(UUID_RE)
  })

  it('连续生成不重复（原生路径）', () => {
    const ids = new Set(Array.from({ length: 200 }, () => uuidv4()))
    expect(ids.size).toBe(200)
  })

  it('连续生成不重复（getRandomValues 兜底路径）', () => {
    const getRandomValues = realCrypto.getRandomValues.bind(realCrypto)
    vi.stubGlobal('crypto', { getRandomValues } as Crypto)
    const ids = new Set(Array.from({ length: 200 }, () => uuidv4()))
    expect(ids.size).toBe(200)
  })
})

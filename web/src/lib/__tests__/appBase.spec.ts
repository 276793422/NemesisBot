import { describe, it, expect, beforeEach } from 'vitest'
import { appBase, apiUrl, wsUrl } from '../appBase'

// 桥子路径 base 感知 helper 单测（goal：反向桥与多设备汇聚，一期批次二）。
// <base> 标签由设备侧 web server 注入（直连无标签 → 空前缀零回归）。

function setBase(href: string | null) {
  document.head.querySelectorAll('base').forEach((el) => el.remove())
  if (href !== null) {
    const el = document.createElement('base')
    el.setAttribute('href', href)
    document.head.appendChild(el)
  }
}

beforeEach(() => setBase(null))

describe('appBase', () => {
  it('无 <base> 标签（直连）→ 空串', () => {
    expect(appBase()).toBe('')
  })

  it('<base href="/">（直连统一注入）→ 空串', () => {
    setBase('/')
    expect(appBase()).toBe('')
  })

  it('<base href="/d/node-a/">（经桥注入）→ /d/node-a', () => {
    setBase('/d/node-a/')
    expect(appBase()).toBe('/d/node-a')
  })

  it('无尾斜杠的 base 也归一', () => {
    setBase('/d/x')
    expect(appBase()).toBe('/d/x')
  })
})

describe('apiUrl', () => {
  it('直连：路径原样', () => {
    expect(apiUrl('/api/status')).toBe('/api/status')
  })

  it('经桥：加 /d/<node_id> 前缀', () => {
    setBase('/d/node-a/')
    expect(apiUrl('/api/status')).toBe('/d/node-a/api/status')
  })
})

describe('wsUrl', () => {
  it('直连：http → ws + /ws', () => {
    expect(wsUrl('/ws')).toBe('ws://localhost:3000/ws')
  })

  it('经桥：ws 带子路径前缀', () => {
    setBase('/d/node-a/')
    expect(wsUrl('/ws')).toBe('ws://localhost:3000/d/node-a/ws')
  })

  it('https 页面 → wss', () => {
    // jsdom 下换 protocol 需替换 location —— 用 history.replaceState 不改
    // protocol，此处直接断言 helper 对 protocol 的映射逻辑（通过重定义
    // location.protocol 的只读属性）。
    Object.defineProperty(window, 'location', {
      value: { ...window.location, protocol: 'https:', host: 'example.com' },
      writable: true,
    })
    expect(wsUrl('/ws')).toBe('wss://example.com/ws')
  })
})

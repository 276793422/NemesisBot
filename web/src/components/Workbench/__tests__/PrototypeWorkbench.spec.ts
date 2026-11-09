import { mount } from '@vue/test-utils'
import { describe, it, expect, vi } from 'vitest'

// registry.ts ↔ PrototypeWorkbench.vue 双向 import（registry 注册表项引用
// 组件、组件引用 parsePayload）——spec 直接以组件为入口会反转 ESM 求值序
// （markRaw(undefined) 崩）。mock 掉 registry（parsePayload 是平凡
// JSON.parse，同义实现）断环，组件隔离测试。

vi.mock('../registry', () => ({
  parsePayload: (payload: string | null | undefined) => {
    try {
      return JSON.parse(payload || '{}') || {}
    } catch {
      return {}
    }
  },
}))

// PrototypeWorkbench 可玩预览的 src scheme 闸回归锁（BUG-20261109-026）：
// iframe 的 :src 是 URL 属性绑定——payload 源自触点载荷（模型/流水线产物），
// 未闸的 javascript:/data: 伪 URL 是 Dashboard 域内脚本执行向量（iframe src
// 的 javascript: 在 about:blank 继承父 origin 执行）。与 Deliveries/McpView
// 同款约定：href/src 属性绑定必过 safeExternalUrl（http/https 白名单），
// 非 http(s) → '' → v-if 诚实缺席。

import PrototypeWorkbench from '../PrototypeWorkbench.vue'

function wb(payload: Record<string, unknown>) {
  return mount(PrototypeWorkbench, {
    props: {
      data: { touchpoint: { payload: JSON.stringify(payload) } },
      pending: false,
      canMute: false,
    },
  })
}

describe('PrototypeWorkbench play_url scheme gate', () => {
  it('http(s) play_url 正常渲染 iframe', () => {
    const w = wb({ version: 1, machine_gate: 'pass', report: 'ok', play_url: 'https://example.com/game/index.html' })
    const frame = w.find('iframe[data-test="proto-play"]')
    expect(frame.exists()).toBe(true)
    expect(frame.attributes('src')).toBe('https://example.com/game/index.html')
  })

  it('javascript: 伪 URL → iframe 诚实缺席（不渲染）', () => {
    const w = wb({ version: 1, machine_gate: 'pass', report: 'ok', play_url: 'javascript:alert(document.domain)' })
    expect(w.find('iframe[data-test="proto-play"]').exists()).toBe(false)
  })

  it('data:text/html 伪 URL → iframe 诚实缺席', () => {
    const w = wb({ version: 1, machine_gate: 'pass', report: 'ok', play_url: 'data:text/html;base64,PHNjcmlwdD4=' })
    expect(w.find('iframe[data-test="proto-play"]').exists()).toBe(false)
  })

  it('载荷不带 play_url → 诚实缺席（既有语义不变）', () => {
    const w = wb({ version: 1, machine_gate: 'pass', report: 'ok' })
    expect(w.find('iframe[data-test="proto-play"]').exists()).toBe(false)
  })
})

import { describe, it, expect, vi } from 'vitest'
import {
  NEAR_BOTTOM_PX,
  distanceFromBottom,
  resolveFollowOnScroll,
  useStickToBottom,
} from '../useStickToBottom'

// 2026-11-09 滚动/焦点/选择批：滚动跟随（stick-to-bottom）单测。
// 纯函数直测状态机；composable 用假容器元素（可变 scrollTop/scrollHeight）
// 直调 onScroll/onWheel/stick——与监听器同一代码路径，jsdom 无需真实布局。

function fakeEl() {
  const el: any = {
    scrollTop: 0,
    scrollHeight: 1000,
    clientHeight: 500,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    scrollTo: undefined as undefined | ((opts: { top: number; behavior: string }) => void),
    getBoundingClientRect: () =>
      ({ top: 0, bottom: 0, left: 0, right: 0, width: 100, height: 500 }) as DOMRect,
    querySelectorAll: vi.fn(() => []),
  }
  return el
}

// attach 测试需要真 Node（MutationObserver.observe 拒收普通对象）：
// 真 DOM 元素 + 自有属性覆盖几何 getter（jsdom 无布局，原型 getter 恒 0）。
function fakeNodeEl(opts?: { scrollTop?: number; scrollHeight?: number; clientHeight?: number }) {
  const el = document.createElement('div')
  for (const k of ['scrollTop', 'scrollHeight', 'clientHeight'] as const) {
    Object.defineProperty(el, k, { value: opts?.[k] ?? 0, writable: true, configurable: true })
  }
  return el
}

describe('useStickToBottom 纯函数', () => {
  it('NEAR_BOTTOM_PX 钉死 80（与旧 checkUserNearBottom 阈值一致）', () => {
    expect(NEAR_BOTTOM_PX).toBe(80)
  })

  it('distanceFromBottom：scrollHeight - scrollTop - clientHeight', () => {
    expect(distanceFromBottom({ scrollTop: 100, scrollHeight: 1600, clientHeight: 500 })).toBe(1000)
    expect(distanceFromBottom({ scrollTop: 1100, scrollHeight: 1600, clientHeight: 500 })).toBe(0)
  })

  it('距底 ≤ 阈值 → 恢复跟随（无论方向）', () => {
    const m = { scrollTop: 1080, scrollHeight: 1600, clientHeight: 500 }
    expect(resolveFollowOnScroll(false, m, 1100)).toBe(true)
    expect(resolveFollowOnScroll(true, m, 1100)).toBe(true)
  })

  it('远端上移（scrollTop 减小 >1px）→ 放弃跟随', () => {
    const m = { scrollTop: 300, scrollHeight: 1600, clientHeight: 500 }
    expect(resolveFollowOnScroll(true, m, 400)).toBe(false)
    // 1px 容差内的抖动不视为上移
    expect(resolveFollowOnScroll(true, m, 301)).toBe(true)
  })

  it('远端下移但未到底 → 保持原状态', () => {
    const m = { scrollTop: 400, scrollHeight: 1600, clientHeight: 500 }
    expect(resolveFollowOnScroll(true, m, 300)).toBe(true)
    expect(resolveFollowOnScroll(false, m, 300)).toBe(false)
  })
})

describe('useStickToBottom composable', () => {
  it('stick：跟随时贴底；放弃跟随后不动作', () => {
    const el = fakeEl()
    const c = useStickToBottom(() => el)
    el.scrollHeight = 2000
    c.stick()
    expect(el.scrollTop).toBe(2000)
    c.onWheel({ deltaY: -100 } as WheelEvent)
    expect(c.follow.value).toBe(false)
    el.scrollHeight = 3000
    c.stick()
    expect(el.scrollTop).toBe(2000) // 未动
  })

  it('onScroll：远端上移释放跟随；回到底部恢复并清未读', () => {
    const el = fakeEl()
    const c = useStickToBottom(() => el)
    expect(c.follow.value).toBe(true)

    // 先下移到远端（scrollTop 增大 = 向下，未到底 → 保持 true）
    el.scrollTop = 300
    c.onScroll()
    expect(c.follow.value).toBe(true)
    // 再向上滚（scrollTop 减小）→ 放弃跟随
    el.scrollTop = 100
    c.onScroll()
    expect(c.follow.value).toBe(false)

    // 离底期间来消息计未读
    c.unreadBelow.value = 2

    // 手动滚回底部 → 恢复跟随 + 未读清零
    el.scrollHeight = 1600
    el.scrollTop = 1100
    c.onScroll()
    expect(c.follow.value).toBe(true)
    expect(c.unreadBelow.value).toBe(0)
  })

  it('attach 初采样：容器静止停在远端（滚动位置恢复场景）→ 诚实不跟随', () => {
    const el = fakeNodeEl({ scrollTop: 0, scrollHeight: 3000, clientHeight: 500 }) // 距底 2500
    const c = useStickToBottom(() => el)
    c.attach()
    expect(c.follow.value).toBe(false)
    c.detach()
  })

  it('attach 初采样：容器在底部 → 保持跟随（fresh mount 常态）', () => {
    const el = fakeNodeEl({ scrollTop: 0, scrollHeight: 0, clientHeight: 0 }) // jsdom 空内容 = 贴底
    const c = useStickToBottom(() => el)
    c.attach()
    expect(c.follow.value).toBe(true)
    c.detach()
  })

  it('jumpToBottom：无条件贴底并复位跟随', () => {
    const el = fakeEl()
    const c = useStickToBottom(() => el)
    c.onWheel({ deltaY: -50 } as WheelEvent)
    expect(c.follow.value).toBe(false)
    el.scrollHeight = 2400
    c.jumpToBottom()
    expect(c.follow.value).toBe(true)
    expect(el.scrollTop).toBe(2400)
  })

  it('scrollToBottomSmooth：有 scrollTo 走平滑；无则退化为直赋值', () => {
    const el = fakeEl()
    el.scrollTo = vi.fn()
    const c = useStickToBottom(() => el)
    c.onWheel({ deltaY: -10 } as WheelEvent)
    c.scrollToBottomSmooth()
    expect(c.follow.value).toBe(true)
    expect(el.scrollTo).toHaveBeenCalledWith({ top: 1000, behavior: 'smooth' })

    const el2 = fakeEl()
    const c2 = useStickToBottom(() => el2)
    c2.scrollToBottomSmooth()
    expect(el2.scrollTop).toBe(1000)
  })

  it('视口锚：捕获首个可见消息，重灌后按同一位置恢复', () => {
    const el = fakeEl()
    el.scrollTop = 150
    const kids = [
      { getBoundingClientRect: () => ({ top: 100, bottom: 200 }) },
      { getBoundingClientRect: () => ({ top: 300, bottom: 400 }) },
    ]
    el.querySelectorAll = vi.fn(() => kids as any)
    const c = useStickToBottom(() => el)

    const anchor = c.captureViewportAnchor()
    expect(anchor).toEqual({ index: 0, offsetTop: 100 })

    // 重灌后 DOM 重建：同序号元素被挤到 top=260（内容在上方变高）。
    ;(kids[0] as any).getBoundingClientRect = () => ({ top: 260, bottom: 360 })
    expect(c.restoreViewportAnchor(anchor)).toBe(true)
    expect(el.scrollTop).toBe(310) // 150 + (260 - 100)
  })

  it('视口锚：索引越界（锚滑出）→ false，调用方回落条件贴底', () => {
    const el = fakeEl()
    el.querySelectorAll = vi.fn(() => [] as any)
    const c = useStickToBottom(() => el)
    expect(c.restoreViewportAnchor({ index: 5, offsetTop: 0 })).toBe(false)
    expect(c.restoreViewportAnchor(null)).toBe(false)
  })

  it('resetFollow：回初值', () => {
    const el = fakeEl()
    const c = useStickToBottom(() => el)
    c.onWheel({ deltaY: -10 } as WheelEvent)
    c.unreadBelow.value = 4
    c.resetFollow()
    expect(c.follow.value).toBe(true)
    expect(c.unreadBelow.value).toBe(0)
  })

  it('attach/detach：scroll/wheel 监听接线对称', () => {
    const el = fakeNodeEl()
    const addSpy = vi.spyOn(el, 'addEventListener')
    const remSpy = vi.spyOn(el, 'removeEventListener')
    const c = useStickToBottom(() => el)
    c.attach()
    expect(addSpy).toHaveBeenCalledWith('scroll', expect.any(Function), { passive: true })
    expect(addSpy).toHaveBeenCalledWith('wheel', expect.any(Function), { passive: true })
    c.detach()
    expect(remSpy).toHaveBeenCalledWith('scroll', expect.any(Function))
    expect(remSpy).toHaveBeenCalledWith('wheel', expect.any(Function))
  })
})

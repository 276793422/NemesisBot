import { ref } from 'vue'

/**
 * 聊天滚动跟随（stick-to-bottom）——2026-11-09 滚动/焦点/选择批。
 *
 * 参照 use-stick-to-bottom（stackblitz-labs）与 DeepSeek Harness ui-chat
 * 的「滚动归属」设计，收敛为三条规则：
 * 1. 跟随态只在两种时刻放弃：滚轮上滚（即时，手势中断语义）、scroll 事件
 *    里 scrollTop 上移（方向判定，覆盖触摸/键盘滚动）；
 * 2. 距底 ≤ NEAR_BOTTOM_PX 即视为回到底部，跟随恢复、未读清零；
 * 3. 跟随态下内容变化即贴底（MutationObserver 监听滚动容器子树）——旧实现
 *    只靠 scroll 事件维护 flag，而内容增长不触发 scroll 事件，「回复到了
 *    却不滚」即由此而来。childList/subtree 覆盖消息追加、打字指示器出现、
 *    markdown 重渲染与时间线 live 更新；纯样式尺寸变化（如图片加载撑高）
 *    不发 mutation，由 @grown 事件与回复落地 stick 兜底。
 *
 * 纯函数（resolveFollowOnScroll / distanceFromBottom）与 DOM 无关，可单测；
 * composable 自身不依赖布局，jsdom（scrollHeight 恒 0）下照常运行。
 */

/** 距底多少像素内算「在底部」（与旧 checkUserNearBottom 的 80px 阈值一致）。 */
export const NEAR_BOTTOM_PX = 80

export interface ScrollMetrics {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
}

export function distanceFromBottom(m: ScrollMetrics): number {
  return m.scrollHeight - m.scrollTop - m.clientHeight
}

/**
 * scroll 事件里的跟随态转移。lastScrollTop 是上一次事件采样的 scrollTop
 * （上移判定容差 1px，吸收亚像素抖动）。
 */
export function resolveFollowOnScroll(
  prev: boolean,
  m: ScrollMetrics,
  lastScrollTop: number,
): boolean {
  if (distanceFromBottom(m) <= NEAR_BOTTOM_PX) return true
  if (m.scrollTop < lastScrollTop - 1) return false
  return prev
}

/** 重灌/重同步的视口锚：首个可见消息元素的序号 + 相对容器顶的偏移。 */
export interface ViewportAnchor {
  index: number
  offsetTop: number
}

export function useStickToBottom(getContainer: () => HTMLElement | null) {
  const follow = ref(true)
  const unreadBelow = ref(0)
  let lastScrollTop = 0
  let mo: MutationObserver | null = null

  function metrics(): ScrollMetrics | null {
    const el = getContainer()
    if (!el) return null
    return {
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
    }
  }

  function onScroll() {
    const m = metrics()
    if (!m) return
    const next = resolveFollowOnScroll(follow.value, m, lastScrollTop)
    lastScrollTop = m.scrollTop
    if (next && !follow.value) unreadBelow.value = 0
    follow.value = next
  }

  function onWheel(e: WheelEvent) {
    // 上滚手势即时放弃跟随——不等 scroll 事件采样（DSH 手势中断语义）。
    if (e.deltaY < 0) follow.value = false
  }

  /** 跟随时贴底；不跟随不动作（旧 scrollToBottomIfNear 的后继）。 */
  function stick() {
    if (!follow.value) return
    const el = getContainer()
    if (el) el.scrollTop = el.scrollHeight
  }

  /** 无条件贴底并复位跟随（会话打开 / 首拉历史——意图明确的场景）。 */
  function jumpToBottom() {
    follow.value = true
    unreadBelow.value = 0
    const el = getContainer()
    if (el) el.scrollTop = el.scrollHeight
  }

  /** 平滑滚底（发送后回看回复；动画可被用户滚轮上滚打断——浏览器原生语义）。 */
  function scrollToBottomSmooth() {
    follow.value = true
    unreadBelow.value = 0
    const el = getContainer()
    if (!el) return
    if (typeof el.scrollTo === 'function') {
      el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' })
    } else {
      // jsdom / 老内核兜底
      el.scrollTop = el.scrollHeight
    }
  }

  /** 重灌/重同步前取视口锚（取不到 = null，恢复侧回落条件贴底）。 */
  function captureViewportAnchor(): ViewportAnchor | null {
    const el = getContainer()
    if (!el) return null
    const kids = el.querySelectorAll('.message')
    const top = el.getBoundingClientRect().top
    for (let i = 0; i < kids.length; i++) {
      const r = kids[i].getBoundingClientRect()
      if (r.bottom >= top) return { index: i, offsetTop: r.top - top }
    }
    return null
  }

  /**
   * 重灌后按锚恢复视口；锚已滑出（索引越界）返回 false，调用方回落
   * 条件贴底（follow=true → 贴底，否则原地）。
   */
  function restoreViewportAnchor(a: ViewportAnchor | null): boolean {
    const el = getContainer()
    if (!a || !el) return false
    const kids = el.querySelectorAll('.message')
    if (a.index >= kids.length) return false
    const top = el.getBoundingClientRect().top
    const nowTop = kids[a.index].getBoundingClientRect().top - top
    el.scrollTop = el.scrollTop + (nowTop - a.offsetTop)
    return true
  }

  /** 会话切换 / 视图重置：回跟随初值、清未读、清采样。 */
  function resetFollow() {
    follow.value = true
    unreadBelow.value = 0
    lastScrollTop = 0
  }

  function attach() {
    const el = getContainer()
    if (!el) return
    el.addEventListener('scroll', onScroll, { passive: true })
    el.addEventListener('wheel', onWheel, { passive: true })
    mo = new MutationObserver(() => stick())
    mo.observe(el, { childList: true, subtree: true })
    // 初采样：跟随态按真实几何初始化（keep-alive 重挂/浏览器滚动位置恢复
    // 等场景停在远端时不假跟随——假跟随会在下一次内容变化时把用户拽到底）。
    const m = metrics()
    if (m) {
      lastScrollTop = m.scrollTop
      follow.value = distanceFromBottom(m) <= NEAR_BOTTOM_PX
    }
  }

  function detach() {
    const el = getContainer()
    if (el) {
      el.removeEventListener('scroll', onScroll)
      el.removeEventListener('wheel', onWheel)
    }
    mo?.disconnect()
    mo = null
  }

  return {
    follow,
    unreadBelow,
    onScroll,
    onWheel,
    stick,
    jumpToBottom,
    scrollToBottomSmooth,
    captureViewportAnchor,
    restoreViewportAnchor,
    resetFollow,
    attach,
    detach,
  }
}

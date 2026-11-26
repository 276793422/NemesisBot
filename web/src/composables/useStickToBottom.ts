import { ref } from 'vue'

/**
 * 聊天滚动跟随（stick-to-bottom）——2026-11-09 滚动/焦点/选择批。
 *
 * 参照 use-stick-to-bottom（stackblitz-labs）与 DeepSeek Harness ui-chat
 * 的「滚动归属」设计，收敛为三条规则：
 * 1. 跟随态只在两种时刻放弃：滚轮上滚（即时，手势中断语义）、scroll 采样里
 *    scrollTop 的上移量超出「钉底恒等式」能解释的量（意图判定，覆盖触摸/键盘
 *    滚动；纯几何下移不杀跟随——2026-11-25 拽底循环根修 + 2026-11-26 收缩
 *    误杀根修，见 resolveFollowOnScroll 注释）；
 * 2. 非上移采样距底 ≤ NEAR_BOTTOM_PX 即视为回到底部，跟随恢复、未读清零；
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
 * scroll 事件里的跟随态转移。last 是上一次事件采样的完整几何
 * {scrollTop, scrollHeight, clientHeight}（null = 尚无采样，跳过意图判定）。
 *
 * 意图守卫（2026-11-25 拽底循环根修 + 2026-11-26 收缩误杀根修）：
 * 用户钉底时贴底恒等式成立——scrollTop = scrollHeight - clientHeight。
 * 纯几何变化（内容收缩时浏览器 clamp / stick() 重钉 / scroll-anchoring
 * 补偿）产生的 scrollTop 下移完全由该恒等式解释：ΔscrollTop =
 * ΔscrollHeight − ΔclientHeight。用户真上滚 u px 时 ΔscrollTop 比恒等式
 * 解释的量**多减** u。故：
 *   dTop < -1 且 dTop < dGeom - 1  →  用户意图（放弃跟随）
 *   其余 scrollTop 下移（dTop == dGeom ± 抖动）  →  纯几何（不杀跟随）
 * 双 -1 容差分别吸收亚像素抖动与恒等式舍入。只看 scrollTop 的旧守卫
 * （2026-11-25 形态：scrollTop 减小 >1px 恒放弃）把 HOOK 场景的周期性
 * 收缩（live 卡折叠 / ≥3 工具卡折叠 / live→落地换装）产生的几何重钉
 * 误判成上滚 → follow 永久解除 → 后续内容全部堆积视口下方（见 BUG
 * 文档）；纯增长对话不收缩，故只有 HOOK 场景突出。
 *
 * 2026-11-25 语义保持面：用户上滚即使仍在距底 80px 圈内也判意图（旧
 * 「无论方向」臂把 onWheel 刚放弃的 follow 立刻重臂回 true，配合浮钮
 * v-if 插拔喂 MutationObserver → stick() 形成自触发拽底循环——上滚净
 * 位移零、视图钉死在底部，实机帧级 trace 实锤）；向下滚回距底 ≤80px
 * 照常恢复跟随；±1px 抖动容差照常。
 */
export function resolveFollowOnScroll(
  prev: boolean,
  m: ScrollMetrics,
  last: ScrollMetrics | null,
): boolean {
  if (last) {
    const dTop = m.scrollTop - last.scrollTop
    const dGeom =
      m.scrollHeight - last.scrollHeight - (m.clientHeight - last.clientHeight)
    if (dTop < -1 && dTop < dGeom - 1) return false
  }
  if (distanceFromBottom(m) <= NEAR_BOTTOM_PX) return true
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
  let lastSample: ScrollMetrics | null = null
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
    const next = resolveFollowOnScroll(follow.value, m, lastSample)
    lastSample = m
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
    lastSample = null
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
      lastSample = m
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

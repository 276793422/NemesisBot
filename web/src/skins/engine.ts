/**
 * 皮肤结构引擎：槽位挂载 + 声明式绑定 + 事件代理。
 *
 * 职责（原语语义见 docs/REPORT/2026-09-27_skin-structure-engine-v2.md）：
 * - `mount(slot, container)`：克隆清洗后的槽内容进容器，建立响应式绑定
 *   （每槽独立 effectScope，unmount 时整体停止——无泄漏）。
 * - `for`/`if` 为**重建式**结构原语：模板摘除留内存 + 锚注释占位，
 *   条件/数组变化时整块重建（无 key，行内状态不保持）。
 * - `bind`/`class`/`attr` 为响应式微绑定；`action` 为渲染期插值 + 容器级
 *   click 代理（closest 单命中 = 天然 stop）。
 * - `local` 为引擎本地布尔状态（`local:toggle:<name>` / `data-nb-outside`）。
 * - v3 内容区原语：`data-nb-field`（双向输入 sink）/ `data-nb-value`
 *   （单向表单值）/ `data-nb-html`（投影 HTML，信任=内置 v-html 路径）/
 *   `data-nb-autoscroll`（容器贴底）。
 * - 数据只读自投影（构造时挂入），交互只经动作白名单 + 输入 sink 注册表
 *   流出。
 *
 * 错误处理一律诚实降级：未知原语忽略、未知动作 warn-once no-op、路径
 * 缺失 → 空值语义，绝不抛出（皮肤坏一处不拖垮宿主）。
 */

import { effectScope, nextTick, reactive, watchEffect, type EffectScope } from 'vue'
import type { SkinProjection, SkinActionHandler, SkinFieldSink } from './types'

/** 作用域链项：for 行变量（值快照——行响应性由整块重建承担）。 */
interface ScopeVar {
  name: string
  value: unknown
}
type Env = ScopeVar[]

/** truthy 语义：false/null/undefined/''/空数组 = 假，其余真。 */
function truthy(v: unknown): boolean {
  if (Array.isArray(v)) return v.length > 0
  return !!v
}

const IDENT = /^[A-Za-z_$][\w$]*$/

/**
 * 路径求值：`.` 分段标识符，首段查 for 作用域（内层遮蔽外层）→ 引擎
 * local → 投影；唯一一元操作 = 首字符 `!`。任何缺失/非法 → undefined
 * （否定形式为 true）。不做表达式引擎。
 */
function resolveExpr(raw: string, local: Record<string, unknown>, env: Env, proj: SkinProjection | null): unknown {
  let expr = raw.trim()
  let neg = false
  while (expr.startsWith('!')) {
    neg = !neg
    expr = expr.slice(1).trim()
  }
  if (!expr) return neg
  const segs = expr.split('.').map((s) => s.trim())
  if (segs.some((s) => !IDENT.test(s))) return neg ? true : undefined
  const head = segs[0]
  let cur: unknown
  for (let i = env.length - 1; i >= 0; i--) {
    if (env[i].name === head) {
      cur = env[i].value
      break
    }
  }
  if (cur === undefined) {
    if (head === 'local') cur = local
    else cur = (proj as unknown as Record<string, unknown> | null)?.[head]
  }
  for (let i = 1; i < segs.length && cur != null; i++) {
    cur = (cur as Record<string, unknown>)[segs[i]]
  }
  return neg ? !truthy(cur) : cur
}

/** `cls:path;cls2:path2` → 有序对（空白容忍；坏段忽略）。 */
function parsePairs(value: string): [string, string][] {
  return value
    .split(';')
    .map((seg) => seg.trim())
    .filter(Boolean)
    .map((seg) => {
      const i = seg.indexOf(':')
      if (i <= 0) return null
      return [seg.slice(0, i).trim(), seg.slice(i + 1).trim()] as [string, string]
    })
    .filter((p): p is [string, string] => !!p && !!p[0] && !!p[1])
}

/** `name:{path}…` 渲染期插值（path 在当前 env 下求值 → 文本替换）。 */
function interpolate(raw: string, local: Record<string, unknown>, env: Env, proj: SkinProjection | null): string {
  return raw.replace(/\{([^{}]+)\}/g, (_, p: string) => {
    const v = resolveExpr(p, local, env, proj)
    return v == null ? '' : String(v)
  })
}

interface OutsideReg {
  el: () => Element | null
  path: string
}

interface SlotRuntime {
  scope: EffectScope
  container: HTMLElement
  cleanups: (() => void)[]
}

export class SkinStructureEngine {
  private slots = new Map<string, DocumentFragment>()
  private runtimes = new Map<string, SlotRuntime>()
  /** 引擎本地布尔状态（local:toggle / data-nb-outside 读写；模板可经
   * `local.<name>` 路径读取）。 */
  readonly local = reactive<Record<string, boolean>>({})
  /** 数据投影（宿主装配；null = 求值恒空）。 */
  projection: SkinProjection | null = null
  /** 动作白名单（宿主注册；唯一交互出口）。 */
  readonly actionHandlers = new Map<string, SkinActionHandler>()
  /** 输入 sink 注册表（v3 data-nb-field；宿主注册，皮肤只能写已注册项——
   * 与动作白名单同构的写通道）。 */
  readonly fieldSinks = new Map<string, SkinFieldSink>()
  private warnedActions = new Set<string>()
  private outsideRegs: OutsideReg[] = []
  private docClick: ((e: MouseEvent) => void) | null = null

  /** 载入槽位表（替换语义：先卸载全部既有运行时）。 */
  load(slots: Map<string, DocumentFragment>): void {
    this.unmountAll()
    this.slots = slots
  }

  has(name: string): boolean {
    return this.slots.has(name)
  }

  slotNames(): string[] {
    return [...this.slots.keys()]
  }

  /**
   * 挂载槽到容器（容器内容清空重建）。重复挂载同一槽 = 先卸后挂。
   * 返回 false = 槽不存在。
   */
  mount(name: string, container: HTMLElement): boolean {
    const frag = this.slots.get(name)
    if (!frag) return false
    this.unmount(name)

    const scope = effectScope()
    const cleanups: (() => void)[] = []
    scope.run(() => {
      container.textContent = ''
      const roots = Array.from(frag.cloneNode(true).childNodes)
      for (const n of roots) container.appendChild(n)
      for (const n of Array.from(container.childNodes)) {
        if (n.nodeType === 1) this.bindTree(n as Element, scope, cleanups, [])
      }
      // 容器级 click 代理（closest 单命中 = 内层动作吃掉事件，天然 stop）。
      const onClick = (e: MouseEvent) => this.handleClick(e, container)
      container.addEventListener('click', onClick)
      cleanups.push(() => container.removeEventListener('click', onClick))
      // document 级 outside 监听（共享一个；有注册才挂）。
      this.ensureDocListener()
    })
    this.runtimes.set(name, { scope, container, cleanups })
    return true
  }

  unmount(name: string): void {
    const rt = this.runtimes.get(name)
    if (!rt) return
    rt.scope.stop()
    for (const f of rt.cleanups) f()
    rt.container.textContent = ''
    this.runtimes.delete(name)
    this.outsideRegs = this.outsideRegs.filter((r) => {
      const el = r.el()
      return el != null && el.isConnected && rt.container.contains(el)
    })
    if (this.outsideRegs.length === 0) this.teardownDocListener()
  }

  unmountAll(): void {
    for (const name of [...this.runtimes.keys()]) this.unmount(name)
  }

  /** 测试/卸载收尾：文档级监听状态（无注册时为 null）。 */
  get docListenerActive(): boolean {
    return this.docClick != null
  }

  // ---- 内部 ----

  private ensureDocListener(): void {
    if (this.docClick) return
    this.docClick = (e: MouseEvent) => {
      const target = e.target as Element | null
      if (!target) return
      for (const reg of this.outsideRegs) {
        const el = reg.el()
        if (el && el.isConnected && !el.contains(target)) {
          this.local[reg.path] = false
        }
      }
    }
    document.addEventListener('click', this.docClick)
  }

  private teardownDocListener(): void {
    if (!this.docClick) return
    document.removeEventListener('click', this.docClick)
    this.docClick = null
  }

  private warnOnce(kind: string, detail: string): void {
    const key = `${kind}:${detail}`
    if (this.warnedActions.has(key)) return
    this.warnedActions.add(key)
    console.warn(`[skin-engine] ${kind}: ${detail}`)
  }

  private handleClick(e: MouseEvent, container: HTMLElement): void {
    const target = e.target as Element | null
    if (!target) return
    const el = target.closest('[data-nb-action]')
    if (!el || !container.contains(el)) return
    e.preventDefault()
    const raw = (el.getAttribute('data-nb-action') ?? '').trim()
    if (!raw) return
    if (raw.startsWith('local:toggle:')) {
      const k = raw.slice('local:toggle:'.length).trim()
      if (k) this.local[k] = !this.local[k]
      return
    }
    const i = raw.indexOf(':')
    const name = i < 0 ? raw : raw.slice(0, i).trim()
    const arg = i < 0 ? '' : raw.slice(i + 1)
    const h = this.actionHandlers.get(name)
    if (!h) {
      this.warnOnce('未知动作（no-op）', name)
      return
    }
    h(arg)
  }

  /** 递归绑定一棵已入 DOM 的子树（structural 原语模板化重建，其余就地）。 */
  private bindTree(el: Element, scope: EffectScope, cleanups: (() => void)[], env: Env): void {
    const forExpr = el.getAttribute('data-nb-for')
    const ifExpr = el.getAttribute('data-nb-if')
    if (forExpr || ifExpr) {
      this.applyStructural(el, forExpr, ifExpr, scope, env)
      return
    }
    this.bindElement(el, scope, cleanups, env)
    for (const child of Array.from(el.children)) {
      this.bindTree(child, scope, cleanups, env)
    }
  }

  /**
   * 结构原语（for/if，互斥时 for 优先）：模板摘除 + 锚注释 + watchEffect
   * 重建。子批绑定在独立子 effectScope，重建前 stop + 摘节点——无泄漏。
   */
  private applyStructural(
    el: Element,
    forExpr: string | null,
    ifExpr: string | null,
    scope: EffectScope,
    env: Env
  ): void {
    const parent = el.parentElement
    if (!parent) return
    const anchor = document.createComment(forExpr ? 'nb-for' : 'nb-if')
    parent.replaceChild(anchor, el)
    const template = el
    let childScope: EffectScope | null = null
    let nodes: Element[] = []
    const dispose = () => {
      childScope?.stop()
      childScope = null
      for (const n of nodes) n.remove()
      nodes = []
    }

    let forVar = ''
    let forPath = ''
    if (forExpr) {
      const m = /^([\w$]+)\s+in\s+(.+)$/.exec(forExpr.trim())
      if (m) {
        forVar = m[1]
        forPath = m[2]
      } else {
        this.warnOnce('data-nb-for 语法非法（原语忽略）', forExpr)
        return
      }
    }

    watchEffect(() => {
      let items: unknown[] = []
      let show = false
      if (forExpr) {
        const v = resolveExpr(forPath, this.local, env, this.projection)
        items = Array.isArray(v) ? v : []
      } else {
        show = truthy(resolveExpr(ifExpr!, this.local, env, this.projection))
      }
      dispose()
      const clones: { clone: Element; item: unknown }[] = []
      if (forExpr) {
        items.forEach((item) => {
          const clone = template.cloneNode(true) as Element
          clone.removeAttribute('data-nb-for')
          clones.push({ clone, item })
        })
      } else if (show) {
        const clone = template.cloneNode(true) as Element
        clone.removeAttribute('data-nb-if')
        clones.push({ clone, item: undefined })
      }
      if (clones.length === 0) return
      childScope = effectScope()
      childScope.run(() => {
        for (const { clone, item } of clones) {
          const childEnv: Env = item === undefined ? env : env.concat([{ name: forVar, value: item }])
          parent.insertBefore(clone, anchor)
          this.bindTree(clone, childScope!, [], childEnv)
          nodes.push(clone)
        }
      })
    })
  }

  /** 非结构性原语就地绑定（单元素）。 */
  private bindElement(el: Element, scope: EffectScope, cleanups: (() => void)[], env: Env): void {
    const bind = el.getAttribute('data-nb-bind')
    if (bind) {
      watchEffect(() => {
        const v = resolveExpr(bind, this.local, env, this.projection)
        el.textContent = v == null ? '' : String(v)
      })
    }

    // v3 data-nb-html：投影 HTML 渲染（信任等级 = 内置 v-html 路径——值
    // 来自宿主投影而非包作者；包作者 HTML 仍被 sanitize 拦在挂载前）。
    const html = el.getAttribute('data-nb-html')
    if (html) {
      watchEffect(() => {
        const v = resolveExpr(html, this.local, env, this.projection)
        ;(el as HTMLElement).innerHTML = v == null ? '' : String(v)
      })
    }

    // v3 data-nb-value：单向投影 → 表单值（预填显示；用户输入经
    // data-nb-field 回写宿主，此处写回同值无光标副作用）。
    const valueExpr = el.getAttribute('data-nb-value')
    if (valueExpr && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) {
      watchEffect(() => {
        const v = resolveExpr(valueExpr, this.local, env, this.projection)
        const s = v == null ? '' : String(v)
        if (el.value !== s) el.value = s
      })
    }

    // v3 data-nb-field：双向输入绑定——input 事件写宿主注册的 sink；
    // 未注册 sink = warn-once 丢弃（与未知动作同语义）。
    const field = el.getAttribute('data-nb-field')
    if (field && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) {
      const sinkName = field.trim()
      const onInput = () => {
        const sink = this.fieldSinks.get(sinkName)
        if (!sink) {
          this.warnOnce('未注册输入 sink（丢弃）', sinkName)
          return
        }
        sink(el.value)
      }
      el.addEventListener('input', onInput)
      cleanups.push(() => el.removeEventListener('input', onInput))
    }

    // v3 data-nb-autoscroll：绑定路径变化后贴底。更新前捕获「是否原本
    // 贴底」（pre-flush 先于 for 重建落 DOM）——用户上翻阅史时不拽回，
    // 滚回底部后恢复跟随（与内置 ChatPanel userNearBottom 同语义）。
    const autoscroll = el.getAttribute('data-nb-autoscroll')
    if (autoscroll) {
      const box = el as HTMLElement
      watchEffect(() => {
        resolveExpr(autoscroll, this.local, env, this.projection)
        const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 120
        void nextTick(() => {
          if (nearBottom) box.scrollTop = box.scrollHeight
        })
      })
    }

    const cls = el.getAttribute('data-nb-class')
    if (cls) {
      const pairs = parsePairs(cls)
      watchEffect(() => {
        for (const [c, p] of pairs) {
          el.classList.toggle(c, truthy(resolveExpr(p, this.local, env, this.projection)))
        }
      })
    }

    const attr = el.getAttribute('data-nb-attr')
    if (attr) {
      // 绑定层二次拒绝：class 交 data-nb-class 管；on* 与 URL 属性是
      // sanitize 属性黑名单的绑定侧镜像。
      const BINDING_DENIED_ATTRS = new Set([
        'href',
        'src',
        'xlink:href',
        'action',
        'formaction',
        'poster',
        'cite',
        'srcset',
      ])
      const pairs = parsePairs(attr).filter(([name]) => {
        const n = name.toLowerCase()
        return n !== 'class' && !n.startsWith('on') && !BINDING_DENIED_ATTRS.has(n)
      })
      for (const [name, p] of pairs) {
        watchEffect(() => {
          const v = resolveExpr(p, this.local, env, this.projection)
          if (v == null || v === false || v === '') el.removeAttribute(name)
          else el.setAttribute(name, String(v))
        })
      }
    }

    const actionRaw = el.getAttribute('data-nb-action')
    if (actionRaw && actionRaw.includes('{')) {
      // 渲染期插值：求值写回真实属性（click 代理读真实值）。
      watchEffect(() => {
        el.setAttribute('data-nb-action', interpolate(actionRaw, this.local, env, this.projection))
      })
    }

    const outside = el.getAttribute('data-nb-outside')
    if (outside) {
      const path = outside.trim()
      this.outsideRegs.push({ el: () => el, path })
      cleanups.push(() => {
        this.outsideRegs = this.outsideRegs.filter((r) => r.el() !== el)
      })
    }
  }
}

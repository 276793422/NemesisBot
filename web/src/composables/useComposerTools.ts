import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * 会话工具状态单例（2026-11-19 资源窗「会话工具」分节）。
 *
 * 模块级单例（useEditorMode / useFileTreePanel 同款）：ChatPanel 与
 * SessionResourcesPanel 共享同一份状态——思考强度（sessionEffort + 探针
 * 标记）、分享/委派弹窗触发器。
 *
 * 分工（拆分迁移拍板）：
 * - 状态真值只有一份（本模块导出的 ref），两个组件读写同一份；
 * - 弹窗渲染（ShareModal / DelegateDialog）仍留在 ChatPanel——资源窗按钮
 *   只置触发器 ref，渲染点不迁移（挂载点与 effectiveSid 解析都在 ChatPanel）；
 * - effort 探针标记拉取（loadEffortMarks）与进会话对齐（syncSessionEffort）
 *   的调用时机仍在 ChatPanel（WS ready / 会话切换钩子在那边），资源窗只读。
 *
 * EFFORT_TIERS_ORDER 与后端 capability::EFFORT_TIERS 对齐（7 档平行数组）。
 */

// --- G3: 会话级思考强度 ---
export const sessionEffort = ref('') // '' = 默认（无 override，跟随模型配置）

// effort 探针标记（probe 二期）：models.list 取默认模型的
// effort_marks/effort_notes（后端 effort_wire_target 方言映射，前端零
// 重建），思考强度下拉渲染 ✓/✗/?。拉取失败 = 空标记（诚实不标）。
export const EFFORT_TIERS_ORDER = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
export const effortMarks = ref<string[]>([])
export const effortNotes = ref<string[]>([])

// --- 弹窗触发器（渲染仍在 ChatPanel） ---
/** L4：会话分享弹窗开关（原工具栏 🔗 按钮 → 资源窗「会话工具」分节）。 */
export const showShare = ref(false)
/** 角色目录与客户端委派（2026-09-28）：委派弹窗开关（原工具栏按钮 → 资源窗）。 */
export const showDelegate = ref(false)

export function useComposerTools() {
  const { request } = useWSAPI()
  const toast = useToast()

  function effortMarkFor(value: string): string {
    // 空数组 = models.list 拉取失败（无数据可比对）→ 不渲染标记（与
    // 「探测过但未定」的 ? 相区分——前者是数据面不可用，后者是判定面未知）。
    if (effortMarks.value.length === 0) return ''
    const i = EFFORT_TIERS_ORDER.indexOf(value)
    if (i < 0) return ''
    const mark = effortMarks.value[i]
    if (mark === 'ok') return ' ✓'
    if (mark === 'bad') return ' ✗'
    return ' ?'
  }

  function effortNoteFor(value: string): string {
    const i = EFFORT_TIERS_ORDER.indexOf(value)
    return (i >= 0 ? effortNotes.value[i] : '') || ''
  }

  async function loadEffortMarks() {
    try {
      const data = await request('models', 'list', {}, 8000)
      const rows: Array<{ is_default?: boolean; effort_marks?: string[]; effort_notes?: string[] }> =
        data?.models || []
      const def = rows.find((m) => m.is_default)
      effortMarks.value = def?.effort_marks || []
      effortNotes.value = def?.effort_notes || []
    } catch {
      effortMarks.value = []
      effortNotes.value = []
    }
  }

  /**
   * 进会话 / 重连时对齐一次 override（失败保持当前值，不炸 UI）。
   * currentSid 闭包由调用方给（晚到回包围栏：响应落地时会话已切走则丢弃）。
   */
  function syncSessionEffort(sid: string, currentSid: () => string) {
    if (!sid) return
    request('chat', 'get_effort', { session_id: sid })
      .then((data) => {
        if (currentSid() !== sid) return
        sessionEffort.value = typeof data?.effort === 'string' ? data.effort : ''
      })
      .catch(() => {})
  }

  /**
   * 下拉切换（乐观更新 + 失败回滚；'' = 清除 override 回落 config 默认）。
   * 接显式值不接 Event——调用方（资源窗/工具栏 select）各自解 target。
   */
  async function setSessionEffort(sid: string, v: string) {
    const prev = sessionEffort.value
    sessionEffort.value = v
    try {
      await request('chat', 'set_effort', {
        session_id: sid || '',
        effort: v === '' ? null : v,
      })
    } catch (err) {
      sessionEffort.value = prev
      toast.error(String((err as Error)?.message ?? err))
    }
  }

  return {
    sessionEffort,
    EFFORT_TIERS_ORDER,
    effortMarks,
    effortNotes,
    showShare,
    showDelegate,
    effortMarkFor,
    effortNoteFor,
    loadEffortMarks,
    syncSessionEffort,
    setSessionEffort,
  }
}

/** 测试辅助：重置模块级单例（生产代码勿用；useEditorMode 同款惯例）。 */
export function _resetComposerToolsForTest() {
  sessionEffort.value = ''
  effortMarks.value = []
  effortNotes.value = []
  showShare.value = false
  showDelegate.value = false
}

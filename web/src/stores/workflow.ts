/**
 * Workflow store — central state for the entire Workflow UI.
 *
 * The store is the single source of truth for:
 *   - TAB 1 (list): the cached list of workflow summaries + driver-status map
 *   - TAB 2 (canvas) / TAB 4 (yaml): the in-progress editing copy + dirty flag
 *   - TAB 3 (history): run list, selected-run detail, checkpoint list/detail
 *
 * All reads/writes go through `useWorkflowApi` so the components never
 * call WSAPI directly — easier to mock in tests and to add caching later.
 */

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { useWorkflowApi } from '../composables/useWorkflowApi'
import type {
  NodeListResponse,
  WorkflowSummary,
  TriggerDriverStatus,
  WorkflowDef,
  ExecutionSummary,
  ExecutionDetail,
  CheckpointMeta,
  Checkpoint,
} from '../types/workflow'
import type {
  DraftSummary,
  DraftDetail,
  DraftDiff,
} from '../composables/wfEditSessions'

export const useWorkflowStore = defineStore('workflow', () => {
  const api = useWorkflowApi()

  // === TAB 1: list ===
  const workflows = ref<WorkflowSummary[]>([])
  const driverStatus = ref<Record<string, TriggerDriverStatus>>({})
  const listLoading = ref(false)
  const listError = ref<string | null>(null)
  const lastListFetch = ref<number>(0)

  // === TAB 2/4: editor (shared) ===
  const editing = ref<WorkflowDef | null>(null)
  const editingDirty = ref(false)
  const editingIsNew = ref(false)
  const validationErrors = ref<string[]>([])

  // === TAB 3: history ===
  const runs = ref<ExecutionSummary[]>([])
  const runsLoading = ref(false)
  const runsError = ref<string | null>(null)
  const selectedRun = ref<ExecutionDetail | null>(null)
  const checkpoints = ref<CheckpointMeta[]>([])
  const selectedCheckpoint = ref<Checkpoint | null>(null)

  // === TAB 5: agentGen（对话生成，2026-09-22） ===
  /** 待应用草稿列表（draft_list 全量，mtime 降序）。 */
  const drafts = ref<DraftSummary[]>([])
  const draftsLoading = ref(false)
  const draftsError = ref<string | null>(null)
  /** 当前对话指向的目标工作流（进入 agentGen 时锁定；null = 新建）。 */
  const agentGenTarget = ref<string | null>(null)
  /**
   * 画布草稿预览状态：非 null 时画布进入 draft-preview 第三数据态——
   * 渲染 draftPreview 定义但禁止一切编辑（palette 隐藏、交互禁用、
   * diff 高亮）。切走 agentGen TAB 时由 WorkflowAgentGen 清空。
   */
  const draftPreview = ref<DraftDetail | null>(null)
  /**
   * 草稿 ↔ 已注册定义的节点级 diff（进入预览时算好存住；null = 全新
   * 工作流或未取到旧定义）。画布用它给节点上 added/removed/changed 高亮。
   */
  const draftPreviewDiff = ref<DraftDiff | null>(null)

  // === Lifecycle ===
  const activeTab = ref<'list' | 'canvas' | 'history' | 'yaml' | 'agentGen'>('list')

  // === Computed ===
  const workflowByName = computed(() => {
    const m: Record<string, WorkflowSummary> = {}
    for (const w of workflows.value) m[w.name] = w
    return m
  })

  const hasUndrivenTriggers = computed(() => {
    return workflows.value.some(wf =>
      wf.triggers.some(t => !t.driven),
    )
  })

  // === Actions: list ===
  async function fetchList(force = false) {
    if (listLoading.value) return
    // Cache for 5s unless forced — saves a round-trip on tab switches.
    if (!force && Date.now() - lastListFetch.value < 5000 && workflows.value.length > 0) {
      return
    }
    listLoading.value = true
    listError.value = null
    try {
      const resp: NodeListResponse = await api.list()
      workflows.value = resp.workflows ?? []
      driverStatus.value = resp.trigger_driver_status ?? {}
      lastListFetch.value = Date.now()
    } catch (e) {
      listError.value = typeof e === 'string' ? e : '加载工作流列表失败'
    } finally {
      listLoading.value = false
    }
  }

  function clearListCache() {
    lastListFetch.value = 0
  }

  // === Actions: editor ===
  async function loadForEdit(name: string) {
    const resp = await api.get(name)
    editing.value = resp.workflow
    editingDirty.value = false
    editingIsNew.value = false
    validationErrors.value = []
  }

  function startNewWorkflow() {
    editing.value = {
      name: '',
      description: '',
      version: '1.0.0',
      triggers: [],
      nodes: [],
      edges: [],
      variables: {},
      metadata: {},
    }
    // Do NOT mark dirty on a blank workflow — only when the user actually
    // edits a field. Otherwise the canvas always shows "unsaved" and the
    // tab-change guard fires on every navigation.
    editingDirty.value = false
    editingIsNew.value = true
    validationErrors.value = []
  }

  /** Discard the in-progress edit. Called when the user confirms "放弃修改"
   * in the tab-change guard, or explicitly via the 丢弃 button on the canvas. */
  function discardEditing() {
    editing.value = null
    editingDirty.value = false
    editingIsNew.value = false
    validationErrors.value = []
  }

  async function saveEditing(): Promise<{ ok: true } | { ok: false; error: string }> {
    if (!editing.value) return { ok: false, error: 'no workflow in editor' }
    try {
      if (editingIsNew.value) {
        await api.create(editing.value)
        editingIsNew.value = false
      } else {
        await api.update(editing.value.name, editing.value)
      }
      editingDirty.value = false
      clearListCache() // list will refetch on next visit
      return { ok: true }
    } catch (e) {
      return { ok: false, error: typeof e === 'string' ? e : String(e) }
    }
  }

  async function deleteWorkflow(name: string): Promise<{ ok: true } | { ok: false; error: string }> {
    try {
      await api.delete(name)
      // Local state cleanup
      workflows.value = workflows.value.filter(w => w.name !== name)
      if (editing.value?.name === name) {
        editing.value = null
        editingDirty.value = false
      }
      return { ok: true }
    } catch (e) {
      return { ok: false, error: typeof e === 'string' ? e : String(e) }
    }
  }

  async function validateEditing() {
    if (!editing.value) return
    try {
      const resp = await api.validate(editing.value)
      validationErrors.value = resp.errors ?? []
      return resp.valid
    } catch (e) {
      validationErrors.value = [typeof e === 'string' ? e : String(e)]
      return false
    }
  }

  async function validateRaw(workflow: WorkflowDef): Promise<{ valid: boolean; errors: string[] }> {
    try {
      const resp = await api.validate(workflow)
      return { valid: resp.valid, errors: resp.errors ?? [] }
    } catch (e) {
      return { valid: false, errors: [typeof e === 'string' ? e : String(e)] }
    }
  }

  async function runNow(name: string, input: Record<string, unknown>): Promise<string | null> {
    try {
      const resp = await api.runNow(name, input)
      return resp.execution_id
    } catch {
      return null
    }
  }

  /**
   * Publish a trigger-event into the engine's EventDispatcher. Used by the
   * canvas page's "⚡ 模拟事件" button. Returns the list of workflows that
   * matched the event (so the UI can show "已触发：X、Y、Z").
   */
  async function fireEvent(
    eventType: string,
    data: Record<string, unknown>,
  ): Promise<{ published: boolean; matched: string[] }> {
    try {
      const resp = await api.fireEvent(eventType, data)
      return { published: resp.published, matched: resp.matched_workflows ?? [] }
    } catch (e) {
      return { published: false, matched: [] }
    }
  }

  // === Actions: history ===
  async function fetchRuns(filter: { workflow_name?: string; state?: string; limit?: number } = {}) {
    if (runsLoading.value) return
    runsLoading.value = true
    runsError.value = null
    try {
      const resp = await api.listExecutions(filter)
      runs.value = resp.executions ?? []
    } catch (e) {
      runsError.value = typeof e === 'string' ? e : '加载执行历史失败'
    } finally {
      runsLoading.value = false
    }
  }

  async function fetchRunDetail(executionId: string) {
    try {
      selectedRun.value = await api.status(executionId)
    } catch (e) {
      selectedRun.value = null
      throw e
    }
  }

  async function fetchCheckpoints(executionId: string) {
    try {
      const resp = await api.listCheckpoints(executionId)
      checkpoints.value = resp.checkpoints ?? []
    } catch {
      checkpoints.value = []
    }
  }

  async function fetchCheckpoint(executionId: string, checkpointId: string) {
    try {
      const resp = await api.getCheckpoint(executionId, checkpointId)
      selectedCheckpoint.value = resp.checkpoint
    } catch {
      selectedCheckpoint.value = null
    }
  }

  async function cancelRun(executionId: string) {
    await api.cancel(executionId)
    await fetchRunDetail(executionId)
  }

  async function resumeRun(executionId: string, review: Record<string, unknown>) {
    await api.resume(executionId, review)
    await fetchRunDetail(executionId)
  }

  // === Actions: agentGen（对话生成草稿面板） ===

  /** 拉取全部待应用草稿（进入 TAB / 会话切换 / tool_event 提醒时调用）。 */
  async function fetchDrafts(force = false) {
    if (draftsLoading.value) return
    if (!force && drafts.value.length === 0 && draftsError.value === null && draftsFetchedOnce) {
      return
    }
    draftsLoading.value = true
    draftsError.value = null
    try {
      const resp = await api.draftList()
      drafts.value = resp.drafts ?? []
      draftsFetchedOnce = true
    } catch (e) {
      draftsError.value = typeof e === 'string' ? e : '加载草稿列表失败'
    } finally {
      draftsLoading.value = false
    }
  }
  let draftsFetchedOnce = false

  /** 应用草稿（转正 + 消费），成功后刷新列表与主列表缓存。 */
  async function applyDraft(name: string): Promise<{ ok: true; replaced: boolean } | { ok: false; error: string }> {
    try {
      const resp = await api.draftApply(name)
      await fetchDrafts(true)
      clearListCache()
      return { ok: true, replaced: resp.replaced_existing }
    } catch (e) {
      return { ok: false, error: typeof e === 'string' ? e : String(e) }
    }
  }

  /** 丢弃草稿（幂等），成功后刷新列表。 */
  async function discardDraft(name: string): Promise<{ ok: true } | { ok: false; error: string }> {
    try {
      await api.draftDiscard(name)
      await fetchDrafts(true)
      // 若画布正在预览这份草稿，同步退出预览态
      if (draftPreview.value?.name === name) {
        draftPreview.value = null
        draftPreviewDiff.value = null
      }
      return { ok: true }
    } catch (e) {
      return { ok: false, error: typeof e === 'string' ? e : String(e) }
    }
  }

  /** 进入画布草稿预览（第三数据态）。diff 由调用方算好传入（需旧定义）。 */
  function enterDraftPreview(detail: DraftDetail, diff: DraftDiff | null = null) {
    draftPreview.value = detail
    draftPreviewDiff.value = diff
    setActiveTab('canvas')
  }

  /** 退出画布草稿预览。 */
  function exitDraftPreview() {
    draftPreview.value = null
    draftPreviewDiff.value = null
  }

  // === Navigation ===
  function setActiveTab(tab: 'list' | 'canvas' | 'history' | 'yaml' | 'agentGen') {
    // 草稿预览态绑定 agentGen 视图：从画布切走即退出预览（防「幽灵预览」
    // 残留到用户手动编辑流程）。画布内部切到 history/yaml 也一样。
    if (tab !== 'canvas' && draftPreview.value) {
      draftPreview.value = null
      draftPreviewDiff.value = null
    }
    activeTab.value = tab
  }

  return {
    // WSAPI 封装（草稿面板等组件直接取用；store 内 action 也走它）
    api,
    // list state
    workflows,
    driverStatus,
    listLoading,
    listError,
    hasUndrivenTriggers,
    workflowByName,
    fetchList,
    clearListCache,
    // editor state
    editing,
    editingDirty,
    editingIsNew,
    validationErrors,
    loadForEdit,
    startNewWorkflow,
    discardEditing,
    saveEditing,
    deleteWorkflow,
    validateEditing,
    validateRaw,
    runNow,
    fireEvent,
    // history state
    runs,
    runsLoading,
    runsError,
    selectedRun,
    checkpoints,
    selectedCheckpoint,
    fetchRuns,
    fetchRunDetail,
    fetchCheckpoints,
    fetchCheckpoint,
    cancelRun,
    resumeRun,
    // agentGen（对话生成）
    drafts,
    draftsLoading,
    draftsError,
    agentGenTarget,
    draftPreview,
    draftPreviewDiff,
    fetchDrafts,
    applyDraft,
    discardDraft,
    enterDraftPreview,
    exitDraftPreview,
    // navigation
    activeTab,
    setActiveTab,
  }
})

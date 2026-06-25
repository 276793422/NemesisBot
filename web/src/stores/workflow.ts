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

  // === Lifecycle ===
  const activeTab = ref<'list' | 'canvas' | 'history' | 'yaml'>('list')

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

  // === Navigation ===
  function setActiveTab(tab: 'list' | 'canvas' | 'history' | 'yaml') {
    activeTab.value = tab
  }

  return {
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
    // navigation
    activeTab,
    setActiveTab,
  }
})

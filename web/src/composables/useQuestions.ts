import { reactive, ref } from 'vue'
import { on, off } from './useSSE'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * F7（devtool-upgrade 阶段 5）：Dashboard 结构化提问卡状态。
 *
 * 后端 WebQuestionBroker 在 agent 的 `question` 工具发起提问时：
 * - 广播 SSE `question-asked`（payload = AgentEvent::QuestionAsked 的 data：
 *   question_id / question / options / multi / timeout_secs / chat_id /
 *   session_key；`question.pending` 列表另带 age_secs）；
 * - 阻塞等待 WSAPI `question.respond`（超时不是错误：广播
 *   `question-resolved{decision:"timeout"}`，模型按最佳判断继续）。
 *
 * 本组合式是全局单例（同 useToast/useApprovals）：initQuestions 订阅 SSE +
 * 拉一次 `question.pending` 补断线漏掉的事件；respond 成功或
 * `question-resolved` 广播到达（任一窗口作答/服务端超时）后本地摘卡；
 * 倒计时到期仅本地摘卡（服务端届时已超时放行，模型的选择会出现在聊天流）。
 */

export interface PendingQuestion {
  question_id: string
  question: string
  options: string[]
  /** true = 可选多项（checkbox）；false = 单选（radio）。 */
  multi: boolean
  timeout_secs: number
  chat_id?: string
  session_key?: string
  /** 本地到期时刻（ms epoch）= 接收时刻 + (timeout_secs - age_secs) * 1000 */
  expiresAt: number
}

const pendingQuestions = reactive<PendingQuestion[]>([])
/** 每秒跳动的当前时刻，驱动卡片倒计时显示。 */
const now = ref(Date.now())
let initialized = false
let ticker: ReturnType<typeof setInterval> | null = null

function upsertFromPayload(data: any) {
  if (!data || typeof data.question_id !== 'string') return
  if (!Array.isArray(data.options)) return
  const existing = pendingQuestions.findIndex(q => q.question_id === data.question_id)
  const age = typeof data.age_secs === 'number' ? data.age_secs : 0
  const timeout = typeof data.timeout_secs === 'number' ? data.timeout_secs : 0
  const entry: PendingQuestion = {
    question_id: data.question_id,
    question: String(data.question ?? ''),
    options: data.options.map((o: unknown) => String(o)),
    multi: data.multi === true,
    timeout_secs: timeout,
    chat_id: data.chat_id || undefined,
    session_key: data.session_key || undefined,
    expiresAt: Date.now() + Math.max(0, timeout - age) * 1000,
  }
  if (existing === -1) {
    pendingQuestions.push(entry)
  } else {
    // SSE 重复推送同一问题（重连重放等）— 保留原到期时刻，避免倒计时重置。
    entry.expiresAt = pendingQuestions[existing].expiresAt
    pendingQuestions.splice(existing, 1, entry)
  }
}

function removeLocal(questionId: string) {
  const idx = pendingQuestions.findIndex(q => q.question_id === questionId)
  if (idx !== -1) pendingQuestions.splice(idx, 1)
}

/** 本地倒计时到期：只摘卡片，服务端超时已放行（模型选择走聊天流）。 */
function pruneExpired() {
  now.value = Date.now()
  for (let i = pendingQuestions.length - 1; i >= 0; i--) {
    if (pendingQuestions[i].expiresAt <= now.value) {
      pendingQuestions.splice(i, 1)
    }
  }
}

async function seedPending() {
  const { request } = useWSAPI()
  try {
    const res = await request('question', 'pending', {}, 5000)
    const list: any[] = res?.pending ?? []
    list.forEach(upsertFromPayload)
  } catch {
    // 未装配 / WS 未就绪 — 静默（未装配实例永远不会有事件推送）。
  }
}

export function useQuestions() {
  const toast = useToast()

  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initQuestions() {
    if (initialized) return
    initialized = true

    on('question-asked', (data: any) => {
      upsertFromPayload(data)
    })
    // 任一窗口作答成功 / 服务端超时 → 所有窗口静默摘卡（竞速败方无感关闭，
    // 也不弹错误 toast——同 approval-resolved 先例）。
    on('question-resolved', (data: any) => {
      if (data && typeof data.question_id === 'string') {
        removeLocal(data.question_id)
      }
    })
    // 断线期间错过的事件：连上后拉一次 pending 补齐。
    seedPending()
    ticker = setInterval(pruneExpired, 1000)
  }

  /** 提交作答。后端校验失败（空选择/非候选/单选多项）请求留在 pending
   * 可重试——本地卡片保留；unknown/not wired（已超时/另一窗口已答）→
   * 同步摘卡。 */
  async function respondTo(questionId: string, selected: string[]) {
    const { request } = useWSAPI()
    try {
      await request('question', 'respond', {
        question_id: questionId,
        selected,
      })
      removeLocal(questionId)
      toast.success('已提交回答')
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('unknown') || msg.includes('not wired')) {
        removeLocal(questionId)
      }
      toast.error(`回答提交失败: ${msg}`)
    }
  }

  return { pendingQuestions, now, initQuestions, respondTo }
}

/** 测试辅助：重置模块级单例状态（生产代码勿用）。 */
export function _resetQuestionsForTest() {
  pendingQuestions.splice(0, pendingQuestions.length)
  if (ticker !== null) {
    clearInterval(ticker)
    ticker = null
  }
  initialized = false
}

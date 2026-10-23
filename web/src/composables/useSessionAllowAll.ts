import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * 会话级全部放行开关状态（2026-10-24 长任务无人值守计划，Feature A）。
 *
 * 作用域是**单个会话**：开启后本会话所有安全审批卡（含 CRITICAL + 紫微
 * force）自动放行——硬拒绝（hard-deny）与急停不在放行之列。无 TTL、无
 * 自动关闭：持久真相源 = 会话 sidecar `allow_all` 字段，重启后自动恢复
 * （与 Full Access 的「运行时态重启失效」刻意相反——本开关是会话属性）。
 *
 * 模块级单例（同 useEditorMode）：聊天框旁按钮 / 编辑条常驻横幅 /
 * SettingsView【编辑器】TAB 行全部读这里——单一真相源。按会话 id 分桶
 * （allowAllBySession），切换会话时由消费方 refresh 对齐。
 *
 * WSAPI：`security.session_allow {session_id, enabled}`（set，响应带
 * set 后快照）/ `security.session_allow_status {session_id}`（刷新）。
 * `injected: false`（security 未启用实例）= sessionAllowAvailable=false，
 * 按钮诚实禁用。
 */

export interface SessionAllowStatus {
  enabled: boolean
  /** 开启时间（RFC3339；未开启 = null）。 */
  since: string | null
  /** 累计自动批准条数。 */
  count: number
}

/** session_id → 当前状态（仅含已查询过的会话；未查询 = 无键）。 */
export const allowAllBySession = ref<Record<string, SessionAllowStatus>>({})
export const sessionAllowAvailable = ref(true)

function applyPayload(sessionId: string, data: any) {
  if (!data) return
  if (data.injected === false) {
    sessionAllowAvailable.value = false
    return
  }
  if (typeof data.enabled === 'boolean') {
    allowAllBySession.value = {
      ...allowAllBySession.value,
      [sessionId]: {
        enabled: data.enabled,
        since: data.since ?? null,
        count: typeof data.count === 'number' ? data.count : 0,
      },
    }
  }
}

export function useSessionAllowAll() {
  const toast = useToast()

  /** 拉取单会话状态（seed / 切会话对齐）。暂时性失败静默（保持旧态）。 */
  async function refreshSessionAllow(sessionId: string | null | undefined) {
    if (!sessionId) return
    const { request } = useWSAPI()
    try {
      const res = await request('security', 'session_allow_status', { session_id: sessionId })
      applyPayload(sessionId, res)
    } catch (err: any) {
      if (String(err ?? '').includes('未装配')) sessionAllowAvailable.value = false
      // 其余失败静默——开关保持旧态（安全缺省 = 不放行）。
    }
  }

  /** 翻转开关（UI 只发意图；生效值以服务端响应为准）。 */
  async function setSessionAllow(sessionId: string | null | undefined, enabled: boolean) {
    if (!sessionId) return
    const { request } = useWSAPI()
    try {
      const res = await request('security', 'session_allow', {
        session_id: sessionId,
        enabled,
      })
      applyPayload(sessionId, res)
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('未装配')) sessionAllowAvailable.value = false
      toast.error(`切换会话放行失败: ${msg}`)
    }
  }

  return { allowAllBySession, sessionAllowAvailable, refreshSessionAllow, setSessionAllow }
}

/** 测试辅助：重置模块级单例（生产代码勿用）。 */
export function _resetSessionAllowForTest() {
  allowAllBySession.value = {}
  sessionAllowAvailable.value = true
}

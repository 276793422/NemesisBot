import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// BUG-20261031-253：rewind 成功 toast 的文件回滚计数恒显示「写 0 / 删 0」。
// - 根因：AgentLoop rewind/redo 响应把恢复文件清单放嵌套 `restored_files`
//   下（rewind.rs 成功路径 json!），rewindToastSummary 读的是顶层
//   `resp?.written` / `resp?.deleted`——恒 undefined → `?? 0` 恒 0。
//   出生即失配（8e80caa2，2026-09-07 同提交内 BE 嵌套、FE 读顶层）。
// - 修复：FE 改读 `resp?.restored_files?.written?.length`。
// 本 spec 用真实响应形状（嵌套 restored_files）钉住 toast 计数非零。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  wsStatus: null as any,
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  h.wsStatus = ref<'connected' | 'disconnected'>('connected')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { h.wsHandler = fn }),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: h.wsStatus,
  }
})

import ChatPanel from '../ChatPanel.vue'
import { sendHistoryRequest } from '../../composables/useWebSocket'
import { useSessionStore } from '../../stores/session'
import { useToast } from '../../composables/useToast'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  // 默认全成功空响应；rewind 用例内覆写。
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

function prepareSessions() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  s.currentId = 's1'
}

function feedHistory(requestId: string, messages: any[]) {
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastHistoryRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

/** 2026-11-12「⋯」菜单迁移：回退按钮收编进三点菜单（Teleport to body →
 *  wrapper.find 找不到，须经 document.body）。 */
async function clickMsgMenuItem(w: any, role: 'user' | 'assistant', itemText: string) {
  const row = w.findAll('.message.' + role).at(-1)
  expect(row, role + ' 消息行应存在').toBeTruthy()
  await row!.find('.msg-menu-trigger').trigger('click')
  await flushPromises()
  const item = Array.from(document.querySelectorAll('.msg-menu-item')).find(b =>
    b.textContent?.includes(itemText),
  )
  expect(item, '「⋯」菜单应含「' + itemText + '」项').toBeTruthy()
  ;(item as HTMLElement).dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await flushPromises()
}

describe('ChatPanel rewind toast 文件回滚计数（restored_files 嵌套键）', () => {
  it('rewind 成功响应（嵌套 restored_files）→ toast 显示真实写/删计数而非恒 0', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    // AgentLoop rewind_from_cut 成功路径的真实响应形状（rewind.rs json!）：
    // 文件清单在嵌套 restored_files 下。
    requestMock.mockImplementation(async (_mod: string, cmd: string) => {
      if (cmd === 'rewind_to_message') {
        return {
          session_key: 's1',
          message_index: 0,
          kept_count: 0,
          removed_count: 2,
          file_restore: 'applied',
          restored_files: { written: ['src/lib.rs'], deleted: ['tmp/old.rs'] },
          redoable: true,
          forced: false,
        }
      }
      return {} // resync / 其他命令默认空响应
    })

    // user 行的「⏪ 撤销此问」（⋯ 菜单内）→ rewindToTurnStart →
    // rewindToastSummary。
    await clickMsgMenuItem(w, 'user', '撤销此问')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('sessions', 'rewind_to_message', {
      session_id: 's1',
      message_index: 0,
      mode: 'before',
      anchor_content: 'q1',
    })
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.message.includes('写 1 / 删 1')),
      `toast 应显示真实计数「写 1 / 删 1」，实际：${toasts.map(t => t.message).join(' | ')}`,
    ).toBe(true)
    w.unmount()
  })

  // 2026-11-12 M4 对称补全回归锁：后端波及面默认拒绝有两清单——conflicts
  // （外部修改）+ other_session_paths（其他会话波及）。此前前端只数
  // conflicts（波及场景误导显示「0 个文件」）且无 force 通道（redo 有确认
  // 流，rewind 没有，用户被拦后无解）。修复后：分列展示 + confirm 确认 +
  // force 重试（与 doRedo 同构）。活体实锚（run9）：fork 后源会话继续
  // 写文件 → 分支回退被 other_session_paths 拦（conflicts=0）。
  it('blocked(conflicts 空 + other_session_paths) → confirm 文案分列 → force 重试成功', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    let rewindCalls = 0
    requestMock.mockImplementation(async (_mod: string, cmd: string, data: any) => {
      if (cmd === 'rewind_to_message') {
        rewindCalls++
        if (!data?.force) {
          // AgentLoop rewind_from_cut blocked 响应的真实形状（rewind.rs
          // json!）：conflicts 空数组 + other_session_paths 非空。
          return {
            blocked: true,
            reason: 'conflict',
            session_key: 's1',
            message_index: 0,
            restore_turn: 2,
            conflicts: [],
            unchecked_paths: [],
            other_session_paths: ['C:\\ws\\agents\\x\\home\\uploads\\v.png'],
          }
        }
        return {
          session_key: 's1',
          message_index: 0,
          kept_count: 0,
          removed_count: 2,
          file_restore: 'applied',
          restored_files: { written: [], deleted: [] },
          redoable: true,
          forced: true,
        }
      }
      return {}
    })

    await clickMsgMenuItem(w, 'user', '撤销此问')
    await flushPromises()

    // 第一次调用无 force，确认后重试带 force:true。
    expect(rewindCalls).toBe(2)
    expect(confirmSpy).toHaveBeenCalledTimes(1)
    const confirmText = confirmSpy.mock.calls[0]?.[0] ?? ''
    expect(confirmText).toContain('其他会话的改动')
    expect(confirmText).toContain('v.png')
    expect(requestMock).toHaveBeenLastCalledWith('sessions', 'rewind_to_message', {
      session_id: 's1',
      message_index: 0,
      mode: 'before',
      force: true,
    })
    // force 成功后走 rewindToastSummary，不残留 blocked 错误 toast。
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.type === 'error'),
      `不应有 error toast，实际：${toasts.map(t => t.type + ':' + t.message).join(' | ')}`,
    ).toBe(false)
    confirmSpy.mockRestore()
    w.unmount()
  })

  it('blocked 且用户取消 confirm → 不发 force 重试、无成功 toast', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false)
    let rewindCalls = 0
    requestMock.mockImplementation(async (_mod: string, cmd: string) => {
      if (cmd === 'rewind_to_message') {
        rewindCalls++
        return {
          blocked: true,
          reason: 'conflict',
          conflicts: [],
          other_session_paths: ['b.txt'],
        }
      }
      return {}
    })

    await clickMsgMenuItem(w, 'user', '撤销此问')
    await flushPromises()

    expect(rewindCalls).toBe(1) // 取消 → 不重试
    // toasts 是全局 reactive 单例——清掉前序用例残留后再断言本用例零产出。
    useToast().toasts.splice(0)
    await flushPromises()
    const toasts = useToast().toasts
    expect(toasts.some(t => t.message.includes('已分叉') || t.message.includes('写 '))).toBe(false)
    confirmSpy.mockRestore()
    w.unmount()
  })

  // 复查收窄回归修复（2026-11-12）：blocked ≠ 只有 conflict——restore_failed
  // /commit_failed（rewind.rs 522/553，文件恢复/日志写回失败，同样零改动可
  // 重试）必须诚实报错终止。旧代码宽条件 if(resp?.blocked) 全拦；本批窄化
  // 成 reason==='conflict' 单臂时曾把这两种 blocked 穿透到 rewindToastSummary
  // 假成功 toast（「已回退：撤掉 0 条消息；文件未动」）——本用例钉死非
  // conflict 臂：不弹 confirm、不重试、error toast 带后端 note、无成功 toast。
  it('blocked(restore_failed) → 诚实 error toast、不 confirm、不重试', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    const confirmSpy = vi.spyOn(window, 'confirm')
    let rewindCalls = 0
    requestMock.mockImplementation(async (_mod: string, cmd: string) => {
      if (cmd === 'rewind_to_message') {
        rewindCalls++
        // AgentLoop rewind blocked restore_failed 的真实形状（rewind.rs 521-532）。
        return {
          blocked: true,
          reason: 'restore_failed',
          session_key: 's1',
          message_index: 0,
          restore_turn: 1,
          file_restore: 'failed',
          restore_failures: [{ path: 'C:\\ws\\src\\lib.rs', error: 'os error 5' }],
          note: '文件恢复不完整，会话对话与 checkpoint 均未改动——解决失败原因后可重试本操作',
        }
      }
      return {}
    })

    await clickMsgMenuItem(w, 'user', '撤销此问')
    await flushPromises()

    expect(rewindCalls).toBe(1)
    expect(confirmSpy).not.toHaveBeenCalled()
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.type === 'error' && t.message.includes('回退未执行') && t.message.includes('文件恢复不完整')),
      `应有带 note 的 error toast，实际：${toasts.map(t => t.type + ':' + t.message).join(' | ')}`,
    ).toBe(true)
    expect(toasts.some(t => t.type === 'success')).toBe(false)
    confirmSpy.mockRestore()
    w.unmount()
  })

  // doRedo 同类洞对齐（2026-11-12 复查横向扫）：redo 前向恢复不完整
  // （rewind.rs 904 起 restore_failed blocked，条目还栈可重试）旧窄条件会
  // 穿透到「已重做上一次回退」假成功 toast。钉死：诚实 error、无 confirm、
  // 无成功 toast。
  it('redo blocked(restore_failed) → 诚实 error toast、不 confirm、无「已重做」', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    const confirmSpy = vi.spyOn(window, 'confirm')
    let redoCalls = 0
    requestMock.mockImplementation(async (_mod: string, cmd: string) => {
      if (cmd === 'redo') {
        redoCalls++
        // redo 前向恢复不完整 blocked 真实形状（rewind.rs 904-917）。
        return {
          blocked: true,
          reason: 'restore_failed',
          session_key: 's1',
          restore_turn: 1,
          file_restore: 'partial',
          restore_failures: [{ path: 'C:\\ws\\src\\a.rs', error: 'os error 5' }],
          note: '文件恢复不完整，会话对话未改动——解决失败原因后可重试本操作',
        }
      }
      return {}
    })

    const redoBtn = w.find('.redo-btn')
    expect(redoBtn.exists(), '重做按钮应渲染').toBe(true)
    await redoBtn.trigger('click')
    await flushPromises()

    expect(redoCalls).toBe(1)
    expect(confirmSpy).not.toHaveBeenCalled()
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.type === 'error' && t.message.includes('重做未执行')),
      `应有「重做未执行」error toast，实际：${toasts.map(t => t.type + ':' + t.message).join(' | ')}`,
    ).toBe(true)
    expect(toasts.some(t => t.message.includes('已重做'))).toBe(false)
    confirmSpy.mockRestore()
    w.unmount()
  })

  // doRedo force 重试后仍可 blocked（force 只强过 conflict 预检，restore 类
  // 失败照样返回）——旧代码无二次检查，穿透「已重做」假成功。钉死二次
  // blocked 检查臂。
  it('redo confirm 后 force 重试仍 blocked → 二次诚实 error、无「已重做」', async () => {
    prepareSessions()
    const w = mount(ChatPanel)
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()

    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    let redoCalls = 0
    requestMock.mockImplementation(async (_mod: string, cmd: string, data: any) => {
      if (cmd === 'redo') {
        redoCalls++
        if (!data?.force) {
          // redo 冲突预检 blocked 真实形状（rewind.rs 842-848）。
          return {
            blocked: true,
            reason: 'conflict',
            session_key: 's1',
            drifted_paths: ['C:\\ws\\src\\lib.rs'],
            note: '回退窗口内外部修改了以上文件，redo 的文件恢复将覆盖这些改动；确认覆盖请带 force 重试',
          }
        }
        // force 强过 conflict 但撞上前向恢复失败（rewind.rs 904 起，force 不豁免）。
        return {
          blocked: true,
          reason: 'restore_failed',
          session_key: 's1',
          restore_turn: 1,
          file_restore: 'failed',
          restore_failures: [{ path: 'C:\\ws\\src\\a.rs', error: 'os error 5' }],
          note: '文件恢复不完整，会话对话未改动——解决失败原因后可重试本操作',
        }
      }
      return {}
    })

    const redoBtn = w.find('.redo-btn')
    await redoBtn.trigger('click')
    await flushPromises()

    expect(redoCalls).toBe(2)
    expect(confirmSpy).toHaveBeenCalledTimes(1)
    const toasts = useToast().toasts
    expect(
      toasts.some(t => t.type === 'error' && t.message.includes('强制重做仍被拒绝')),
      `应有二次 error toast，实际：${toasts.map(t => t.type + ':' + t.message).join(' | ')}`,
    ).toBe(true)
    expect(toasts.some(t => t.message.includes('已重做'))).toBe(false)
    confirmSpy.mockRestore()
    w.unmount()
  })
})

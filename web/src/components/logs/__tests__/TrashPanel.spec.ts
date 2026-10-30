import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// 会话回收站页签（2026-10-30）dispatch 级 spec：
// - 列表渲染：标题（无 title 回退 display_id）/ 原 sid / 时间 / 条数 / 大小 /
//   来源徽标 / 孤儿徽标；空态文案；
// - 直接恢复成功 → toast + trashList 重拉；
// - 拍板② 冲突改名流：`{conflict:true, suggested_sid}` → 改名模态 + 输入框
//   预填建议 sid → 确认带 new_sid 重试；空 sid 拒发；
// - 彻底删除（单条）/清空回收站：二次确认模态，取消不发请求，确认才调
//   trash_purge / trash_purge_all。
// 后端语义（移入/恢复冲突结构化返回/purge）由
// crates/nemesis-agent/src/chat_trash/trash_tests.rs +
// crates/nemesis-web/src/handlers/sessions_trash_tests.rs 钉住。

const trashListMock = vi.fn()
const trashRestoreMock = vi.fn()
const trashPurgeMock = vi.fn()
const trashPurgeAllMock = vi.fn()

vi.mock('../../../composables/useChatApi', () => ({
  useChatApi: () => ({
    trashList: (...a: any[]) => trashListMock(...a),
    trashRestore: (...a: any[]) => trashRestoreMock(...a),
    trashPurge: (...a: any[]) => trashPurgeMock(...a),
    trashPurgeAll: (...a: any[]) => trashPurgeAllMock(...a),
  }),
}))

import TrashPanel from '../TrashPanel.vue'

function row(over: Record<string, unknown> = {}) {
  return {
    entry: '1760000000000_conflict1',
    original_session_key: 'agent:main:session:conflict1',
    display_id: 'conflict1',
    title: '旧会话标题',
    deleted_at_ms: 1760000000000,
    message_count: 12,
    bytes: 2048,
    source: 'single',
    orphan: false,
    ...over,
  }
}

beforeEach(() => {
  trashListMock.mockReset().mockResolvedValue({ entries: [] })
  trashRestoreMock.mockReset()
  trashPurgeMock.mockReset()
  trashPurgeAllMock.mockReset()
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(TrashPanel)
  await flushPromises()
  return w
}

describe('TrashPanel 会话回收站（2026-10-30）', () => {
  it('列表渲染：标题/原 sid/条数/大小/来源徽标；无 title 回退 display_id；孤儿徽标；空态', async () => {
    const w = await mountView()
    expect(w.find('[data-test="trash-empty"]').exists()).toBe(true)
    expect(w.find('[data-test="trash-purge-all-btn"]').exists()).toBe(false)

    trashListMock.mockResolvedValue({
      entries: [
        row(),
        row({
          entry: '1760000001000_batchy',
          display_id: 'batchy',
          title: null,
          source: 'batch',
          orphan: true,
        }),
      ],
    })
    const w2 = mount(TrashPanel)
    await flushPromises()

    const rows = w2.findAll('[data-test="trash-row"]')
    expect(rows.length).toBe(2)
    expect(rows[0].text()).toContain('旧会话标题')
    expect(rows[0].text()).toContain('conflict1')
    expect(rows[0].text()).toContain('12')
    expect(rows[0].text()).toContain('2.0 KB')
    expect(rows[0].text()).toContain('单个删除')
    // 无 title → display_id 回退；来源徽标 + 孤儿徽标
    expect(rows[1].text()).toContain('batchy')
    expect(rows[1].text()).toContain('批量删除')
    expect(rows[1].find('.orphan-badge').exists()).toBe(true)
    // 空态消失，清空按钮出现
    expect(w2.find('[data-test="trash-purge-all-btn"]').exists()).toBe(true)
    w.unmount()
    w2.unmount()
  })

  it('直接恢复成功：调 trashRestore(entry)、toast 成功、列表重拉', async () => {
    trashListMock.mockResolvedValue({ entries: [row()] })
    trashRestoreMock.mockResolvedValue({ restored: 'agent:main:session:conflict1' })
    const w = await mountView()

    await w.find('[data-test="trash-restore-btn"]').trigger('click')
    await flushPromises()

    expect(trashRestoreMock).toHaveBeenCalledTimes(1)
    expect(trashRestoreMock).toHaveBeenCalledWith('1760000000000_conflict1', undefined)
    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'success' && t.message.includes('已恢复'))).toBe(true)
    // 成功后重拉（初次挂载 1 次 + 成功后 1 次）
    expect(trashListMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('拍板② 冲突改名流：conflict 返回 → 模态预填建议 sid；确认带 new_sid 重试；空 sid 拒发', async () => {
    trashListMock.mockResolvedValue({ entries: [row()] })
    trashRestoreMock
      .mockResolvedValueOnce({ conflict: true, suggested_sid: 'conflict1-restored-1760000002000' })
      .mockResolvedValueOnce({ restored: 'agent:main:session:conflict1-restored-1760000002000', renamed: true })
    const w = await mountView()

    await w.find('[data-test="trash-restore-btn"]').trigger('click')
    await flushPromises()

    // 结构化冲突 → 改名模态 + 输入框预填建议 sid；条目不消耗（列表未清空）
    const modal = w.find('[data-test="trash-conflict-modal"]')
    expect(modal.exists()).toBe(true)
    const input = modal.find('[data-test="trash-conflict-sid"]')
    expect((input.element as HTMLInputElement).value).toBe('conflict1-restored-1760000002000')

    // 空 sid 拒发
    await input.setValue('   ')
    await modal.find('[data-test="trash-conflict-confirm"]').trigger('click')
    expect(trashRestoreMock).toHaveBeenCalledTimes(1)

    // 预填建议 sid 确认 → 带 new_sid 重试
    await input.setValue('conflict1-restored-1760000002000')
    await modal.find('[data-test="trash-conflict-confirm"]').trigger('click')
    await flushPromises()
    expect(trashRestoreMock).toHaveBeenCalledTimes(2)
    expect(trashRestoreMock).toHaveBeenLastCalledWith(
      '1760000000000_conflict1',
      'conflict1-restored-1760000002000',
    )
    expect(w.find('[data-test="trash-conflict-modal"]').exists()).toBe(false)
    w.unmount()
  })

  it('彻底删除（单条）：二次确认——取消不发请求，确认调 trash_purge 并重拉', async () => {
    trashListMock.mockResolvedValue({ entries: [row()] })
    const w = await mountView()

    await w.find('[data-test="trash-purge-btn"]').trigger('click')
    const modal = w.find('[data-test="trash-purge-modal"]')
    expect(modal.exists()).toBe(true)
    expect(modal.text()).toContain('不可逆')

    // 取消：不发请求
    await modal.find('[data-test="trash-purge-cancel"]').trigger('click')
    expect(trashPurgeMock).not.toHaveBeenCalled()

    // 确认：调 trash_purge(entry)
    await w.find('[data-test="trash-purge-btn"]').trigger('click')
    await w.find('[data-test="trash-purge-confirm"]').trigger('click')
    await flushPromises()
    expect(trashPurgeMock).toHaveBeenCalledWith('1760000000000_conflict1')
    expect(trashListMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('清空回收站：二次确认——取消不发请求，确认调 trash_purge_all', async () => {
    trashListMock.mockResolvedValue({ entries: [row(), row({ entry: 'e2', display_id: 's2' })] })
    trashPurgeAllMock.mockResolvedValue({ purged: 2 })
    const w = await mountView()

    await w.find('[data-test="trash-purge-all-btn"]').trigger('click')
    const modal = w.find('[data-test="trash-purge-all-modal"]')
    expect(modal.exists()).toBe(true)
    expect(modal.text()).toContain('2')

    await modal.find('[data-test="trash-purge-all-cancel"]').trigger('click')
    expect(trashPurgeAllMock).not.toHaveBeenCalled()

    await w.find('[data-test="trash-purge-all-btn"]').trigger('click')
    await w.find('[data-test="trash-purge-all-confirm"]').trigger('click')
    await flushPromises()
    expect(trashPurgeAllMock).toHaveBeenCalledTimes(1)
    w.unmount()
  })
})

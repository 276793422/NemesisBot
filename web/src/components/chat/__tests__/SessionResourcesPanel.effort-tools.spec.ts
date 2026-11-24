import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { computed } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

// 会话工具分节（2026-11-19 拆分迁移 → 九键全迁）：原 ChatPanel 输入框
// 工具栏九键在有资源窗注入面（ChatView provide）时不再渲染，控件的家在
// SessionResourcesPanel：
// - 会话工具行：构建/计划 + 插队 + 分享/委派/思考强度（低频配置键不再
//   占输入近邻）；
// - 放行开关分节：Full Access / 外部写删 / 全部放行；
// - 语音分节：听写 / 语音对话 / 语音播放 / 实时语音。
// - 选项集 = ''+EFFORT_TIERS 七档（顺序钉死，G3 2026-11-09 扩档）；
// - set_effort 乐观更新 + 失败回滚（useComposerTools 单例内）；
// - effort 探针标记（probe 二期）渲染 ✓/✗/?；
// - ChatPanel 侧回归锁：裸挂载（standalone/workflow 挂载面）工具栏保留
//   九键；有 provide 注入面时工具栏整体不渲染 + 资源窗开关按钮在场；
//   语音激活态常驻条（voice-strip）随单例亮。
// 状态真值 = 各模块级单例——测试间逐一 _resetXxxForTest。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import SessionResourcesPanel from '../SessionResourcesPanel.vue'
import ChatPanel from '../../ChatPanel.vue'
import {
  _resetComposerToolsForTest,
  effortMarks,
  effortNotes,
  showShare,
  showDelegate,
} from '../../../composables/useComposerTools'
import {
  _resetEditorModeForTest,
  fullAccess,
  editorAvailable,
} from '../../../composables/useEditorMode'
import {
  _resetSessionAllowForTest,
  allowAllBySession,
  sessionAllowAvailable,
} from '../../../composables/useSessionAllowAll'
import {
  _resetVoiceControlsForTest,
  sttReady,
  ttsReady,
  voiceDictation,
  voiceDialogue,
  voicePlayback,
  voiceRealtime,
} from '../../../composables/useVoiceControls'
import { RESOURCES_PANEL_CTL, type ResourcesPanelCtl } from '../../../composables/useResourcesPanel'
import { useChatStore } from '../../../stores/chat'
import { useSessionStore } from '../../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  _resetComposerToolsForTest()
  _resetEditorModeForTest()
  _resetSessionAllowForTest()
  _resetVoiceControlsForTest()
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
})

function mountPanel(sessionId: string | null) {
  return mount(SessionResourcesPanel, { props: { sessionId } })
}

function effortSelect(w: ReturnType<typeof mount>) {
  return w.find('select.srp-effort-select')
}

describe('会话工具分节 · 思考强度下拉', () => {
  it('8 项选项集（默认 + off + 6 wire 档）顺序钉死', async () => {
    const w = mountPanel('s1')
    await flushPromises()
    const sel = effortSelect(w)
    expect(sel.exists()).toBe(true)
    const values = sel.findAll('option').map((o) => (o.element as HTMLOptionElement).value)
    expect(values).toEqual(['', 'off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'])
    expect((sel.element as HTMLSelectElement).value).toBe('')
    w.unmount()
  })

  it('选 xhigh → set_effort {effort:"xhigh"}；选回默认 → effort:null', async () => {
    const w = mountPanel('s1')
    await flushPromises()
    const sel = effortSelect(w)
    await sel.setValue('xhigh')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_effort', {
      session_id: 's1',
      effort: 'xhigh',
    })
    await sel.setValue('')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_effort', {
      session_id: 's1',
      effort: null,
    })
    w.unmount()
  })

  it('set_effort 失败 → 回滚到前值（不炸 UI）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_effort') return Promise.reject(new Error('unknown effort'))
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await effortSelect(w).setValue('xhigh')
    await flushPromises()
    expect((effortSelect(w).element as HTMLSelectElement).value).toBe('')
    w.unmount()
  })

  it('sessionId 缺席 → 会话域键全部禁用（无会话域不发请求）；非会话域键照常可用', async () => {
    const w = mountPanel(null)
    await flushPromises()
    const byText = (t: string) =>
      w.findAll('.srp-tool-btn').filter((b) => b.text().includes(t))
    // 会话域键禁用：分享/委派/思考强度 + 外部写删（FA 关连锁）+ 全部放行 +
    // 语音四键（引擎未就绪缺省禁用同形态——实时语音双引擎闸，2026-11-20）。
    for (const t of ['分享', '委派', '外部写删', '全部放行', '听写', '语音对话', '语音播放', '实时语音']) {
      const btns = byText(t)
      expect(btns.length).toBeGreaterThan(0)
      expect((btns[0].element as HTMLButtonElement).disabled).toBe(true)
    }
    expect((effortSelect(w).element as HTMLSelectElement).disabled).toBe(true)
    // 非会话域键可用：构建/插队/Full Access（引擎位与模式是全局态，不依赖
    // 具体会话；实时语音已并入引擎闸，不再在此列）。
    for (const t of ['构建', '插队', 'Full Access']) {
      const btns = byText(t)
      expect(btns.length).toBeGreaterThan(0)
      expect((btns[0].element as HTMLButtonElement).disabled).toBe(false)
    }
    w.unmount()
  })
})

describe('会话工具分节 · effort 探针标记（probe 二期）', () => {
  function markText(w: ReturnType<typeof mount>, value: string) {
    const opt = effortSelect(w)
      .findAll('option')
      .find((o) => (o.element as HTMLOptionElement).value === value)!
    return opt.text()
  }

  it('effort_marks → ✓/✗/? 渲染（后端方言映射，前端零重建）', async () => {
    // 标记真值由 ChatPanel 的 loadEffortMarks 写入单例（调用时机在那边）；
    // 本测试直接 seed 单例验证渲染投影。
    effortMarks.value = ['ok', 'unknown', 'ok', 'bad', 'unknown', 'ok', 'ok']
    effortNotes.value = ['', '', '', 'status 400: unsupported value medium', '', '', '']
    const w = mountPanel('s1')
    await flushPromises()
    expect(markText(w, 'low')).toContain('✓')
    expect(markText(w, 'medium')).toContain('✗')
    expect(markText(w, 'high')).toContain('?')
    expect(markText(w, 'xhigh')).toContain('✓')
    // off 恒 NotSent（不发送=无验证对象）：不渲染标记。
    expect(markText(w, 'off')).not.toContain('✓')
    expect(markText(w, 'off')).not.toContain('?')
    // 拒绝原文进 option title（tooltip）。
    const med = effortSelect(w)
      .findAll('option')
      .find((o) => (o.element as HTMLOptionElement).value === 'medium')!
    expect(med.attributes('title')).toContain('unsupported value medium')
    w.unmount()
  })

  it('marks 空（拉取失败/未探测）→ 全部无标记回落（诚实未知）', async () => {
    const w = mountPanel('s1')
    await flushPromises()
    expect(markText(w, 'low')).toBe('思考·低')
    expect(markText(w, 'max')).toBe('思考·最大')
    w.unmount()
  })
})

describe('会话工具分节 · 分享/委派触发器', () => {
  it('点分享 → 单例 showShare 翻转；点委派 → showDelegate 翻转', async () => {
    const w = mountPanel('s1')
    await flushPromises()
    expect(showShare.value).toBe(false)
    expect(showDelegate.value).toBe(false)
    await w.findAll('.srp-tool-btn').filter((b) => b.text().includes('分享'))[0].trigger('click')
    await w.findAll('.srp-tool-btn').filter((b) => b.text().includes('委派'))[0].trigger('click')
    expect(showShare.value).toBe(true)
    expect(showDelegate.value).toBe(true)
    w.unmount()
  })
})

describe('九键全迁 · 构建/计划与插队', () => {
  /** 会话工具行首键 = 构建/计划（分节第一张 srp-tools 卡的第一个按钮）。 */
  function modeBtn(w: ReturnType<typeof mount>) {
    return w.find('.srp-card.srp-tools .srp-tool-btn')
  }

  it('点构建 → chat.set_mode plan；回包落 store + 文案/激活态翻转；再点切回', async () => {
    const store = useChatStore()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_mode') return Promise.resolve({ mode: 'plan' })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    expect(modeBtn(w).text()).toContain('构建')
    await modeBtn(w).trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'set_mode', { session_id: 's1', mode: 'plan' })
    expect(store.agentMode).toBe('plan')
    expect(modeBtn(w).text()).toContain('计划')
    expect(modeBtn(w).classes()).toContain('active')
    // 切回构建。
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_mode') return Promise.resolve({ mode: 'build' })
      return Promise.resolve({})
    })
    await modeBtn(w).trigger('click')
    await flushPromises()
    expect(store.agentMode).toBe('build')
    expect(modeBtn(w).text()).toContain('构建')
    expect(modeBtn(w).classes()).not.toContain('active')
    w.unmount()
  })

  it('set_mode 失败 → toast 不翻转（await 回包后才落 store）', async () => {
    const store = useChatStore()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_mode') return Promise.reject(new Error('boom'))
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await modeBtn(w).trigger('click')
    await flushPromises()
    expect(store.agentMode).not.toBe('plan')
    expect(modeBtn(w).text()).toContain('构建')
    w.unmount()
  })

  it('插队键默认在场（inbox 快照缺席回落 steer）；点击给输入加 ! 前缀，已有前缀不动', async () => {
    const store = useChatStore()
    const w = mountPanel('s1')
    await flushPromises()
    // 2026-11-24 发送方式三键迁入后，面板有两个含「插队」的 .srp-tool-btn
    //（会话工具卡的一键前缀 + 发送方式卡的 ⚡插队路由键）——本用例锚定
    // 会话工具卡那颗（prefixSteer 语义），按卡标题过滤。
    const toolsCard = () =>
      w.findAll('.srp-card').find((c) => c.find('.srp-card-title').text() === '会话工具')!
    const steer = () => toolsCard().findAll('.srp-tool-btn').filter((b) => b.text().includes('插队'))
    expect(steer().length).toBe(1)
    // 无前缀 → 加；已有半角/全角前缀 → 不动（单前缀，不叠加）。
    store.input = '修复登录 bug'
    await steer()[0].trigger('click')
    expect(store.input).toBe('! 修复登录 bug')
    await steer()[0].trigger('click')
    expect(store.input).toBe('! 修复登录 bug')
    store.input = '！已全角'
    await steer()[0].trigger('click')
    expect(store.input).toBe('！已全角')
    w.unmount()
  })

  it('inbox 快照明确 mode=reject → 插队键隐藏（显式配置照常尊重）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'inbox_status')
        return Promise.resolve({ available: true, mode: 'reject', next_turn: 0, next_step: 0, capacity: 8, busy: false })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    expect(w.findAll('.srp-tool-btn').filter((b) => b.text().includes('插队')).length).toBe(0)
    w.unmount()
  })
})

describe('九键全迁 · 放行开关分节', () => {
  function byText(w: ReturnType<typeof mount>, t: string) {
    return w.findAll('.srp-tool-btn').filter((b) => b.text().includes(t))[0]
  }

  it('点 Full Access 开 → editor.set {full:true, ext:保持}；服务端生效值回写（UI 只发意图）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ full_access: true, external_write: false })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, 'Full Access').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('editor', 'set', { full_access: true, external_write: false })
    expect(fullAccess.value).toBe(true)
    expect(byText(w, 'Full Access').classes()).toContain('active')
    // 外部写删解锁（依赖 FA）。
    expect((byText(w, '外部写删').element as HTMLButtonElement).disabled).toBe(false)
    w.unmount()
  })

  it('点 Full Access 关 → 显式双关 {full:false, ext:false}（服务端联动会把 (false,true) 折回 full=true）', async () => {
    fullAccess.value = true
    const { externalWrite } = await import('../../../composables/useEditorMode')
    externalWrite.value = true
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, 'Full Access').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('editor', 'set', { full_access: false, external_write: false })
    w.unmount()
  })

  it('点外部写删 → 显式带 full:true（依赖闸：按钮仅在 FA 开时可点）', async () => {
    fullAccess.value = true
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ full_access: true, external_write: true })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, '外部写删').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('editor', 'set', { full_access: true, external_write: true })
    const { externalWrite } = await import('../../../composables/useEditorMode')
    expect(externalWrite.value).toBe(true)
    expect(byText(w, '外部写删').classes()).toContain('active')
    w.unmount()
  })

  it('security 未装配 → Full Access 禁用（editorAvailable=false 诚实降级）', async () => {
    editorAvailable.value = false
    const w = mountPanel('s1')
    await flushPromises()
    expect((byText(w, 'Full Access').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })

  it('点全部放行 → security.session_allow {session_id, enabled:true}；回包落分桶 + 激活态', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'session_allow') return Promise.resolve({ enabled: true, since: 't0', count: 0 })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, '全部放行').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'session_allow', { session_id: 's1', enabled: true })
    expect(allowAllBySession.value['s1']?.enabled).toBe(true)
    expect(byText(w, '全部放行').classes()).toContain('active')
    // 再点关（enabled 取自当前态取反）。
    await byText(w, '全部放行').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('security', 'session_allow', { session_id: 's1', enabled: false })
    w.unmount()
  })

  it('security 未装配 → 全部放行禁用', async () => {
    sessionAllowAvailable.value = false
    const w = mountPanel('s1')
    await flushPromises()
    expect((byText(w, '全部放行').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })
})

describe('九键全迁 · 语音分节', () => {
  function byText(w: ReturnType<typeof mount>, t: string) {
    return w.findAll('.srp-tool-btn').filter((b) => b.text().includes(t))[0]
  }

  it('引擎未就绪 → 听写/语音对话/语音播放/实时语音全禁用且 title 指路（2026-11-20 双引擎闸）', async () => {
    const w = mountPanel('s1')
    await flushPromises()
    for (const t of ['听写', '语音对话', '语音播放']) {
      const btn = byText(w, t)
      expect((btn.element as HTMLButtonElement).disabled).toBe(true)
      expect(btn.attributes('title')).toContain('语音通道页')
    }
    // 实时语音依赖 STT+TTS 双引擎——任一未就绪即禁用，title 指路双引擎。
    const rt = byText(w, '实时语音')
    expect((rt.element as HTMLButtonElement).disabled).toBe(true)
    expect(rt.attributes('title')).toContain('语音通道页')
    expect(rt.attributes('title')).toContain('双引擎')
    w.unmount()
  })

  /** 挂载后 initVoiceState 会拉 engine_status 覆盖引擎位——引擎真值必须
   *  走 mock（直接 seed ref 会被覆盖回 false）。 */
  function mountWithEngines2(stt: boolean, tts: boolean) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'engine_status') return Promise.resolve({ stt_ready: stt, tts_ready: tts })
      if (cmd === 'voice_config_get') return Promise.resolve({ silence_timeout: 3 })
      return Promise.resolve({})
    })
    return mountPanel('s1')
  }

  it('仅单引擎就绪（只 STT 或只 TTS）→ 实时语音仍禁用（双引擎缺一不可）；双就绪 → 放行', async () => {
    let w = mountWithEngines2(true, false)
    await flushPromises()
    expect((byText(w, '实时语音').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
    w = mountWithEngines2(false, true)
    await flushPromises()
    expect((byText(w, '实时语音').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
    w = mountWithEngines2(true, true)
    await flushPromises()
    expect((byText(w, '实时语音').element as HTMLButtonElement).disabled).toBe(false)
    w.unmount()
  })

  function mountWithEngines() {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'engine_status') return Promise.resolve({ stt_ready: true, tts_ready: true })
      if (cmd === 'voice_config_get') return Promise.resolve({ silence_timeout: 3 })
      return Promise.resolve({})
    })
    return mountPanel('s1')
  }

  it('点听写 → stt_to_input_start + 单例亮（激活态）；再点 → stop + 熄', async () => {
    const w = mountWithEngines()
    await flushPromises()
    const btn = byText(w, '听写')
    expect((btn.element as HTMLButtonElement).disabled).toBe(false)
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('voice', 'stt_to_input_start')
    expect(voiceDictation.value).toBe(true)
    expect(byText(w, '听写').classes()).toContain('active')
    await byText(w, '听写').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('voice', 'stt_to_input_stop')
    expect(voiceDictation.value).toBe(false)
    w.unmount()
  })

  it('点语音对话 → stt_dialogue_start 带 session_id（G6 realtime 接力寻址：sessionId 显式传参）', async () => {
    const w = mountWithEngines()
    await flushPromises()
    await byText(w, '语音对话').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('voice', 'stt_dialogue_start', {
      silence_timeout: 3,
      session_id: 's1',
    })
    expect(voiceDialogue.value).toBe(true)
    w.unmount()
  })

  it('点语音播放开 → 纯配置态无起停请求；关 → tts_playback_stop', async () => {
    const w = mountWithEngines()
    await flushPromises()
    await byText(w, '语音播放').trigger('click')
    await flushPromises()
    expect(voicePlayback.value).toBe(true)
    expect(byText(w, '语音播放').classes()).toContain('active')
    expect(requestMock).not.toHaveBeenCalledWith('voice', 'tts_playback_start')
    await byText(w, '语音播放').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('voice', 'tts_playback_stop')
    expect(voicePlayback.value).toBe(false)
    w.unmount()
  })

  it('点实时语音 → chat_config_set read-merge-write（realtime.enabled 翻转，其余键保留）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'engine_status') return Promise.resolve({ stt_ready: true, tts_ready: true })
      if (cmd === 'voice_config_get') return Promise.resolve({ silence_timeout: 3 })
      if (cmd === 'chat_config_get')
        return Promise.resolve({ keep: 1, realtime: { enabled: false, stream_asi: true } })
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, '实时语音').trigger('click')
    await flushPromises()
    const setCall = requestMock.mock.calls.find((c) => c[0] === 'voice' && c[1] === 'chat_config_set')
    expect(setCall).toBeTruthy()
    const payload = setCall![2] as Record<string, any>
    expect(payload.keep).toBe(1) // 全文为底：非管辖键不被抹
    expect(payload.realtime.enabled).toBe(true)
    expect(payload.realtime.stream_asi).toBe(true) // realtime 块合并：子键保留
    expect(voiceRealtime.value).toBe(true)
    expect(byText(w, '实时语音').classes()).toContain('active')
    w.unmount()
  })

  it('chat_config_get 读不到 → toggleRealtime 回滚本地态（不盲写空 base）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'engine_status') return Promise.resolve({ stt_ready: true, tts_ready: true })
      if (cmd === 'voice_config_get') return Promise.resolve({ silence_timeout: 3 })
      if (cmd === 'chat_config_get') return Promise.resolve(null)
      return Promise.resolve({})
    })
    const w = mountPanel('s1')
    await flushPromises()
    await byText(w, '实时语音').trigger('click')
    await flushPromises()
    expect(voiceRealtime.value).toBe(false) // 翻转后回滚
    expect(requestMock.mock.calls.some((c) => c[1] === 'chat_config_set')).toBe(false)
    w.unmount()
  })

  it('toggleRealtime 运行时闸：引擎未就绪 → 不翻转不写盘（按钮 disabled 双保险，2026-11-20）', async () => {
    // 直接压 composable 层闸（按钮 disabled 时 jsdom 不派发 click，闸是
    // 第二道防线：存量恢复态/引擎中途掉线的 engine_fault 场景）。
    const { useVoiceControls } = await import('../../../composables/useVoiceControls')
    const { toggleRealtime } = useVoiceControls()
    await toggleRealtime()
    expect(voiceRealtime.value).toBe(false)
    expect(requestMock.mock.calls.some((c) => c[1] === 'chat_config_get')).toBe(false)
    expect(requestMock.mock.calls.some((c) => c[1] === 'chat_config_set')).toBe(false)
  })

  it('toggleRealtime 关臂不受引擎闸：引擎中途掉线（engine_fault）仍可关（开臂独占闸）', async () => {
    voiceRealtime.value = true // 存量开着，引擎掉了
    sttReady.value = false
    ttsReady.value = false
    const { useVoiceControls } = await import('../../../composables/useVoiceControls')
    const { toggleRealtime } = useVoiceControls()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'chat_config_get')
        return Promise.resolve({ keep: 1, realtime: { enabled: true } })
      return Promise.resolve({})
    })
    await toggleRealtime()
    expect(voiceRealtime.value).toBe(false)
    const setCall = requestMock.mock.calls.find((c) => c[0] === 'voice' && c[1] === 'chat_config_set')
    expect((setCall![2] as Record<string, any>).realtime.enabled).toBe(false)
  })
})

describe('ChatPanel 侧（迁移回归锁）', () => {
  beforeEach(() => {
    useSessionStore().currentId = 's1'
  })

  it('工具栏三键已移除（不许回潮）；挂载仍发 get_effort 对齐', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get_effort') return Promise.resolve({ effort: 'high' })
      return Promise.resolve({})
    })
    const w = mount(ChatPanel)
    await flushPromises()
    // 下拉/分享/委派三键不在工具栏（已迁资源窗）。
    expect(w.find('select.effort-select').exists()).toBe(false)
    const texts = w.findAll('.voice-toolbar button').map((b) => b.text())
    expect(texts.some((t) => t.includes('分享'))).toBe(false)
    expect(texts.some((t) => t.includes('委派'))).toBe(false)
    // 对齐调用时机保留在 ChatPanel（进会话 syncSessionEffort）。
    expect(requestMock).toHaveBeenCalledWith('chat', 'get_effort', { session_id: 's1' })
    w.unmount()
  })

  it('裸挂载（standalone/workflow 面）工具栏保留九键平铺：无 vt-row；无注入面时资源窗开关不渲染', async () => {
    const w = mount(ChatPanel)
    await flushPromises()
    // 三行包装 .vt-row 不复存在（单行 flex 容器直接装按钮）。
    expect(w.find('.vt-row').exists()).toBe(false)
    const texts = w.findAll('.voice-toolbar button').map((b) => b.text())
    // 九键在场（插队键随 steer 配置可选，不参与钉死断言）。
    for (const t of ['构建', 'Full Access', '外部写删', '全部放行', '听写', '语音对话', '语音播放', '实时语音']) {
      expect(texts.some((x) => x.includes(t))).toBe(true)
    }
    // 【收起工具栏】按钮退役：toolbar-toggle 系列不再出现工具栏字样 title
    //（会话列表开关 title 是「显示/隐藏会话列表」，不受影响）。
    const toggleTitles = w.findAll('button.toolbar-toggle').map((b) => b.attributes('title'))
    expect(toggleTitles.some((t) => t?.includes('工具栏'))).toBe(false)
    // 资源窗开关经注入供给（本 describe 裸挂载无 ChatView provide → 不渲染，无死按钮）。
    expect(toggleTitles.some((t) => t?.includes('资源窗'))).toBe(false)
    w.unmount()
  })

  it('有资源窗注入面 → 工具栏整个不渲染（九键的家在资源窗）；开关按钮在场且走注入 toggle', async () => {
    useSessionStore().currentId = 's1'
    const toggle = vi.fn()
    const ctl: ResourcesPanelCtl = { open: computed(() => false), toggle }
    const w = mount(ChatPanel, {
      global: { provide: { [RESOURCES_PANEL_CTL as symbol]: ctl } },
    })
    await flushPromises()
    expect(w.find('.voice-toolbar').exists()).toBe(false)
    // toolbar-toggle 同名两类：会话列表开关 + 资源窗开关——按 title 过滤后者。
    const btn = w
      .findAll('button.toolbar-toggle')
      .filter((b) => (b.attributes('title') ?? '').includes('资源窗'))[0]
    expect(btn).toBeTruthy()
    expect(btn.attributes('title')).toBe('打开资源窗')
    await btn.trigger('click')
    expect(toggle).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('语音激活常驻条（voice-strip）：单例任一开关亮 → 条在场点名开关；全关 → 条消失', async () => {
    useSessionStore().currentId = 's1'
    const w = mount(ChatPanel)
    await flushPromises()
    expect(w.find('.voice-strip').exists()).toBe(false)
    voiceDictation.value = true
    voiceRealtime.value = true
    await flushPromises()
    const strip = w.find('.voice-strip')
    expect(strip.exists()).toBe(true)
    expect(strip.text()).toContain('听写')
    expect(strip.text()).toContain('实时语音')
    expect(strip.text()).not.toContain('语音播放')
    expect(strip.text()).not.toContain('语音对话')
    // 无注入面（裸挂载）→ 开关位置指向「上方工具栏」；有注入面指向资源窗。
    expect(strip.text()).toContain('上方工具栏')
    w.unmount()
  })

  it('工具栏实时语音键双引擎闸（2026-11-20）：引擎未就绪禁用 + title 指路；双就绪放行', async () => {
    useSessionStore().currentId = 's1'
    const rt = (w: ReturnType<typeof mount>) =>
      w.findAll('.voice-toolbar button').filter((b) => b.text().includes('实时语音'))[0]
    let w = await mount(ChatPanel)
    await flushPromises()
    expect((rt(w).element as HTMLButtonElement).disabled).toBe(true)
    expect(rt(w).attributes('title')).toContain('双引擎')
    w.unmount()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'engine_status') return Promise.resolve({ stt_ready: true, tts_ready: true })
      return Promise.resolve({})
    })
    w = await mount(ChatPanel)
    await flushPromises()
    expect((rt(w).element as HTMLButtonElement).disabled).toBe(false)
    w.unmount()
  })
})

describe('SessionResourcesPanel · 拉取竞态（2026-11-19）', () => {
  it('WS 未连接时不发请求（门）；连接建立后自动首拉', async () => {
    const wsModule = await import('../../../composables/useWebSocket')
    const wsStatus = wsModule.wsStatus as unknown as { value: string }
    wsStatus.value = 'connecting'
    const w = mountPanel('s1')
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalledWith('session', 'resources', { session_id: 's1' })
    wsStatus.value = 'connected'
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('session', 'resources', { session_id: 's1' })
    expect(w.find('.srp-empty-hint').exists()).toBe(false)
    wsStatus.value = 'connected'
    w.unmount()
  })

  it('空应答 → 温和重试至成功（成功归零）', async () => {
    vi.useFakeTimers()
    let calls = 0
    requestMock.mockImplementation((_m: string, cmd: string) => {
      // 只数 resources 拉取（九键全迁后挂载还并发 voice/inbox/allow 请求，
      // 全量计数会污染重试节奏断言）。
      if (cmd !== 'resources') return Promise.resolve({})
      calls++
      return calls <= 3 ? null : { revision: 1 }
    })
    const w = mountPanel('s1')
    // 首拉（null）→ 400ms 重试 ×3 → 第 4 拉成功。
    await vi.advanceTimersByTimeAsync(1200)
    expect(calls).toBe(4)
    expect(w.find('.srp-empty-hint').exists()).toBe(false)
    w.unmount()
    vi.useRealTimers()
  })

  it('持续空应答 → 上限 5 次重试后停（不空转；诚实错误行不闪烁）', async () => {
    vi.useFakeTimers()
    requestMock.mockResolvedValue(null)
    const w = mountPanel('s1')
    await vi.advanceTimersByTimeAsync(400 * 10)
    const calls = requestMock.mock.calls.filter((c) => c[1] === 'resources').length
    expect(calls).toBe(6) // 首拉 1 + 重试 5
    w.unmount()
    vi.useRealTimers()
  })
})

import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'

/**
 * 语音工具栏状态单例（2026-11-19 资源窗九键全迁）。
 *
 * 模块级单例（useComposerTools / useEditorMode 同款）：ChatPanel 与
 * SessionResourcesPanel 共享同一份状态——STT/TTS 引擎就绪位、四个语音
 * 开关（听写/语音对话/语音播放/实时语音）与静音超时配置。
 *
 * 分工（九键全迁拍板）：
 * - 状态真值只有一份（本模块导出的 ref），两个组件读写同一份；
 * - 开关函数（start/stop 请求 + 配置持久化）也住这里——资源窗语音分节
 *   与 ChatPanel（无 ChatView 注入面的 standalone/workflow 挂载面保留
 *   原工具栏）调用同一套函数；
 * - STT push 帧处理（partial/accumulate/auto_send → 输入框草稿）仍留
 *   ChatPanel——它们写 chatStore.input，与开关状态无耦合；engine_fault
 *   复位也在 ChatPanel 侧直接改这里的共享 ref（同一份，无同步问题）；
 * - 初始化（initVoiceState）幂等：ChatPanel WS ready 时调，资源窗挂载
 *   时也调（已在场 = 重复拉取一次配置，无害）。
 *
 * dialogue 启动需要 session_id（realtime 接力寻址 G6：后端据此注册
 * session_key → TTS 接力/打断只作用于本会话）——由调用方显式传。
 */

// --- 引擎就绪位（engine_status / engine_fault push 维护） ---
export const sttReady = ref(false)
export const ttsReady = ref(false)

// --- 四开关（visual state；重启后 initVoiceState 按「 pipelines 未跑」
//     恢复为关——原 ChatPanel 语义照搬） ---
export const voiceDictation = ref(false)
export const voiceDialogue = ref(false)
export const voicePlayback = ref(false)
// 实时语音主开关（realtime P1 G6）：纯配置态（无 pipeline），恢复显示；
// 后端热态在 gateway 启动时从同一文件装载，两边同源。
export const voiceRealtime = ref(false)

/** 语音对话启动参数：静音超时（voice_config_get 对齐）。 */
export const silenceTimeout = ref(3.0)

export function useVoiceControls() {
  const { request } = useWSAPI()

  /** 配置持久化（read-merge-write：chat_config_set 是整文件替换语义——
   *  只写 3 个键会把 realtime 块静默抹掉。读不到全文就中止：以空 base
   *  写回会把其余键全部丢掉）。 */
  async function saveVoiceConfig() {
    try {
      const cur = await request('voice', 'chat_config_get').catch(() => null)
      if (!cur) return
      const merged = {
        ...((cur as Record<string, unknown>) ?? {}),
        dictation_enabled: voiceDictation.value,
        dialogue_enabled: voiceDialogue.value,
        playback_enabled: voicePlayback.value,
      }
      await request('voice', 'chat_config_set', merged)
    } catch {}
  }

  /** 幂等初始化：配置恢复显示 + 引擎就绪位 + 静音超时（失败 = 按钮禁用）。 */
  async function initVoiceState() {
    try {
      const [config, engines, voiceCfg] = await Promise.all([
        request('voice', 'chat_config_get'),
        request('voice', 'engine_status'),
        request('voice', 'voice_config_get'),
      ])
      if (config) {
        // Visual-only restore: buttons show enabled state but pipelines are
        // NOT started（原 ChatPanel 注释照搬：恢复后立即复位前三开关）。
        voiceDictation.value = config.dictation_enabled ?? false
        voiceDialogue.value = config.dialogue_enabled ?? false
        voicePlayback.value = config.playback_enabled ?? false
        voiceDictation.value = false
        voiceDialogue.value = false
        voicePlayback.value = false
        voiceRealtime.value = (config.realtime as { enabled?: boolean } | undefined)?.enabled ?? false
      }
      if (engines) {
        sttReady.value = engines.stt_ready ?? false
        ttsReady.value = engines.tts_ready ?? false
      }
      if (voiceCfg) {
        silenceTimeout.value = voiceCfg.silence_timeout ?? 3.0
      }
    } catch {
      // Voice not available — keep buttons disabled
    }
  }

  async function toggleDictation() {
    if (voiceDictation.value) {
      await request('voice', 'stt_to_input_stop').catch(() => {})
      voiceDictation.value = false
    } else {
      if (!sttReady.value) return
      // Close dialogue if open
      if (voiceDialogue.value) {
        await request('voice', 'stt_dialogue_stop').catch(() => {})
        voiceDialogue.value = false
      }
      try {
        await request('voice', 'stt_to_input_start')
        voiceDictation.value = true
      } catch {}
    }
    void saveVoiceConfig()
  }

  /** sessionId：realtime 接力寻址（G6）；缺省 = 后端按默认会话处理。 */
  async function toggleDialogue(sessionId?: string) {
    if (voiceDialogue.value) {
      await request('voice', 'stt_dialogue_stop').catch(() => {})
      voiceDialogue.value = false
    } else {
      if (!sttReady.value) return
      // Close dictation if open
      if (voiceDictation.value) {
        await request('voice', 'stt_to_input_stop').catch(() => {})
        voiceDictation.value = false
      }
      try {
        await request('voice', 'stt_dialogue_start', {
          silence_timeout: silenceTimeout.value,
          session_id: sessionId,
        })
        voiceDialogue.value = true
      } catch {}
    }
    void saveVoiceConfig()
  }

  async function togglePlayback() {
    if (voicePlayback.value) {
      await request('voice', 'tts_playback_stop').catch(() => {})
      voicePlayback.value = false
    } else {
      if (!ttsReady.value) return
      voicePlayback.value = true
    }
    void saveVoiceConfig()
  }

  /** 实时语音：read-merge-write realtime 块，后端写盘成功即 apply 热态；
   *  读不到全文/失败回滚本地态（原 ChatPanel 语义照搬）。 */
  async function toggleRealtime() {
    voiceRealtime.value = !voiceRealtime.value
    try {
      const cur = await request('voice', 'chat_config_get').catch(() => null)
      if (!cur) {
        voiceRealtime.value = !voiceRealtime.value
        return
      }
      const base = (cur as Record<string, unknown>) as Record<string, unknown>
      await request('voice', 'chat_config_set', {
        ...base,
        realtime: {
          ...((base.realtime as Record<string, unknown>) ?? {}),
          enabled: voiceRealtime.value,
        },
      })
    } catch {
      voiceRealtime.value = !voiceRealtime.value
    }
  }

  return {
    sttReady,
    ttsReady,
    voiceDictation,
    voiceDialogue,
    voicePlayback,
    voiceRealtime,
    silenceTimeout,
    initVoiceState,
    toggleDictation,
    toggleDialogue,
    togglePlayback,
    toggleRealtime,
  }
}

/** 测试辅助：重置模块级单例（生产代码勿用；useComposerTools 同款惯例）。 */
export function _resetVoiceControlsForTest() {
  sttReady.value = false
  ttsReady.value = false
  voiceDictation.value = false
  voiceDialogue.value = false
  voicePlayback.value = false
  voiceRealtime.value = false
  silenceTimeout.value = 3.0
}

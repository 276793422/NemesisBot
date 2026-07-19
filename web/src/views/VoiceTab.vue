<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { addMessageHandler, removeMessageHandler } from '../composables/useWebSocket'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// --- State ---
const loading = ref(true)
const status = ref<any>(null)
const showConfig = ref(false)
const configContent = ref('')
const configExists = ref(false)
const setupProgress = ref('')

// Voice config
const selectedSpeaker = ref(45)
const volume = ref(50)
const speed = ref(1.0)
const captureDevice = ref('')
const playbackDevice = ref('')
const sttEnabled = ref(false)
const ttsEnabled = ref(false)
const punctEnabled = ref(false)
const aecEnabled = ref(false)
// AEC 进阶参数：房间预设映射到 filter_length（16kHz 下的采样数）。改后重启 STT 生效。
const aecAdvancedOpen = ref(false)
const aecFilterLength = ref(8192) // 与后端 DEFAULT_FILTER_LENGTH 对齐
const aecPreprocess = ref(true)

// 房间类型预设：filter_length（采样数）≈ 回声尾 ms × 16
const AEC_ROOM_PRESETS = [
  { key: 'small', label: '小房间', filterLength: 4096, ms: 256 },
  { key: 'normal', label: '普通房间', filterLength: 8192, ms: 512 },
  { key: 'large', label: '空旷大房', filterLength: 16384, ms: 1024 },
] as const
// 当前值命中的预设（没命中说明值被手动改过，按钮组不高亮）
const aecCurrentRoomKey = computed(() => {
  const found = AEC_ROOM_PRESETS.find((p) => p.filterLength === aecFilterLength.value)
  return found ? found.key : ''
})
const aecCurrentRoomMs = computed(() => Math.round(aecFilterLength.value / 16))
function selectAecRoom(filterLength: number) {
  aecFilterLength.value = filterLength
}

// TTS test
const ttsText = ref('')
const ttsPlaying = ref(false)

// STT test
const sttRunning = ref(false)
const sttResults = ref<string[]>([])

// Devices
const inputDevices = ref<any[]>([])
const outputDevices = ref<any[]>([])

// Speakers
const speakers = ref<any[]>([])

// Speaker verification (voiceprint)
const speakerEngineEnabled = ref(false)
const speakerThreshold = ref(0.65)
const silenceTimeout = ref(3.0)
const voiceDialogueActive = ref(false)
const speakerRegistered = ref<string[]>([])
const speakerRegistering = ref(false)
const speakerRegisterName = ref('owner')
const speakerRegisterElapsed = ref(0)
let _registerTimer: ReturnType<typeof setInterval> | null = null
const speakerTesting = ref(false)
const speakerTestResults = ref<{ text: string; similarity: number; matched: boolean }[]>([])

const VOICEPRINT_TEXT = `御街行  土世界
重重稼穑连天外，无金出，无水溉。点点阴火透幽玄，残木唯存灰蔼。借此幽火，抚耀残木，再待参天开。
命如朽壤天上来，稼穑破，多滞碍。幸得玄术镇灵台，师言机缘犹在。地利不及，天时不至，静待亦何奈。`

// SSE handler ref for cleanup
let _onSetupProgress: ((data: any) => void) | null = null
let _wsHandler: ((data: any) => void) | null = null

function formatSize(bytes: number | null | undefined): string {
  if (!bytes) return ''
  if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  if (bytes >= 1024) return (bytes / 1024).toFixed(0) + ' KB'
  return bytes + ' B'
}

// --- Data loading ---

async function loadStatus() {
  try {
    status.value = await request('voice', 'status')
  } catch (e: any) {
    toast.error('环境检测失败: ' + e)
  }
}

async function loadDevices() {
  try {
    const data = await request('voice', 'devices')
    inputDevices.value = data?.input || []
    outputDevices.value = data?.output || []
  } catch (e: any) {
    // Non-critical, ignore
  }
}

async function loadSpeakers() {
  try {
    const data = await request('voice', 'speakers')
    speakers.value = data?.speakers || []
  } catch (e: any) {
    // Non-critical
  }
}

async function loadConfig() {
  try {
    const data = await request('voice', 'config_get')
    configExists.value = data?.exists || false
    configContent.value = data?.content || ''
  } catch (e: any) {
    toast.error('加载配置失败: ' + e)
  }
}

async function loadAll() {
  loading.value = true
  await Promise.all([loadStatus(), loadDevices(), loadSpeakers(), loadConfig()])
  loading.value = false
}

// --- Actions ---

async function checkEnv() {
  try {
    status.value = await request('voice', 'check')
    toast.success('环境检查完成')
  } catch (e: any) {
    toast.error('检查失败: ' + e)
  }
}

async function oneClickSetup() {
  setupProgress.value = '正在安装...'
  try {
    await request('voice', 'setup', undefined, 0)
    toast.success('一键安装完成')
    setupProgress.value = ''
    await loadStatus()
  } catch (e: any) {
    toast.error('安装失败: ' + e)
    setupProgress.value = ''
  }
}

async function stopSetup() {
  try {
    await request('voice', 'stop_setup')
    setupProgress.value = ''
    toast.success('已停止安装')
  } catch (e: any) {
    toast.error('停止失败: ' + e)
  }
}

async function installRuntime() {
  setupProgress.value = '正在安装运行库...'
  try {
    await request('voice', 'install_runtime', undefined, 0)
    toast.success('运行库安装完成')
    setupProgress.value = ''
    await loadStatus()
  } catch (e: any) {
    toast.error('运行库安装失败: ' + e)
    setupProgress.value = ''
  }
}

async function installModel(model: string, label: string) {
  setupProgress.value = `正在安装${label}模型...`
  try {
    await request('voice', 'install_model', { model }, 0)
    toast.success(`${label}模型安装完成`)
    setupProgress.value = ''
    await loadStatus()
  } catch (e: any) {
    toast.error(`${label}模型安装失败: ` + e)
    setupProgress.value = ''
  }
}

async function installAec() {
  setupProgress.value = '正在安装回声消除库...'
  try {
    await request('voice', 'install_aec', undefined, 0)
    toast.success('回声消除库安装完成')
    setupProgress.value = ''
    await loadStatus()
    // 安装即默认开启（触发 watch 自动保存）
    if (aecReady.value && !aecEnabled.value) {
      aecEnabled.value = true
    }
  } catch (e: any) {
    toast.error('回声消除库安装失败: ' + e)
    setupProgress.value = ''
  }
}

async function saveConfig() {
  try {
    await request('voice', 'config_set', { content: configContent.value })
    toast.success('配置已保存')
    await loadStatus()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function toggleConfig() {
  showConfig.value = !showConfig.value
  if (showConfig.value) await loadConfig()
}

async function playTTS() {
  if (!ttsText.value.trim()) {
    toast.error('请输入要转换的文字')
    return
  }
  ttsPlaying.value = true
  try {
    await request('voice', 'tts', {
      text: ttsText.value,
      speaker: selectedSpeaker.value,
      speed: speed.value,
      volume: volume.value,
    })
  } catch (e: any) {
    toast.error('TTS 失败: ' + e)
  }
  ttsPlaying.value = false
}

async function startSTT() {
  sttResults.value = []
  try {
    await request('voice', 'stt_start')
    sttRunning.value = true
  } catch (e: any) {
    toast.error('启动听写失败: ' + e)
  }
}

async function stopSTT() {
  try {
    await request('voice', 'stt_stop')
    sttRunning.value = false
  } catch (e: any) {
    toast.error('停止听写失败: ' + e)
  }
}

// --- Computed ---

const dllStatus = computed(() => {
  if (!status.value?.dlls) return []
  return status.value.dlls
})

const runtimeReady = computed(() => status.value?.all_dlls_present && status.value?.config_exists)

const models = computed(() => status.value?.models || {})
const aecReady = computed(() => !!status.value?.aec?.ready)

const currentSpeakerName = computed(() => {
  const s = speakers.value.find((sp: any) => sp.speaker_id === selectedSpeaker.value)
  return s ? `${s.id} (${s.gender})` : `Speaker ${selectedSpeaker.value}`
})

// --- SSE for setup progress ---
_onSetupProgress = (data: any) => {
  if (data?.message) setupProgress.value = data.message
  if (data?.status === 'complete' || data?.status === 'error') {
    setTimeout(() => { setupProgress.value = '' }, 2000)
  }
}
sseOn('voice-setup', _onSetupProgress)

// --- WebSocket push handler for STT results & speaker test ---
_wsHandler = (data: any) => {
  if (data?.type === 'push' && data?.module === 'voice') {
    if (data?.cmd === 'stt_result') {
      const text = data?.data?.text || ''
      if (text) sttResults.value.push(text)
    } else if (data?.cmd === 'speaker_test_result') {
      const r = data?.data
      if (r?.text) {
        speakerTestResults.value.push({
          text: r.text,
          similarity: r.similarity ?? 0,
          matched: r.matched ?? false,
        })
      }
    }
  }
}
addMessageHandler(_wsHandler)

// --- Voice config persistence ---

async function loadVoiceConfig() {
  try {
    const data = await request('voice', 'voice_config_get')
    if (data) {
      if (data.speaker_id != null) selectedSpeaker.value = data.speaker_id
      if (data.volume != null) volume.value = data.volume
      if (data.speed != null) speed.value = data.speed
      if (data.capture_device != null) captureDevice.value = data.capture_device
      if (data.playback_device != null) playbackDevice.value = data.playback_device
      if (data.stt_enabled != null) sttEnabled.value = data.stt_enabled
      if (data.tts_enabled != null) ttsEnabled.value = data.tts_enabled
      if (data.punct_enabled != null) punctEnabled.value = data.punct_enabled
      if (data.aec_enabled != null) aecEnabled.value = data.aec_enabled
      if (data.aec_filter_length != null) aecFilterLength.value = data.aec_filter_length
      if (data.aec_preprocess != null) aecPreprocess.value = data.aec_preprocess
      if (data.speaker_enabled != null) speakerEngineEnabled.value = data.speaker_enabled
      if (data.silence_timeout != null) silenceTimeout.value = data.silence_timeout
    }
  } catch (_e) {
    // Use defaults
  }
}

let _saveTimer: ReturnType<typeof setTimeout> | null = null

function saveVoiceConfigDebounced() {
  if (_saveTimer) clearTimeout(_saveTimer)
  _saveTimer = setTimeout(async () => {
    try {
      await request('voice', 'voice_config_set', {
        speaker_id: selectedSpeaker.value,
        volume: volume.value,
        speed: speed.value,
        capture_device: captureDevice.value,
        playback_device: playbackDevice.value,
        stt_enabled: sttEnabled.value,
        tts_enabled: ttsEnabled.value,
        punct_enabled: punctEnabled.value,
        aec_enabled: aecEnabled.value,
        aec_filter_length: aecFilterLength.value,
        aec_preprocess: aecPreprocess.value,
        speaker_enabled: speakerEngineEnabled.value,
        silence_timeout: silenceTimeout.value,
      })
    } catch (_e) {
      // Silent fail
    }
  }, 500)
}

// Engine state initialization flag — prevent engine commands during initial load
const _engineInitialized = ref(false)
const _skipEngineWatch = ref(false)

// Watch all config values and auto-save
watch([selectedSpeaker, volume, speed, captureDevice, playbackDevice, sttEnabled, ttsEnabled, punctEnabled, aecEnabled, aecFilterLength, aecPreprocess, speakerEngineEnabled, silenceTimeout], () => {
  saveVoiceConfigDebounced()
})

// Engine start/stop on STT toggle change
watch(sttEnabled, async (enabled) => {
  if (!_engineInitialized.value || _skipEngineWatch.value) return
  try {
    if (enabled) {
      await request('voice', 'engine_start', { model: 'stt' })
    } else {
      await request('voice', 'engine_stop', { model: 'stt' })
    }
  } catch (e: any) {
    toast.error(enabled ? `STT引擎启动失败: ${e}` : `STT引擎停止失败: ${e}`)
    _skipEngineWatch.value = true
    sttEnabled.value = !enabled
    _skipEngineWatch.value = false
  }
})

// Engine start/stop on TTS toggle change
watch(ttsEnabled, async (enabled) => {
  if (!_engineInitialized.value || _skipEngineWatch.value) return
  try {
    if (enabled) {
      await request('voice', 'engine_start', { model: 'tts' })
    } else {
      await request('voice', 'engine_stop', { model: 'tts' })
    }
  } catch (e: any) {
    toast.error(enabled ? `TTS引擎启动失败: ${e}` : `TTS引擎停止失败: ${e}`)
    _skipEngineWatch.value = true
    ttsEnabled.value = !enabled
    _skipEngineWatch.value = false
  }
})

// Speaker engine start/stop on toggle change
watch(speakerEngineEnabled, async (enabled) => {
  if (!_engineInitialized.value || _skipEngineWatch.value) return
  try {
    if (enabled) {
      await request('voice', 'engine_start', { model: 'speaker' })
      await loadSpeakerStatus()
    } else {
      await request('voice', 'engine_stop', { model: 'speaker' })
    }
  } catch (e: any) {
    toast.error(enabled ? `声纹引擎启动失败: ${e}` : `声纹引擎停止失败: ${e}`)
    _skipEngineWatch.value = true
    speakerEngineEnabled.value = !enabled
    _skipEngineWatch.value = false
  }
})

// Speaker verification functions
async function loadSpeakerStatus() {
  try {
    const data = await request('voice', 'speaker_status')
    if (data) {
      speakerThreshold.value = data.threshold ?? 0.65
      speakerRegistered.value = data.speakers ?? []
      voiceDialogueActive.value = data.stt_dialogue_active ?? false
    }
  } catch {}
}

async function startSpeakerRegister() {
  try {
    speakerRegistering.value = true
    speakerRegisterElapsed.value = 0
    await request('voice', 'speaker_register_start', { name: speakerRegisterName.value })
    _registerTimer = setInterval(() => {
      speakerRegisterElapsed.value += 0.1
      if (speakerRegisterElapsed.value >= 30) {
        stopSpeakerRegister()
      }
    }, 100)
  } catch (e: any) {
    speakerRegistering.value = false
    toast.error(`开始录制失败: ${e}`)
  }
}

async function stopSpeakerRegister() {
  if (_registerTimer) {
    clearInterval(_registerTimer)
    _registerTimer = null
  }
  try {
    const data = await request('voice', 'speaker_register_stop')
    if (data?.registered) {
      toast.success(`声纹注册成功: ${data.name} (${data.duration.toFixed(1)}秒)`)
      await loadSpeakerStatus()
    }
  } catch (e: any) {
    toast.error(`注册失败: ${e}`)
  } finally {
    speakerRegistering.value = false
    speakerRegisterElapsed.value = 0
  }
}

function cancelSpeakerRegister() {
  if (_registerTimer) {
    clearInterval(_registerTimer)
    _registerTimer = null
  }
  request('voice', 'speaker_register_cancel').catch(() => {})
  speakerRegistering.value = false
  speakerRegisterElapsed.value = 0
}

async function removeSpeakerVoiceprint(name: string) {
  try {
    await request('voice', 'speaker_remove', { name })
    toast.success(`已删除声纹: ${name}`)
    await loadSpeakerStatus()
  } catch (e: any) {
    toast.error(`删除失败: ${e}`)
  }
}

async function startSpeakerTest() {
  speakerTestResults.value = []
  speakerTesting.value = true
  try {
    await request('voice', 'speaker_test_start', undefined, 0)
  } catch (e: any) {
    toast.error(`测试失败: ${e}`)
    speakerTesting.value = false
  }
}

async function stopSpeakerTest() {
  try {
    await request('voice', 'speaker_test_stop')
  } catch (e: any) {
    toast.error(`停止失败: ${e}`)
  }
  speakerTesting.value = false
}

async function updateSpeakerThreshold(value: number) {
  speakerThreshold.value = value
  try {
    await request('voice', 'speaker_set_threshold', { threshold: value })
  } catch {}
}

onMounted(async () => {
  await loadAll()
  await loadVoiceConfig()
  await loadSpeakerStatus()
  _engineInitialized.value = true
})

onUnmounted(() => {
  if (_onSetupProgress) sseOff('voice-setup', _onSetupProgress)
  if (_wsHandler) removeMessageHandler(_wsHandler)
  if (_saveTimer) clearTimeout(_saveTimer)
})
</script>

<template>
  <div v-if="loading" style="text-align: center; padding: var(--space-8);">
    <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
  </div>

  <div v-if="!loading" style="display: flex; flex-direction: column; gap: var(--space-4);">

    <!-- Setup progress bar -->
    <div v-if="setupProgress" class="card" style="padding: var(--space-3) var(--space-4); background: var(--accent-bg, rgba(59,130,246,0.08)); border-color: var(--accent);">
      <div style="display: flex; align-items: center; gap: var(--space-3);">
        <div class="spinner spinner-sm"></div>
        <span style="font-size: var(--text-sm); color: var(--accent);">{{ setupProgress }}</span>
      </div>
    </div>

    <!-- Row 1: Environment + Configuration -->
    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4);">

    <!-- Section 1: Environment Management -->
    <div class="card">
      <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
        <h3 style="margin: 0;">环境管理</h3>
        <div style="display: flex; gap: var(--space-2);">
          <button class="btn btn-sm" @click="toggleConfig">{{ showConfig ? '隐藏配置' : '查看配置' }}</button>
          <button class="btn btn-sm" @click="checkEnv">检查环境</button>
          <button class="btn btn-sm btn-primary" @click="oneClickSetup" :disabled="!!setupProgress">一键安装</button>
          <button v-if="setupProgress" class="btn btn-sm btn-danger" @click="stopSetup">停止安装</button>
        </div>
      </div>
      <div class="card-body">
        <!-- Runtime DLLs -->
        <div style="margin-bottom: var(--space-3);">
          <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
            <span style="font-weight: 500;">运行库</span>
            <button class="btn btn-sm" @click="installRuntime" :disabled="!!setupProgress || runtimeReady">安装</button>
          </div>
          <div style="padding-left: var(--space-4);">
            <div v-for="dll in dllStatus" :key="dll.name" style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0; font-size: var(--text-sm);">
              <span :style="{ color: dll.exists ? 'var(--success)' : 'var(--text-secondary)' }">{{ dll.exists ? '●' : '○' }}</span>
              <span>{{ dll.name }}</span>
              <span v-if="dll.size_bytes" style="color: var(--text-secondary);">({{ formatSize(dll.size_bytes) }})</span>
            </div>
          </div>
        </div>

        <!-- Models -->
        <div>
          <div style="font-weight: 500; margin-bottom: var(--space-2);">模型</div>
          <div style="display: flex; flex-direction: column; gap: var(--space-2); padding-left: var(--space-4);">
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: models.stt?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ models.stt?.ready ? '●' : '○' }}</span>
                <span>STT 模型</span>
              </span>
              <button class="btn btn-sm" @click="installModel('stt', 'STT')" :disabled="!!setupProgress || models.stt?.ready">安装</button>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: models.vad?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ models.vad?.ready ? '●' : '○' }}</span>
                <span>VAD 模型</span>
              </span>
              <button class="btn btn-sm" @click="installModel('vad', 'VAD')" :disabled="!!setupProgress || models.vad?.ready">安装</button>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: models.tts?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ models.tts?.ready ? '●' : '○' }}</span>
                <span>TTS 模型</span>
              </span>
              <button class="btn btn-sm" @click="installModel('tts', 'TTS')" :disabled="!!setupProgress || models.tts?.ready">安装</button>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: models.punct?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ models.punct?.ready ? '●' : '○' }}</span>
                <span>标点模型</span>
              </span>
              <button class="btn btn-sm" @click="installModel('punct', '标点')" :disabled="!!setupProgress || models.punct?.ready">安装</button>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: models.speaker?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ models.speaker?.ready ? '●' : '○' }}</span>
                <span>声纹模型</span>
              </span>
              <button class="btn btn-sm" @click="installModel('speaker', '声纹')" :disabled="!!setupProgress || models.speaker?.ready">安装</button>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);">
                <span :style="{ color: aecReady ? 'var(--success)' : 'var(--text-secondary)' }">{{ aecReady ? '●' : '○' }}</span>
                <span>回声消除</span>
              </span>
              <button class="btn btn-sm" @click="installAec" :disabled="!!setupProgress || aecReady">安装</button>
            </div>
          </div>
        </div>

        <!-- Config editor (toggle) -->
        <div v-if="showConfig" class="config-editor-wrap">
          <div class="config-editor-header">
            <span class="config-editor-title">原始配置（进阶）</span>
            <button class="btn btn-sm btn-primary" @click="saveConfig" :disabled="!configExists">保存</button>
          </div>
          <p class="config-editor-hint">日常用下方开关即可。仅在排查问题时直接改配置文本。</p>
          <textarea class="form-textarea config-editor" v-model="configContent" :disabled="!configExists" placeholder="配置文件为空，点击保存可创建"></textarea>
        </div>
      </div>
    </div>

    <!-- Section 2: Voice Configuration -->
    <div class="card">
      <div class="card-header"><h3 style="margin: 0;">语音配置</h3></div>
      <div class="card-body">
        <div class="voice-config-grid">
          <!-- Speaker -->
          <div class="voice-field">
            <span class="voice-label">输出音色</span>
            <select class="form-select voice-select" v-model.number="selectedSpeaker">
              <option v-for="sp in speakers" :key="sp.speaker_id" :value="sp.speaker_id">
                {{ sp.id }} ({{ sp.gender }})
              </option>
            </select>
          </div>

          <!-- Volume -->
          <div class="voice-field">
            <span class="voice-label">合成音量</span>
            <div class="slider-row">
              <input type="range" class="nice-slider" min="1" max="100" v-model.number="volume" />
              <span class="slider-value">{{ volume }}</span>
            </div>
          </div>

          <!-- Speed -->
          <div class="voice-field">
            <span class="voice-label">语速</span>
            <div class="slider-row">
              <input type="range" class="nice-slider" min="0.5" max="2.0" step="0.1" v-model.number="speed" />
              <span class="slider-value">{{ speed.toFixed(1) }}x</span>
            </div>
          </div>

          <!-- Input device -->
          <div class="voice-field">
            <span class="voice-label">输入设备</span>
            <select class="form-select voice-select" v-model="captureDevice">
              <option value="">默认麦克风</option>
              <option v-for="dev in inputDevices" :key="dev.index" :value="dev.name">
                {{ dev.name }}{{ dev.is_default ? ' (默认)' : '' }}
              </option>
            </select>
          </div>

          <!-- Output device -->
          <div class="voice-field">
            <span class="voice-label">输出设备</span>
            <select class="form-select voice-select" v-model="playbackDevice">
              <option value="">默认扬声器</option>
              <option v-for="dev in outputDevices" :key="dev.index" :value="dev.name">
                {{ dev.name }}{{ dev.is_default ? ' (默认)' : '' }}
              </option>
            </select>
          </div>

          <!-- Auto-send timeout -->
          <div class="voice-field">
            <span class="voice-label">自动发送</span>
            <div class="slider-row">
              <input type="range" class="nice-slider" min="1" max="30" step="0.5" v-model.number="silenceTimeout" :disabled="voiceDialogueActive" />
              <span class="slider-value">{{ silenceTimeout.toFixed(1) }}秒</span>
            </div>
          </div>

          <!-- Speaker engine toggle -->
          <div class="voice-field toggle-field">
            <span class="voice-label">声纹模型</span>
            <div class="nice-toggle" :class="{ active: speakerEngineEnabled, disabled: false }" @click="speakerEngineEnabled = !speakerEngineEnabled" role="switch" :aria-checked="speakerEngineEnabled">
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
              <span class="toggle-text">{{ speakerEngineEnabled ? '启用' : '停用' }}</span>
            </div>
          </div>

          <!-- TTS toggle -->
          <div class="voice-field toggle-field">
            <span class="voice-label">TTS 模型</span>
            <div class="nice-toggle" :class="{ active: ttsEnabled }" @click="ttsEnabled = !ttsEnabled" role="switch" :aria-checked="ttsEnabled">
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
              <span class="toggle-text">{{ ttsEnabled ? '启用' : '停用' }}</span>
            </div>
          </div>

          <!-- STT toggle -->
          <div class="voice-field toggle-field">
            <span class="voice-label">STT 模型</span>
            <div class="nice-toggle" :class="{ active: sttEnabled }" @click="sttEnabled = !sttEnabled" role="switch" :aria-checked="sttEnabled">
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
              <span class="toggle-text">{{ sttEnabled ? '启用' : '停用' }}</span>
            </div>
          </div>

          <!-- Punct toggle -->
          <div class="voice-field toggle-field">
            <span class="voice-label">标点模型</span>
            <div class="nice-toggle" :class="{ active: punctEnabled, disabled: sttEnabled }" @click="!sttEnabled && (punctEnabled = !punctEnabled)" role="switch" :aria-checked="punctEnabled">
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
              <span class="toggle-text">{{ punctEnabled ? '启用' : '停用' }}</span>
            </div>
          </div>

          <!-- AEC toggle -->
          <div class="voice-field toggle-field">
            <span class="voice-label">回声消除</span>
            <div class="nice-toggle" :class="{ active: aecEnabled, disabled: !aecReady || sttEnabled }" @click="aecReady && !sttEnabled && (aecEnabled = !aecEnabled)" role="switch" :aria-checked="aecEnabled">
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
              <span class="toggle-text">{{ !aecReady ? '未安装' : (aecEnabled ? '启用' : '停用') }}</span>
            </div>
          </div>

          <!-- AEC 进阶参数 -->
          <div v-if="aecReady && aecEnabled" class="aec-advanced">
            <div class="aec-advanced-header" @click="aecAdvancedOpen = !aecAdvancedOpen">
              <span class="aec-caret">{{ aecAdvancedOpen ? '▾' : '▸' }}</span>
              <span>AEC 进阶</span>
            </div>
            <div v-if="aecAdvancedOpen" class="aec-advanced-body">
              <div class="aec-row">
                <span class="aec-row-key">房间类型</span>
                <div class="aec-room-group">
                  <button
                    v-for="p in AEC_ROOM_PRESETS"
                    :key="p.key"
                    type="button"
                    class="aec-room-btn"
                    :class="{ active: aecCurrentRoomKey === p.key }"
                    @click="selectAecRoom(p.filterLength)"
                  >{{ p.label }}</button>
                </div>
                <span class="aec-hint">当前回声尾 {{ aecCurrentRoomMs }} ms</span>
              </div>
              <div class="aec-row">
                <span class="aec-row-key">降噪预处理</span>
                <div class="nice-toggle" :class="{ active: aecPreprocess }" @click="aecPreprocess = !aecPreprocess" role="switch" :aria-checked="aecPreprocess">
                  <span class="toggle-track"><span class="toggle-thumb"></span></span>
                  <span class="toggle-text">{{ aecPreprocess ? '启用' : '停用' }}</span>
                </div>
              </div>
              <div class="aec-restart-hint">⚠ 修改后重启语音对话生效</div>
            </div>
          </div>
        </div>
      </div>
    </div>

    </div><!-- End Row 1 -->

    <!-- Row 2: Speaker + Voice Test -->
    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4);">

    <!-- Section 3: Speaker Verification (Voiceprint) -->
    <div class="card">
      <div class="card-header"><h3 style="margin: 0;">声纹识别</h3></div>
      <div class="card-body">
        <!-- Threshold -->
        <div class="threshold-row">
          <span class="threshold-label">匹配阈值</span>
          <input type="range" class="nice-slider" min="0.3" max="0.95" step="0.01" v-model.number="speakerThreshold" @change="updateSpeakerThreshold(speakerThreshold)" :disabled="!speakerEngineEnabled" />
          <span class="slider-value">{{ speakerThreshold.toFixed(2) }}</span>
        </div>

        <!-- Registered voiceprints -->
        <div style="margin-bottom: var(--space-3);">
          <span style="font-weight: 500; font-size: var(--text-sm);">已注册声纹:</span>
          <div v-if="speakerRegistered.length > 0" style="margin-top: var(--space-1); padding-left: var(--space-4);">
            <div v-for="name in speakerRegistered" :key="name" style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0;">
              <span style="color: var(--success);">●</span>
              <span style="font-size: var(--text-sm);">{{ name }}</span>
              <button class="btn btn-sm btn-danger" @click="removeSpeakerVoiceprint(name)" style="margin-left: auto; font-size: 11px; padding: 1px 6px;" :disabled="speakerEngineEnabled">删除</button>
            </div>
          </div>
          <div v-else style="color: var(--text-secondary); font-size: var(--text-sm); padding-left: var(--space-4);">
            未注册
          </div>
        </div>

        <!-- Register new voiceprint -->
        <div style="margin-bottom: var(--space-4);">
          <div style="font-weight: 500; margin-bottom: var(--space-3);">录制新声纹</div>
          <div style="font-size: var(--text-sm); color: var(--text-secondary); margin-bottom: var(--space-3); line-height: 1.6;">你有 30 秒的时间，读完如下内容，然后系统会根据你的声音生成新的声纹。系统只给你 30 秒时间，若你读不完，则系统不会等你。</div>
          <div style="border-left: 3px solid var(--border); padding: var(--space-3) var(--space-4); margin-bottom: var(--space-3); font-size: var(--text-sm); line-height: 1.8; white-space: pre-line; color: var(--text-secondary);">{{ VOICEPRINT_TEXT }}</div>
          <div v-if="!speakerRegistering" style="margin-top: var(--space-2);">
            <button class="btn btn-primary" @click="startSpeakerRegister" :disabled="speakerEngineEnabled || sttEnabled">开始录制</button>
          </div>
          <div v-else style="margin-top: var(--space-2);">
            <div style="display: flex; align-items: center; gap: var(--space-3); margin-bottom: var(--space-2);">
              <progress :value="speakerRegisterElapsed" max="30" style="flex: 1; height: 8px;"></progress>
              <span style="font-size: var(--text-sm); color: var(--accent); min-width: 80px;">{{ speakerRegisterElapsed.toFixed(1) }}s / 30.0s</span>
            </div>
            <div style="display: flex; gap: var(--space-2);">
              <button class="btn btn-danger" @click="stopSpeakerRegister">停止录制</button>
              <button class="btn" @click="cancelSpeakerRegister">取消</button>
            </div>
          </div>
        </div>

        <!-- Test (placeholder — will be redesigned in C) -->
        <div>
          <div style="font-weight: 500; margin-bottom: var(--space-2);">声纹测试</div>
          <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
            <button v-if="!speakerTesting" class="btn btn-primary" @click="startSpeakerTest" :disabled="speakerEngineEnabled || sttEnabled">开始测试</button>
            <button v-if="speakerTesting" class="btn btn-danger" @click="stopSpeakerTest">停止测试</button>
          </div>
          <div v-if="speakerTestResults.length > 0" style="border: 1px solid var(--border); border-radius: var(--radius-md); padding: var(--space-3); max-height: 250px; overflow-y: auto;">
            <div v-for="(r, i) in speakerTestResults" :key="i" style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0; font-size: var(--text-sm);">
              <span :style="{ color: r.matched ? 'var(--success)' : 'var(--danger)', fontWeight: 500 }">{{ r.matched ? '✓' : '✗' }}</span>
              <span style="flex: 1;">{{ r.text }}</span>
              <span style="color: var(--text-secondary); min-width: 100px; text-align: right;">相似度: {{ r.similarity.toFixed(2) }}</span>
            </div>
          </div>
          <div v-else-if="speakerTesting" style="color: var(--text-secondary); font-size: var(--text-sm);">
            请说话...
          </div>
        </div>
      </div>
    </div>

    <!-- Section 4: Voice Test -->
    <div class="card">
      <div class="card-header"><h3 style="margin: 0;">语音测试</h3></div>
      <div class="card-body">
        <!-- TTS test -->
        <div style="margin-bottom: var(--space-4);">
          <div style="font-weight: 500; margin-bottom: var(--space-2);">TTS 合成</div>
          <div style="display: flex; gap: var(--space-2);">
            <textarea class="form-textarea" style="flex: 1; min-height: 80px; resize: vertical;" v-model="ttsText" placeholder="输入要转换的文字..." @keydown.ctrl.enter="playTTS"></textarea>
            <button class="btn btn-primary" @click="playTTS" :disabled="ttsPlaying || !ttsText.trim() || ttsEnabled">
              {{ ttsPlaying ? '播放中...' : '播放' }}
            </button>
          </div>
        </div>

        <!-- STT test -->
        <div>
          <div style="font-weight: 500; margin-bottom: var(--space-2);">STT 听写</div>
          <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
            <button v-if="!sttRunning" class="btn btn-primary" @click="startSTT" :disabled="sttEnabled">开始听写</button>
            <button v-if="sttRunning" class="btn btn-danger" @click="stopSTT">停止听写</button>
          </div>
          <div v-if="sttResults.length > 0" style="background: var(--bg-secondary); border: 1px solid var(--border); border-radius: var(--radius-md); padding: var(--space-3); max-height: 200px; overflow-y: auto;">
            <div v-for="(line, i) in sttResults" :key="i" style="font-size: var(--text-sm); padding: var(--space-1) 0;">
              {{ line }}
            </div>
          </div>
          <div v-else-if="sttRunning" style="color: var(--text-secondary); font-size: var(--text-sm);">
            请说话...
          </div>
        </div>
      </div>
    </div>

    </div><!-- End Row 2 -->

  </div>
</template>

<style scoped>
/* ===== Voice Config Grid ===== */
.voice-config-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-4) var(--space-5);
  align-items: center;
}

.voice-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.voice-field.toggle-field {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-2) 0;
}

.voice-label {
  font-size: var(--text-sm);
  color: var(--text);
  font-weight: 600;
}

.voice-select {
  width: 100%;
}

.toggle-field .voice-label {
  flex: 1;
}

/* ===== Config Editor ===== */
.config-editor-wrap {
  margin-top: var(--space-4);
  border-top: 1px solid var(--border);
  padding-top: var(--space-4);
}

.config-editor-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}

.config-editor-title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
}

.config-editor-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-2);
}

.config-editor {
  min-height: 300px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}

/* ===== Nice Slider ===== */
.slider-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.nice-slider {
  -webkit-appearance: none;
  appearance: none;
  flex: 1;
  height: 6px;
  background: var(--border);
  border-radius: var(--radius-full);
  outline: none;
  cursor: pointer;
}

.nice-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  box-shadow: var(--shadow-sm);
  transition: transform var(--duration-fast), box-shadow var(--duration-fast);
}

.nice-slider::-webkit-slider-thumb:hover {
  transform: scale(1.15);
  box-shadow: 0 0 0 4px var(--accent-muted);
}

.nice-slider::-moz-range-thumb {
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  border: none;
  box-shadow: var(--shadow-sm);
}

.nice-slider:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.slider-value {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--accent);
  background: var(--accent-muted);
  padding: 2px 10px;
  border-radius: var(--radius-sm);
  min-width: 50px;
  text-align: center;
}

/* ===== Nice Toggle ===== */
.nice-toggle {
  display: inline-flex;
  align-items: center;
  gap: var(--space-3);
  cursor: pointer;
  user-select: none;
  transition: opacity var(--duration-fast);
}

.nice-toggle.disabled {
  opacity: 0.4;
  cursor: not-allowed;
  pointer-events: none;
}

.nice-toggle .toggle-track {
  position: relative;
  width: 44px;
  height: 24px;
  background: var(--border);
  border-radius: var(--radius-full);
  transition: background var(--duration-fast);
  flex-shrink: 0;
}

.nice-toggle.active .toggle-track {
  background: var(--accent);
}

.nice-toggle .toggle-thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 20px;
  height: 20px;
  background: white;
  border-radius: 50%;
  transition: transform var(--duration-fast);
  box-shadow: var(--shadow-xs);
}

.nice-toggle.active .toggle-thumb {
  transform: translateX(20px);
}

.nice-toggle .toggle-text {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
}

/* ===== Threshold Row ===== */
.threshold-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.threshold-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
  min-width: 70px;
}

/* ===== AEC Advanced ===== */
.aec-advanced {
  grid-column: 1 / -1;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  overflow: hidden;
  margin-top: var(--space-2);
}

.aec-advanced-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  background: var(--bg-secondary);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  cursor: pointer;
  user-select: none;
  transition: color var(--duration-fast);
}

.aec-advanced-header:hover {
  color: var(--text);
}

.aec-caret {
  display: inline-block;
  width: 12px;
}

.aec-advanced-body {
  padding: var(--space-4);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.aec-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-wrap: wrap;
}

.aec-row-key {
  min-width: 90px;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 600;
}

.aec-room-group {
  display: inline-flex;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  overflow: hidden;
}

.aec-room-btn {
  padding: var(--space-2) var(--space-3);
  border: none;
  border-right: 1px solid var(--border);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--text-sm);
  cursor: pointer;
  transition: all var(--duration-fast);
  font-family: var(--font-sans);
}

.aec-room-btn:last-child {
  border-right: none;
}

.aec-room-btn:hover {
  background: var(--surface-hover);
  color: var(--text);
}

.aec-room-btn.active {
  background: var(--accent);
  color: #fff;
}

.aec-hint {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.aec-restart-hint {
  font-size: var(--text-sm);
  color: var(--warning);
}

/* ===== Responsive ===== */
@media (max-width: 768px) {
  .voice-config-grid {
    grid-template-columns: 1fr;
  }
}
</style>

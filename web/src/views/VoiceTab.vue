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

// --- WebSocket push handler for STT results ---
_wsHandler = (data: any) => {
  if (data?.type === 'push' && data?.module === 'voice' && data?.cmd === 'stt_result') {
    const text = data?.data?.text || ''
    if (text) sttResults.value.push(text)
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
watch([selectedSpeaker, volume, speed, captureDevice, playbackDevice, sttEnabled, ttsEnabled, punctEnabled], () => {
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

onMounted(async () => {
  await loadAll()
  await loadVoiceConfig()
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
          </div>
        </div>

        <!-- Config editor (toggle) -->
        <div v-if="showConfig" style="margin-top: var(--space-4); border-top: 1px solid var(--border); padding-top: var(--space-4);">
          <textarea class="form-textarea" style="min-height: 300px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="configContent" :disabled="!configExists"></textarea>
          <div style="margin-top: var(--space-2); display: flex; justify-content: flex-end;">
            <button class="btn btn-sm btn-primary" @click="saveConfig" :disabled="!configExists">保存</button>
          </div>
        </div>
      </div>
    </div>

    <!-- Section 2: Voice Configuration -->
    <div class="card">
      <div class="card-header"><h3 style="margin: 0;">语音配置</h3></div>
      <div class="card-body">
        <div class="settings-grid">
          <!-- Speaker -->
          <span class="settings-key">输出音色</span>
          <select class="form-select" v-model.number="selectedSpeaker" style="width: 100%;">
            <option v-for="sp in speakers" :key="sp.speaker_id" :value="sp.speaker_id">
              {{ sp.id }} ({{ sp.gender }})
            </option>
          </select>

          <!-- Volume -->
          <span class="settings-key">合成音量</span>
          <div style="display: flex; align-items: center; gap: var(--space-3);">
            <input type="range" min="1" max="100" v-model.number="volume" style="flex: 1;" />
            <span style="font-size: var(--text-sm); min-width: 32px; text-align: right;">{{ volume }}</span>
          </div>

          <!-- Speed -->
          <span class="settings-key">语速</span>
          <div style="display: flex; align-items: center; gap: var(--space-3);">
            <input type="range" min="0.5" max="2.0" step="0.1" v-model.number="speed" style="flex: 1;" />
            <span style="font-size: var(--text-sm); min-width: 40px; text-align: right;">{{ speed.toFixed(1) }}x</span>
          </div>

          <!-- Input device -->
          <span class="settings-key">输入设备</span>
          <select class="form-select" v-model="captureDevice" style="width: 100%;">
            <option value="">默认麦克风</option>
            <option v-for="dev in inputDevices" :key="dev.index" :value="dev.name">
              {{ dev.name }}{{ dev.is_default ? ' (默认)' : '' }}
            </option>
          </select>

          <!-- Output device -->
          <span class="settings-key">输出设备</span>
          <select class="form-select" v-model="playbackDevice" style="width: 100%;">
            <option value="">默认扬声器</option>
            <option v-for="dev in outputDevices" :key="dev.index" :value="dev.name">
              {{ dev.name }}{{ dev.is_default ? ' (默认)' : '' }}
            </option>
          </select>

          <!-- TTS toggle -->
          <span class="settings-key">TTS 模型</span>
          <label class="toggle-switch">
            <input type="checkbox" v-model="ttsEnabled" />
            <span class="toggle-slider"></span>
            <span class="toggle-label">{{ ttsEnabled ? '启用' : '停用' }}</span>
          </label>

          <!-- STT toggle -->
          <span class="settings-key">STT 模型</span>
          <label class="toggle-switch">
            <input type="checkbox" v-model="sttEnabled" />
            <span class="toggle-slider"></span>
            <span class="toggle-label">{{ sttEnabled ? '启用' : '停用' }}</span>
          </label>

          <!-- Punct toggle -->
          <span class="settings-key">标点模型</span>
          <label class="toggle-switch">
            <input type="checkbox" v-model="punctEnabled" :disabled="sttEnabled" />
            <span class="toggle-slider"></span>
            <span class="toggle-label">{{ punctEnabled ? '启用' : '停用' }}</span>
          </label>
        </div>
      </div>
    </div>

    <!-- Section 3: Voice Test -->
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

  </div>
</template>

<style scoped>
.settings-grid {
  display: grid;
  grid-template-columns: 120px 1fr;
  gap: var(--space-3) var(--space-4);
  align-items: center;
}
.settings-key {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
}

/* Range slider styling */
input[type="range"] {
  height: 6px;
  appearance: none;
  background: var(--border);
  border-radius: 3px;
  outline: none;
}
input[type="range"]::-webkit-slider-thumb {
  appearance: none;
  width: 16px;
  height: 16px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
}

.btn-danger {
  background: var(--danger, #ef4444);
  color: white;
  border-color: var(--danger, #ef4444);
}
.btn-danger:hover {
  opacity: 0.9;
}

/* Toggle switch */
.toggle-switch {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  position: relative;
}
.toggle-switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}
.toggle-slider {
  width: 36px;
  height: 20px;
  background: var(--border, #d1d5db);
  border-radius: 10px;
  position: relative;
  transition: background 0.2s;
  flex-shrink: 0;
}
.toggle-slider::after {
  content: '';
  position: absolute;
  width: 16px;
  height: 16px;
  background: white;
  border-radius: 50%;
  top: 2px;
  left: 2px;
  transition: transform 0.2s;
  box-shadow: 0 1px 3px rgba(0,0,0,0.15);
}
.toggle-switch input:checked + .toggle-slider {
  background: var(--accent, #3b82f6);
}
.toggle-switch input:checked + .toggle-slider::after {
  transform: translateX(16px);
}
.toggle-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  user-select: none;
}
.toggle-switch input:disabled + .toggle-slider {
  opacity: 0.4;
  cursor: not-allowed;
}
.toggle-switch input:disabled ~ .toggle-label {
  opacity: 0.5;
}
</style>

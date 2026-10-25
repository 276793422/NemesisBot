<script setup lang="ts">
// 本地模型页（2026-10-25 本地模型架构更新第一步）：
// TAB【框架】= 框架卡片网格（v1 仅 onnx / sherpa-onnx）。卡片内容/布局/
// 数据源与【语音通道】页【环境管理】卡的【运行库】节一致（voice.status →
// dlls），安装走同一 install_runtime 通路（后端同一单飞闸，两页互斥）。
// 后续 TAB（模型等）与第二框架卡片在此留位。
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('frameworks')
const loading = ref(true)
// voice 模块不可用原因（feature 裁剪 / 非 Windows / 请求失败）→ 空态诚实提示
const unavailable = ref('')
const status = ref<any>(null)
// 安装进行中（点击即置位，不等 SSE——确定性反馈）；进度文本/百分比由 SSE 驱动
const installing = ref(false)
const setupProgress = ref('')
const setupPct = ref<number | null>(null)

const dlls = computed<any[]>(() => status.value?.dlls || [])
const runtimeReady = computed(() => !!status.value?.all_dlls_present)

function formatSize(bytes: number | null | undefined): string {
  if (!bytes) return ''
  if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  if (bytes >= 1024) return (bytes / 1024).toFixed(0) + ' KB'
  return bytes + ' B'
}

async function load() {
  loading.value = true
  try {
    status.value = await request('voice', 'status')
    unavailable.value = ''
  } catch (e: any) {
    unavailable.value = String(e)
    status.value = null
  }
  loading.value = false
}

async function installRuntime() {
  if (installing.value) return
  installing.value = true
  setupProgress.value = '正在安装 onnx 运行库...'
  setupPct.value = null
  try {
    await request('voice', 'install_runtime', undefined, 0)
    toast.success('onnx 运行库安装完成')
  } catch (e: any) {
    toast.error('onnx 运行库安装失败: ' + e)
  }
  installing.value = false
  setupProgress.value = ''
  setupPct.value = null
  await load()
}

// SSE 安装进度（与语音页同一事件流；phase=runtime 属本页职责）。
// status=progress 携带 stage/pct/message → 文本 + 进度条实时更新。
function _onSetupProgress(data: any) {
  if (data?.phase === 'model') return // 模型进度归语音通道页设置弹窗
  if (data?.status === 'progress') {
    if (data.message) setupProgress.value = data.message
    setupPct.value = typeof data.pct === 'number' ? data.pct : null
    return
  }
  if (data?.message) setupProgress.value = data.message
  if (data?.status === 'complete' || data?.status === 'error') {
    setupPct.value = null
    setTimeout(() => { setupProgress.value = '' }, 2000)
    load()
  }
}
sseOn('voice-setup', _onSetupProgress)
onUnmounted(() => sseOff('voice-setup', _onSetupProgress))

onMounted(load)
</script>

<template>
  <div class="page-local-models">
    <div class="page-header"><h2>本地模型</h2></div>
    <div class="page-body">
      <!-- TAB 栏（v1 仅框架；后续模型 TAB 在此追加） -->
      <div class="lm-tabs">
        <button
          class="lm-tab"
          :class="{ active: activeTab === 'frameworks' }"
          @click="activeTab = 'frameworks'"
        >框架</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else-if="unavailable" class="empty-state">
        <h3>框架信息不可用</h3>
        <p>{{ unavailable }}</p>
        <p style="font-size: var(--text-sm); color: var(--text-secondary);">本地模型框架（onnx / sherpa-onnx）当前仅 Windows 平台支持。</p>
      </div>

      <!-- 框架卡片网格 -->
      <div v-else class="lm-grid">
        <div class="card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0; display: flex; align-items: center; gap: var(--space-2);">
              onnx
              <span
                :style="{
                  fontSize: '11px',
                  padding: '1px 8px',
                  borderRadius: 'var(--radius-sm)',
                  background: runtimeReady ? 'var(--success-bg, rgba(34,197,94,0.12))' : 'var(--warning-bg, rgba(234,179,8,0.12))',
                  color: runtimeReady ? 'var(--success)' : 'var(--warning)',
                }"
              >{{ runtimeReady ? '就绪' : '未完整安装' }}</span>
            </h3>
            <button class="btn btn-sm btn-primary" @click="installRuntime" :disabled="installing || runtimeReady">
              {{ installing ? '安装中...' : '安装' }}
            </button>
          </div>
          <div class="card-body">
            <div style="font-size: var(--text-sm); color: var(--text-secondary); margin-bottom: var(--space-2);">
              sherpa-onnx 推理运行库 · 语音识别 / 合成 / VAD 等本地模型共用
            </div>
            <!-- 安装进度：spinner + 文本 + 进度条（pct 未知时不确定条动画） -->
            <div
              v-if="setupProgress"
              style="margin: var(--space-2) 0; padding: var(--space-3); border-radius: var(--radius-md); background: var(--accent-bg, rgba(59,130,246,0.08)); border: 1px solid var(--accent);"
            >
              <div style="display: flex; align-items: center; gap: var(--space-3); margin-bottom: var(--space-2);">
                <div class="spinner spinner-sm"></div>
                <span style="font-size: var(--text-sm); color: var(--accent);">{{ setupProgress }}</span>
              </div>
              <div class="lm-progress-track">
                <div
                  class="lm-progress-bar"
                  :class="{ indeterminate: setupPct === null }"
                  :style="{ width: setupPct !== null ? Math.min(100, Math.max(0, setupPct)) + '%' : '' }"
                ></div>
              </div>
            </div>
            <div style="padding-left: var(--space-1);">
              <div
                v-for="dll in dlls"
                :key="dll.name"
                style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0; font-size: var(--text-sm);"
              >
                <span :style="{ color: dll.exists ? 'var(--success)' : 'var(--text-secondary)' }">{{ dll.exists ? '●' : '○' }}</span>
                <span>{{ dll.name }}</span>
                <span v-if="dll.size_bytes" style="color: var(--text-secondary);">({{ formatSize(dll.size_bytes) }})</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.lm-tabs {
  display: flex;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
  border-bottom: 1px solid var(--border-light);
  padding-bottom: var(--space-2);
}
.lm-tab {
  background: none;
  border: none;
  padding: var(--space-2) var(--space-4);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  cursor: pointer;
  border-radius: var(--radius-md);
}
.lm-tab:hover {
  color: var(--text);
  background: var(--surface-hover, rgba(128, 128, 128, 0.08));
}
.lm-tab.active {
  color: var(--accent);
  background: var(--accent-bg, rgba(59, 130, 246, 0.1));
  font-weight: 500;
}
.lm-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
  gap: var(--space-4);
}
.lm-progress-track {
  height: 6px;
  border-radius: 3px;
  background: var(--border-light, rgba(128, 128, 128, 0.2));
  overflow: hidden;
}
.lm-progress-bar {
  height: 100%;
  border-radius: 3px;
  background: var(--accent);
  transition: width 0.4s ease;
}
.lm-progress-bar.indeterminate {
  width: 35% !important;
  animation: lm-indeterminate 1.2s ease-in-out infinite;
}
@keyframes lm-indeterminate {
  0% { margin-left: -35%; }
  100% { margin-left: 100%; }
}
</style>

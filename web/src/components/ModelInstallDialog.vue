<script setup lang="ts">
// 模型设置弹窗（2026-10-25 本地模型架构更新）：
// - 列出该类全部可装变体（voice.model_catalog），逐行安装（install-by-name）
//   与「设为当前」（voice.set_model，写 config.toml 后引擎重启生效）。
// - 安装进度经 SSE voice-setup（phase=model + name 匹配）在行内可见。
// - 唯一关闭途径 = 显式【关闭】按钮：backdrop 点击 / Esc 一律不关
//   （用户看着安装进度的硬需求，不可照抄 ForkSessionModal 的 @click.self）。
// - 单飞：一行安装中，其余【安装】禁用；【设为当前】不受安装锁影响。
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { useToast } from '../composables/useToast'

const props = defineProps<{ category: string; label: string }>()
const emit = defineEmits<{ (e: 'close'): void; (e: 'changed'): void }>()

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const variants = ref<any[]>([])
const installing = ref<string | null>(null) // 安装中的变体名（null = 空闲）
const progressText = ref('')
const settingCurrent = ref<string | null>(null)
// 刚设为当前成功的变体名 → 行内小字提示「语音引擎重启后生效」
const restartHint = ref<string | null>(null)

async function load() {
  try {
    const catalog = await request('voice', 'model_catalog')
    variants.value = catalog?.categories?.[props.category] || []
  } catch (e: any) {
    toast.error('加载模型目录失败: ' + e)
    variants.value = []
  }
  loading.value = false
}

async function install(v: any) {
  if (installing.value) return // 单飞闸（双保险，按钮已禁用）
  installing.value = v.name
  progressText.value = '准备安装...'
  try {
    await request('voice', 'install_model', { model: props.category, name: v.name }, 0)
    toast.success(`${v.name} 安装完成`)
  } catch (e: any) {
    toast.error(`${v.name} 安装失败: ` + e)
  } finally {
    installing.value = null
    progressText.value = ''
    await load()
    emit('changed')
  }
}

async function setCurrent(v: any) {
  settingCurrent.value = v.name
  try {
    await request('voice', 'set_model', { category: props.category, name: v.name })
    toast.success(`已设为当前：${v.name}`)
    restartHint.value = v.name
    await load()
    emit('changed')
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  } finally {
    settingCurrent.value = null
  }
}

// SSE 安装进度（phase=model 按 name 匹配行；后端 install-by-name 帧带 name）
function _onSetupProgress(data: any) {
  if (data?.phase !== 'model') return
  if (data?.name !== installing.value) return
  if (data?.status === 'starting') progressText.value = data.message || '开始安装...'
  else if (data?.status === 'progress' && data?.message) progressText.value = data.message
}
sseOn('voice-setup', _onSetupProgress)
onUnmounted(() => sseOff('voice-setup', _onSetupProgress))

onMounted(load)
</script>

<template>
  <!-- 无 @click.self：点 backdrop 不关（只有显式关闭按钮可关） -->
  <div class="modal-backdrop">
    <div class="modal" role="dialog" aria-modal="true" :aria-label="`${label}模型设置`">
      <div class="modal-header">
        <h3 style="margin: 0;">{{ label }}模型设置</h3>
        <button class="btn btn-sm" @click="emit('close')">关闭</button>
      </div>
      <div class="modal-body" style="display: flex; flex-direction: column; gap: var(--space-2);">
        <div v-if="loading" style="text-align: center; padding: var(--space-6);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>

        <div v-else-if="variants.length === 0" style="text-align: center; padding: var(--space-6); color: var(--text-secondary);">
          暂无可选模型
        </div>

        <template v-else>
          <div
            v-for="v in variants"
            :key="v.name"
            style="display: flex; align-items: center; gap: var(--space-3); padding: var(--space-2) var(--space-3); border: 1px solid var(--border-light); border-radius: var(--radius-md);"
          >
            <span :style="{ color: v.installed ? 'var(--success)' : 'var(--text-secondary)' }">{{ v.installed ? '●' : '○' }}</span>
            <div style="flex: 1; min-width: 0;">
              <div style="display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap;">
                <span style="font-weight: 500; font-size: var(--text-sm);">{{ v.name }}</span>
                <span
                  v-if="v.is_current"
                  style="font-size: 11px; padding: 1px 6px; border-radius: var(--radius-sm); background: var(--success-bg, rgba(34,197,94,0.12)); color: var(--success);"
                >✓ 当前使用</span>
              </div>
              <div style="font-size: 12px; color: var(--text-secondary); margin-top: 2px;">
                {{ v.description || v.repo }}<template v-if="v.file_count"> · {{ v.file_count }} 文件</template>
              </div>
              <div v-if="installing === v.name" style="display: flex; align-items: center; gap: var(--space-2); margin-top: 4px;">
                <div class="spinner spinner-sm"></div>
                <span style="font-size: 12px; color: var(--accent);">{{ progressText || '安装中...' }}</span>
              </div>
              <div v-if="restartHint === v.name && v.is_current" style="font-size: 12px; color: var(--warning); margin-top: 4px;">
                语音引擎重启后生效
              </div>
            </div>
            <div style="display: flex; gap: var(--space-2); flex-shrink: 0;">
              <!-- 未安装 → 安装（单飞：任一行安装中全部禁用） -->
              <button
                v-if="!v.installed"
                class="btn btn-sm btn-primary"
                :disabled="!!installing"
                @click="install(v)"
              >{{ installing === v.name ? '安装中' : '安装' }}</button>
              <!-- 已安装非当前 → 设为当前 -->
              <button
                v-else-if="!v.is_current"
                class="btn btn-sm"
                :disabled="settingCurrent === v.name"
                @click="setCurrent(v)"
              >{{ settingCurrent === v.name ? '设置中' : '设为当前' }}</button>
              <!-- 当前使用 → 无操作 -->
              <span v-else style="font-size: 12px; color: var(--text-secondary); align-self: center;">使用中</span>
            </div>
          </div>
        </template>
      </div>
      <div class="modal-footer" style="justify-content: space-between; align-items: center;">
        <span style="font-size: 12px; color: var(--text-secondary);">设为当前写入 config.toml，语音引擎重启后生效</span>
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>

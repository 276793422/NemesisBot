<script setup lang="ts">
/**
 * 知识库设置窗（天工系统 v1，2026-11-26）——knowledge.get/set 前端面。
 *
 * 程序不理解「知识库」：这里只编辑一份声明式配置（总开关 + code_root/
 * info_root 配对 + 注入模式），保存后后端机械执行（TIANGONG 存在性声明
 * 注入 + read/write/edit/multiedit 路径重定向），语义由 LLM 从声明与
 * TIANGONG 内容获得。
 *
 * 关窗校验（用户裁决）：总开关开着但没有任何有效配对 → 阻止关闭并提示
 * （必须关开关或配好至少一对才能退出）——防止「开了开关却空转」的静默
 * 无效态。
 *
 * 生效时机诚实声明：声明注入与路径重定向**写盘即热生效**（后端 mtime
 * 缓存）；配对目录（项目目录 + 知识库目录）的围栏放行（extra_roots）
 * **重启网关后生效**（Arc 注册期烧入）——与附加放行目录同口径。
 */
import { ref } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

export interface KnowledgePair {
  code_root: string
  info_root: string
  tiangong_load: string
}

export interface KnowledgeConfigFront {
  enabled: boolean
  pairs: KnowledgePair[]
}

const props = defineProps<{
  /** 会话 id（空 = 主对话）。 */
  sessionId?: string
  /** 配置落点（knowledge.get 返回的 config_path，展示用）。 */
  configPath: string
  /** 当前生效配置（knowledge.get 返回的 config）。 */
  config: KnowledgeConfigFront
  /** code_root 下拉候选（主 workspace + 项目目录盘点）。 */
  dirOptions?: string[]
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'saved', config: KnowledgeConfigFront): void
}>()

const { request } = useWSAPI()
const toast = useToast()

const enabled = ref(props.config?.enabled ?? false)
const pairs = ref<KnowledgePair[]>(
  (props.config?.pairs ?? []).map(p => ({
    code_root: p.code_root ?? '',
    info_root: p.info_root ?? '',
    tiangong_load: p.tiangong_load === 'full' ? 'full' : 'pointer',
  })),
)
const inlineError = ref('')
const busy = ref(false)

/** 绝对路径形态：Windows 盘符或 POSIX 根（仅形态校验，存在性/重叠由
 *  后端 validate_knowledge_config 权威判定）。 */
function isAbsolutePath(p: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('/')
}

/** 有效配对：两路径都填且都是绝对路径形态。 */
function isPairValid(p: KnowledgePair): boolean {
  return !!p.code_root.trim() && !!p.info_root.trim()
    && isAbsolutePath(p.code_root.trim()) && isAbsolutePath(p.info_root.trim())
}

/** 关窗校验（用户裁决）：开关开着但没有任何有效配对 → 阻止并提示。 */
function guardClose(): boolean {
  if (enabled.value && !pairs.value.some(isPairValid)) {
    inlineError.value = '知识库开关已开启，但尚未配置有效配对（项目目录 + 知识库目录）。请完成配对，或关闭总开关后再退出。'
    return false
  }
  return true
}

function addPair() {
  pairs.value.push({ code_root: '', info_root: '', tiangong_load: 'pointer' })
}

function removePair(i: number) {
  pairs.value.splice(i, 1)
}

/** 取消 = 关窗校验同样生效（改过的开关态算数——与「关窗」同语义）。 */
function onCancel() {
  if (guardClose()) emit('close')
}

async function save() {
  inlineError.value = ''
  // 形态前置校验（后端 validate 是权威，这里先拦明显残缺降低往返）。
  for (let i = 0; i < pairs.value.length; i++) {
    const p = pairs.value[i]
    if (!p.code_root.trim() && !p.info_root.trim()) continue // 空行跳过（保存时剔除）
    if (!isAbsolutePath(p.code_root.trim()) || !isAbsolutePath(p.info_root.trim())) {
      inlineError.value = `配对 ${i + 1}：项目目录与知识库目录都必须是绝对路径（如 C:\\works\\proj 或 /home/me/kb）`
      return
    }
  }
  if (!guardClose()) return
  const kept = pairs.value
    .filter(p => p.code_root.trim() || p.info_root.trim())
    .map(p => ({
      code_root: p.code_root.trim(),
      info_root: p.info_root.trim(),
      tiangong_load: p.tiangong_load === 'full' ? 'full' : 'pointer',
    }))
  if (busy.value) return
  busy.value = true
  try {
    await request('knowledge', 'set', {
      session_id: props.sessionId || '',
      config: { enabled: enabled.value, pairs: kept },
    })
    toast.success('知识库配置已保存（声明与重定向热生效；项目与知识库目录围栏放行重启网关后生效）')
    emit('saved', { enabled: enabled.value, pairs: kept })
  } catch (e: unknown) {
    toast.error(typeof e === 'string' ? e : (e as Error)?.message || '保存失败')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="modal-backdrop" @click.self="onCancel">
    <div class="modal">
      <div class="modal-header">
        <h3>知识库设置</h3>
        <button class="close-btn" title="关闭" @click="onCancel">×</button>
      </div>
      <div class="modal-body">
        <p class="hint">
          知识库 = 把项目内缺失的引导文件（AGENTS.md / CLAUDE.md / TIANGONG.md）
          重定向到知识库目录的镜像副本。本地已有的文件优先，不镜像；AI 会在
          上下文里看到「本项目存在天工引导文件」的提示并按需自行阅读。
        </p>
        <label class="form-switch">
          <input v-model="enabled" type="checkbox" data-test="kb-enabled" />
          <span>启用知识库重定向（总开关，默认关闭）</span>
        </label>

        <div v-if="pairs.length" class="pair-list">
          <div v-for="(p, i) in pairs" :key="i" class="pair-row" data-test="kb-pair">
            <div class="pair-fields">
              <div class="form-group">
                <label class="form-label">项目目录（代码根）</label>
                <input
                  v-model="p.code_root"
                  class="form-input"
                  list="kb-dir-options"
                  placeholder="如 C:\works\proj（绝对路径）"
                />
              </div>
              <div class="form-group">
                <label class="form-label">知识库目录（信息根）</label>
                <input
                  v-model="p.info_root"
                  class="form-input"
                  placeholder="镜像引导文件的存放目录（绝对路径）"
                />
              </div>
              <div class="form-group">
                <label class="form-label">TIANGONG 注入模式</label>
                <select v-model="p.tiangong_load" class="form-input">
                  <option value="pointer">指针声明（只提示存在，AI 按需读）</option>
                  <option value="full">全文注入（根层生效时全文进上下文）</option>
                </select>
              </div>
            </div>
            <button class="pair-remove" type="button" title="删除该配对" @click="removePair(i)">✕</button>
          </div>
        </div>
        <p v-else class="hint muted-hint">尚未配置任何配对。</p>
        <datalist id="kb-dir-options">
          <option v-for="d in props.dirOptions ?? []" :key="d" :value="d" />
        </datalist>
        <button class="btn add-pair-btn" type="button" data-test="kb-add-pair" @click="addPair">＋ 添加配对</button>

        <div class="notes">
          <p>· 声明扫描粒度：项目根目录 + 一级子目录（更深层级请写进 TIANGONG 内容引导 AI）。</p>
          <p>· 声明注入与路径重定向：保存后热生效，无需重启。</p>
          <p>· 配对目录（项目目录 + 知识库目录）的围栏放行：重启网关后生效。</p>
          <p class="kb-config-path" :title="configPath">配置文件：{{ configPath }}</p>
        </div>
        <div v-if="inlineError" class="inline-error" data-test="kb-error">{{ inlineError }}</div>
      </div>
      <div class="modal-footer">
        <button class="btn" data-test="kb-cancel" @click="onCancel">取消</button>
        <button class="btn primary" :disabled="busy" data-test="kb-save" @click="save">
          {{ busy ? '保存中…' : '保存' }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.form-switch {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  margin: 8px 0 12px;
  cursor: pointer;
}
.pair-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-bottom: 10px;
}
.pair-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-primary);
}
.pair-fields {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.pair-fields .form-group {
  margin-bottom: 0;
}
.pair-remove {
  background: none;
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px 8px;
}
.pair-remove:hover {
  color: var(--danger, #e05252);
  border-color: var(--danger, #e05252);
}
.add-pair-btn {
  margin-bottom: 12px;
}
.muted-hint {
  margin-bottom: 8px;
}
.notes {
  font-size: 11px;
  color: var(--text-muted);
  line-height: 1.7;
  border-top: 1px dashed var(--border);
  padding-top: 8px;
}
.notes p {
  margin: 0;
}
.kb-config-path {
  word-break: break-all;
}
</style>

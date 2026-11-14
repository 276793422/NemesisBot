<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 专家Agent 管理页（2026-11-13 专家子代理）：
// - 默认子代理置顶锁定卡（仅可改名，无开关、不可删除——出厂常驻）；
// - 专家卡片栅格：启用开关（停用 = spawn 名册摘除，可逆）+ 编辑/删除；
// - 空名册引导人格超市【雇佣专家】（专家可在超市原记录重复雇佣）。
// 数据源 experts.* WSAPI；执行面 spawn 工具 expert 参数（名册不进
// system prompt/schema，切页零 prompt cache 影响）。

const { request } = useWSAPI()
const toast = useToast()
const router = useRouter()

interface ExpertEntry {
  slug: string
  name: string
  summary: string
  tags: string[]
  model: string
  enabled: boolean
  source: string // manual | shop
  hired_from: string
  prompt?: string
}

const experts = ref<ExpertEntry[]>([])
const defaultName = ref('默认子代理')
const loading = ref(true)

// 模式策略（agents.experts 三键，2026-11-13 总开关 + 自动降级）：
// enabled=总开关（默认关——关时 spawn 点名/prompt 诚实拒绝并给开启指引）；
// degrade=自动降级（默认开——无匹配专家回灌指引让模型自拟 prompt 重调，
// 不问用户）；notify=降级通知（默认关——静默降级）。
const policy = ref({ enabled: false, degrade: true, notify: false })
const policyLoaded = ref(false)

async function loadPolicy() {
  try {
    const p = await request('experts', 'config_get')
    policy.value = {
      enabled: !!p?.enabled,
      degrade: !!p?.degrade,
      notify: !!p?.notify,
    }
    policyLoaded.value = true
  } catch (e: any) {
    // 策略面缺席（旧装配/桥未注入）不阻塞页面主体——静默保留保守默认。
    policyLoaded.value = false
  }
}

async function togglePolicy(key: 'enabled' | 'degrade' | 'notify') {
  const next = !policy.value[key]
  try {
    const p = await request('experts', 'config_set', { [key]: next })
    policy.value = {
      enabled: !!p?.enabled,
      degrade: !!p?.degrade,
      notify: !!p?.notify,
    }
    if (key === 'enabled') {
      if (next) {
        toast.success('专家模式已开启（本次启用已后台记录）')
      } else {
        toast.info('专家模式已关闭：spawn 点名专家/自拟提示词将诚实拒绝')
      }
    } else if (key === 'degrade') {
      toast.success(next ? '自动降级已开启：无匹配专家时模型自拟提示词续跑' : '自动降级已关闭：无匹配专家将诚实拒绝')
    } else {
      toast.success(next ? '降级通知已开启' : '降级通知已关闭（静默降级）')
    }
  } catch (e: any) {
    toast.error(String(e))
  }
}

const showEditDialog = ref(false)
const editingSlug = ref<string | null>(null) // null = 新建
const confirmDeleteSlug = ref('')
const showRenameDefault = ref(false)

// 编辑模态 prompt 拉取生命周期（BUG-20261114-001）：
// - promptLoaded：get 成功返回才置 true——失败/在途期间 saveExpert 不携带
//   prompt 字段（后端 partial update 缺省 = 保留现值），堵住「瞬时读失败 →
//   空串写回 → 存量提示词被清空」数据丢失通道；
// - promptFetchSeq：单调递增请求代序——关 A 开 B 后 A 的晚到响应不得写进
//   B 的编辑框（SessionSidebar BUG-20261106-029 同族围栏先例）。
const promptLoaded = ref(false)
let promptFetchSeq = 0

const defaultForm = () => ({
  name: '',
  summary: '',
  tagsText: '',
  model: '',
  prompt: '',
})
const form = ref(defaultForm())

const hasExperts = computed(() => experts.value.length > 0)

async function load() {
  loading.value = true
  try {
    const data = await request('experts', 'list')
    experts.value = data?.experts || []
    defaultName.value = data?.default_name || '默认子代理'
  } catch (e: any) {
    toast.error('加载专家名册失败: ' + e)
  }
  loading.value = false
}

function openCreate() {
  editingSlug.value = null
  form.value = defaultForm()
  showEditDialog.value = true
}

function openEdit(e: ExpertEntry) {
  editingSlug.value = e.slug
  form.value = {
    name: e.name,
    summary: e.summary || '',
    tagsText: (e.tags || []).join(', '),
    model: e.model || '',
    prompt: '',
  }
  promptLoaded.value = false
  const seq = ++promptFetchSeq
  // 编辑模态先拉 prompt 全文（list 面不带，体积面）；过期响应（已换目标/
  // 重开模态）按代序丢弃，不写共享 form。
  request('experts', 'get', { key: e.slug })
    .then((full: any) => {
      if (seq !== promptFetchSeq) return
      form.value.prompt = full?.prompt || ''
      promptLoaded.value = true
    })
    .catch((err: any) => {
      if (seq !== promptFetchSeq) return
      toast.error('读取专家详情失败: ' + err)
    })
  showEditDialog.value = true
}

async function saveExpert() {
  if (!form.value.name.trim()) {
    toast.warn('请填写专家名')
    return
  }
  const payload: any = {
    name: form.value.name.trim(),
    summary: form.value.summary.trim(),
    tags: form.value.tagsText
      .split(/[,，]/)
      .map((t: string) => t.trim())
      .filter(Boolean),
    model: form.value.model.trim(),
  }
  // prompt 仅在可安全携带时上行（BUG-20261114-001）：新建空串是合法草稿
  // 恒携带；编辑态 get 未成功加载（失败/在途）则缺省——后端保留现值，
  // 不把空串/在途半态写回存量提示词。
  if (!editingSlug.value || promptLoaded.value) {
    payload.prompt = form.value.prompt
  }
  try {
    if (editingSlug.value) {
      await request('experts', 'update', { key: editingSlug.value, ...payload })
      toast.success('专家已更新')
    } else {
      await request('experts', 'create', payload)
      toast.success('专家已创建')
    }
    showEditDialog.value = false
    await load()
  } catch (e: any) {
    toast.error(String(e))
  }
}

// 在飞围栏（BUG-20261114-001）：双击第二发读到的仍是旧 e.enabled，两发
// 同值请求 + 响应后本地翻转会让 UI 与名册状态相反（服务端停在第一发的
// 目标值，UI 翻转两次回到原值）。在飞期间忽略重入。
const toggleBusy = new Set<string>()
async function toggleExpert(e: ExpertEntry) {
  if (toggleBusy.has(e.slug)) return
  toggleBusy.add(e.slug)
  try {
    await request('experts', 'set_enabled', { key: e.slug, enabled: !e.enabled })
    e.enabled = !e.enabled
    if (!e.enabled) {
      toast.info(`「${e.name}」已停用：子代理名册摘除（定义保留，可随时启用）`)
    } else {
      toast.success(`「${e.name}」已启用`)
    }
  } catch (err: any) {
    toast.error(String(err))
  } finally {
    toggleBusy.delete(e.slug)
  }
}

async function removeExpert(e: ExpertEntry) {
  if (confirmDeleteSlug.value !== e.slug) {
    confirmDeleteSlug.value = e.slug
    return
  }
  try {
    await request('experts', 'remove', { key: e.slug })
    toast.success(`「${e.name}」已删除（超市来源可从人格超市重新雇佣）`)
    confirmDeleteSlug.value = ''
    await load()
  } catch (err: any) {
    toast.error(String(err))
  }
}

async function saveDefaultName() {
  if (!form.value.name.trim()) {
    toast.warn('请填写新名称')
    return
  }
  try {
    await request('experts', 'update', { key: '_default', name: form.value.name.trim() })
    defaultName.value = form.value.name.trim()
    toast.success('默认子代理已改名')
    showRenameDefault.value = false
  } catch (e: any) {
    toast.error(String(e))
  }
}

onMounted(() => {
  load()
  loadPolicy()
})
</script>

<template>
  <div class="page-experts">
    <div class="page-header">
      <h2>专家Agent</h2>
      <div class="page-header-actions">
        <button class="btn btn-primary" @click="openCreate">＋ 新建专家</button>
      </div>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-if="!loading">
        <!-- 模式策略条（agents.experts 三键）：总开关默认关（关时 spawn
             点名/prompt 诚实拒绝并给开启指引）；降级默认开；通知默认关。 -->
        <div class="expert-policy-bar">
          <div class="expert-policy-item">
            <div
              class="toggle"
              :class="{ active: policy.enabled }"
              data-test="policy-enabled"
              :title="policy.enabled ? '专家模式已开启（点击关闭）' : '专家模式已关闭（点击开启）'"
              @click="togglePolicy('enabled')"
            ></div>
            <div>
              <div class="expert-policy-label">专家模式</div>
              <div class="expert-policy-hint">关闭时 spawn 点名专家/自拟提示词将被拒绝（错误信息自带开启指引）</div>
            </div>
          </div>
          <div class="expert-policy-item" :class="{ 'is-inert': !policy.enabled }">
            <div
              class="toggle"
              :class="{ active: policy.degrade }"
              data-test="policy-degrade"
              :title="policy.degrade ? '自动降级已开启（点击关闭）' : '自动降级已关闭（点击开启）'"
              @click="togglePolicy('degrade')"
            ></div>
            <div>
              <div class="expert-policy-label">自动降级</div>
              <div class="expert-policy-hint">无匹配专家时不问用户：回灌指引让模型自拟提示词重调（长任务防停车）</div>
            </div>
          </div>
          <div class="expert-policy-item" :class="{ 'is-inert': !policy.enabled }">
            <div
              class="toggle"
              :class="{ active: policy.notify }"
              data-test="policy-notify"
              :title="policy.notify ? '降级通知已开启（点击关闭）' : '降级通知已关闭（点击开启）'"
              @click="togglePolicy('notify')"
            ></div>
            <div>
              <div class="expert-policy-label">降级通知</div>
              <div class="expert-policy-hint">发生自动降级时弹出提示（默认静默）</div>
            </div>
          </div>
        </div>

        <!-- 默认子代理：置顶锁定卡（仅可改名，无开关、不可删除） -->
        <div class="expert-default-row">
          <div class="skill-card expert-default-card">
            <div class="skill-card-header">
              <div>
                <div class="skill-name">{{ defaultName }}</div>
                <div class="skill-author">spawn 未点名专家时的兜底执行形态（继承主对话模型与主人格）</div>
              </div>
              <span class="badge badge-neutral">出厂常驻</span>
            </div>
            <div style="display: flex; gap: var(--space-2); margin-top: var(--space-2);">
              <button class="btn btn-sm" @click="form.name = defaultName; showRenameDefault = true">✏ 改名</button>
            </div>
          </div>
        </div>

        <!-- 专家卡片栅格 -->
        <div v-if="!hasExperts" class="empty-state" style="margin-top: var(--space-5);">
          <h3>暂无雇佣专家</h3>
          <p>到人格超市挑选现成专家一键雇佣，或手工新建</p>
          <div style="display: flex; gap: var(--space-2); justify-content: center; margin-top: var(--space-3);">
            <button class="btn btn-primary" @click="router.push('/persona-shop')">🛒 前往人格超市</button>
            <button class="btn" @click="openCreate">＋ 手工新建</button>
          </div>
        </div>
        <div v-else style="display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: var(--space-4); margin-top: var(--space-4);">
          <div v-for="e in experts" :key="e.slug" class="skill-card" :style="{ opacity: e.enabled ? 1 : 0.55 }">
            <div class="skill-card-header">
              <div>
                <div class="skill-name">{{ e.name }}</div>
                <div style="display: flex; gap: 4px; margin-top: 4px; flex-wrap: wrap;">
                  <span class="badge" :class="e.source === 'shop' ? 'badge-success' : 'badge-neutral'" style="font-size: 0.65rem;">
                    {{ e.source === 'shop' ? '超市雇佣' : '手工创建' }}
                  </span>
                  <span v-if="e.model" class="badge badge-neutral" style="font-size: 0.65rem;">{{ e.model }}</span>
                </div>
              </div>
              <div
                class="toggle"
                :class="{ active: e.enabled }"
                :title="e.enabled ? '已启用（点击停用）' : '已停用（点击启用）'"
                @click="toggleExpert(e)"
              ></div>
            </div>
            <div class="skill-description" style="display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">
              {{ e.summary || '暂无描述' }}
            </div>
            <div v-if="e.tags && e.tags.length" style="display: flex; gap: 4px; flex-wrap: wrap; margin-top: var(--space-2);">
              <span v-for="t in e.tags" :key="t" class="badge badge-neutral" style="font-size: 0.6rem;">{{ t }}</span>
            </div>
            <div style="display: flex; gap: var(--space-2); margin-top: var(--space-3);">
              <button class="btn btn-sm" @click="openEdit(e)">编辑</button>
              <template v-if="confirmDeleteSlug === e.slug">
                <span style="font-size: var(--text-xs); color: var(--error); align-self: center;">确定解雇？</span>
                <button class="btn btn-sm btn-danger" @click="removeExpert(e)">确认</button>
                <button class="btn btn-sm" @click="confirmDeleteSlug = ''">取消</button>
              </template>
              <button v-else class="btn btn-sm btn-danger" @click="removeExpert(e)">删除</button>
            </div>
          </div>
        </div>
      </template>

      <!-- 新建/编辑模态 -->
      <div v-if="showEditDialog" class="modal-backdrop" @click.self="showEditDialog = false">
        <div class="modal" style="max-width: 560px;">
          <div class="modal-header">
            <h3>{{ editingSlug ? '编辑专家' : '新建专家' }}</h3>
            <button class="modal-close" @click="showEditDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <div class="form-group">
              <label class="form-label">专家名 *</label>
              <input class="form-input" v-model="form.name" placeholder="例如: code-reviewer" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">一句话简介</label>
              <input class="form-input" v-model="form.summary" placeholder="模型选专家时看到的第一行" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">标签（逗号分隔）</label>
              <input class="form-input" v-model="form.tagsText" placeholder="例如: review, rust" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">模型覆盖（留空 = 继承主对话模型）</label>
              <input class="form-input" v-model="form.model" placeholder="model_list 里的别名，例如 gpt-6.1-sol" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">系统提示词（整段替换主人格，需自足）</label>
              <textarea class="form-textarea" v-model="form.prompt" rows="10" placeholder="You are a code review expert. When given a diff, ..." style="width: 100%; font-family: monospace;"></textarea>
              <p v-if="editingSlug && !promptLoaded" style="font-size: var(--text-xs); color: var(--text-muted); margin-top: 4px;">
                提示词加载中/加载失败——本次保存不会改动现有提示词
              </p>
            </div>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showEditDialog = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="saveExpert" :disabled="!form.name.trim()">{{ editingSlug ? '保存' : '创建' }}</button>
          </div>
        </div>
      </div>

      <!-- 默认子代理改名模态 -->
      <div v-if="showRenameDefault" class="modal-backdrop" @click.self="showRenameDefault = false">
        <div class="modal" style="max-width: 400px;">
          <div class="modal-header">
            <h3>默认子代理改名</h3>
            <button class="modal-close" @click="showRenameDefault = false">&times;</button>
          </div>
          <div class="modal-body">
            <div class="form-group">
              <label class="form-label">新名称 *</label>
              <input class="form-input" v-model="form.name" placeholder="例如: 总管" style="width: 100%;">
              <p style="font-size: var(--text-xs); color: var(--text-muted); margin-top: 4px;">
                仅显示名变化——执行形态零改变（spawn 未点名专家时的兜底）。
              </p>
            </div>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showRenameDefault = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="saveDefaultName" :disabled="!form.name.trim()">保存</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.expert-default-row {
  display: flex;
  justify-content: flex-end;
}

.expert-default-card {
  width: 340px;
  border-style: dashed;
}

.expert-policy-bar {
  display: flex;
  gap: var(--space-6, 24px);
  flex-wrap: wrap;
  padding: var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius, 8px);
  background: var(--bg-elevated, transparent);
  margin-bottom: var(--space-4);
}

.expert-policy-item {
  display: flex;
  align-items: flex-start;
  gap: var(--space-3, 12px);
  flex: 1;
  min-width: 220px;
}

.expert-policy-item.is-inert {
  opacity: 0.5;
}

.expert-policy-label {
  font-weight: 600;
  font-size: var(--text-sm, 0.875rem);
}

.expert-policy-hint {
  font-size: var(--text-xs, 0.75rem);
  color: var(--text-muted);
  margin-top: 2px;
}
</style>

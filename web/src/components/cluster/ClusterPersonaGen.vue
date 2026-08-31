<script setup lang="ts">
import { ref, computed } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface CoverageEntry {
  unit_id: string
  status: 'covered' | 'skipped' | 'missing' | 'suspect'
  location?: string
  reason?: string
}

interface CoverageReport {
  total: number
  covered: number
  skipped: number
  missing: number
  suspect: number
  coverage_rate: number
  entries: CoverageEntry[]
  segment_gaps: string[]
}

interface PersonaPackage {
  node_name: string
  display_name: string
  emoji: string
  role: string
  category: string
  tags: string[]
  identity_md: string
  soul_md: string
  coverage?: CoverageReport | null
}

// 双区输入，各自独立
const jdText = ref('')
const resumeText = ref('')

// 生成状态：'jd' | 'resume' | null
const generating = ref<'jd' | 'resume' | null>(null)
// 生成结果（可编辑预览）
const pkg = ref<PersonaPackage | null>(null)
const pkgKind = ref<'jd' | 'resume' | null>(null)
const applying = ref(false)
const applyNote = ref('')

const busy = computed(() => generating.value !== null || applying.value)

// tags 用逗号分隔的输入框双向绑定到数组
const tagsText = computed({
  get: () => (pkg.value?.tags ?? []).join(', '),
  set: (v: string) => {
    if (pkg.value) {
      pkg.value.tags = v.split(',').map(s => s.trim()).filter(Boolean)
    }
  },
})

// —— 覆盖报告展示（生成时的对账结果，编辑不改写它） ——
const coverage = computed(() => pkg.value?.coverage ?? null)

// 完整 = 无缺失且无整段缺口（与后端 CoverageReport::is_complete 同义）。
const coverageComplete = computed(
  () => !!coverage.value && coverage.value.missing === 0 && coverage.value.segment_gaps.length === 0,
)

const coverageRatePct = computed(() => {
  if (!coverage.value) return ''
  return `${Math.round(coverage.value.coverage_rate * 100)}%`
})

// 只列需要人看的问题条目（missing 硬缺口 + suspect 软疑点）。
const coverageProblems = computed(() => {
  if (!coverage.value) return []
  return coverage.value.entries.filter(e => e.status === 'missing' || e.status === 'suspect')
})

const coverageStatusText: Record<string, string> = {
  missing: '缺失',
  suspect: '疑点',
  skipped: '跳过',
  covered: '已覆盖',
}

async function generate(kind: 'jd' | 'resume') {
  const text = kind === 'jd' ? jdText.value : resumeText.value
  if (!text.trim()) {
    toast.error(kind === 'jd' ? '请先粘贴 JD 内容' : '请先粘贴简历内容')
    return
  }
  generating.value = kind
  applyNote.value = ''
  try {
    const data: any = await request('cluster', 'persona_generate', { kind, text })
    pkg.value = data as PersonaPackage
    pkgKind.value = kind
    toast.success('人格生成完成，请检查后应用')
  } catch (e: any) {
    toast.error('生成失败: ' + (e?.message || e))
  } finally {
    generating.value = null
  }
}

async function apply() {
  if (!pkg.value) return
  applying.value = true
  applyNote.value = ''
  try {
    const data: any = await request('cluster', 'persona_apply', pkg.value)
    applyNote.value = data?.note || ''
    toast.success(
      data?.reloaded
        ? `已应用为「${data.display_name}」并重载集群`
        : `已应用为「${data.display_name}」`,
    )
  } catch (e: any) {
    toast.error('应用失败: ' + (e?.message || e))
  } finally {
    applying.value = false
  }
}

function clearPreview() {
  pkg.value = null
  pkgKind.value = null
  applyNote.value = ''
}
</script>

<template>
  <div class="persona-gen">
    <!-- 说明 -->
    <div class="card">
      <div class="card-header"><h3>集群人格生成</h3></div>
      <div class="card-body">
        <p class="text-muted text-sm" style="margin:0">
          左侧粘贴 JD、右侧粘贴简历，点对应按钮生成集群节点人格；下方预览可编辑，确认后应用到本节点
         （写入 cluster/IDENTITY.md + SOUL.md，更新节点身份，并重载集群）。生成通常需要 10-30 秒。
        </p>
      </div>
    </div>

    <!-- 双区：左 JD / 右 简历 -->
    <div class="pg-dual">
      <!-- JD 区 -->
      <section class="card pg-pane">
        <div class="card-header"><h3>JD 岗位描述</h3></div>
        <div class="card-body pg-pane-body">
          <textarea
            class="form-textarea pg-editor"
            v-model="jdText"
            :disabled="busy"
            placeholder="粘贴 JD 全文（任意格式）…"
          ></textarea>
          <button
            type="button"
            class="btn btn-primary pg-btn"
            :disabled="busy || !jdText.trim()"
            @click="generate('jd')"
          >
            {{ generating === 'jd' ? '生成中…' : '✨ 用此 JD 生成' }}
          </button>
        </div>
      </section>

      <!-- 简历区 -->
      <section class="card pg-pane">
        <div class="card-header"><h3>简历</h3></div>
        <div class="card-body pg-pane-body">
          <textarea
            class="form-textarea pg-editor"
            v-model="resumeText"
            :disabled="busy"
            placeholder="粘贴简历全文（任意格式）…"
          ></textarea>
          <button
            type="button"
            class="btn btn-primary pg-btn"
            :disabled="busy || !resumeText.trim()"
            @click="generate('resume')"
          >
            {{ generating === 'resume' ? '生成中…' : '✨ 用此简历生成' }}
          </button>
        </div>
      </section>
    </div>

    <!-- 生成结果预览（可编辑） -->
    <section v-if="pkg" class="card">
      <div class="card-header">
        <h3>生成结果 <span class="pg-kind-tag">{{ pkgKind === 'resume' ? '来自简历' : '来自 JD' }}</span></h3>
        <button type="button" class="btn btn-sm" :disabled="applying" @click="clearPreview">清除</button>
      </div>
      <div class="card-body pg-preview-body">
        <!-- 身份字段 -->
        <div class="pg-fields">
          <label class="pg-field">
            <span>显示名 / 节点名</span>
            <input class="form-input" v-model="pkg.display_name" :disabled="applying" />
          </label>
          <label class="pg-field pg-field-sm">
            <span>Emoji</span>
            <input class="form-input" v-model="pkg.emoji" :disabled="applying" />
          </label>
          <label class="pg-field">
            <span>分类</span>
            <input class="form-input" v-model="pkg.category" :disabled="applying" />
          </label>
          <label class="pg-field pg-field-sm">
            <span>集群角色</span>
            <select class="form-input" v-model="pkg.role" :disabled="applying">
              <option value="worker">worker</option>
              <option value="manager">manager</option>
            </select>
          </label>
          <label class="pg-field pg-field-wide">
            <span>标签（逗号分隔）</span>
            <input class="form-input" v-model="tagsText" :disabled="applying" />
          </label>
          <label class="pg-field pg-field-wide">
            <span>node_name（英文标识）</span>
            <input class="form-input" v-model="pkg.node_name" :disabled="applying" />
          </label>
        </div>

        <!-- 人格文件编辑 -->
        <div class="pg-files">
          <div class="pg-file">
            <div class="pg-file-label">IDENTITY.md</div>
            <textarea class="form-textarea pg-md" v-model="pkg.identity_md" :disabled="applying"></textarea>
          </div>
          <div class="pg-file">
            <div class="pg-file-label">SOUL.md</div>
            <textarea class="form-textarea pg-md" v-model="pkg.soul_md" :disabled="applying"></textarea>
          </div>
        </div>

        <!-- 完整性覆盖报告（生成时对账结果） -->
        <div v-if="coverage" class="pg-coverage" data-testid="coverage-panel">
          <div class="pg-coverage-head">
            <span class="pg-coverage-title">完整性覆盖</span>
            <span class="pg-coverage-rate">{{ coverageRatePct }}</span>
            <span class="pg-coverage-badge" :class="coverageComplete ? 'is-ok' : 'is-gap'">
              {{ coverageComplete ? '✓ 完整' : '⚠ 有缺口' }}
            </span>
          </div>
          <div class="pg-coverage-counts">
            <span>信息单元 {{ coverage.total }}</span>
            <span>已覆盖 {{ coverage.covered }}</span>
            <span>跳过 {{ coverage.skipped }}</span>
            <span class="pg-coverage-bad-count" :class="{ 'is-nonzero': coverage.missing > 0 }">缺失 {{ coverage.missing }}</span>
            <span class="pg-coverage-bad-count" :class="{ 'is-nonzero': coverage.suspect > 0 }">疑点 {{ coverage.suspect }}</span>
          </div>
          <ul v-if="coverage.segment_gaps.length" class="pg-coverage-gaps">
            <li v-for="gap in coverage.segment_gaps" :key="gap">⚠ 整段未产出：{{ gap }}</li>
          </ul>
          <table v-if="coverageProblems.length" class="pg-coverage-table">
            <thead>
              <tr><th>单元</th><th>状态</th><th>说明</th></tr>
            </thead>
            <tbody>
              <tr v-for="e in coverageProblems" :key="e.unit_id + e.status">
                <td class="pg-mono">{{ e.unit_id }}</td>
                <td>{{ coverageStatusText[e.status] || e.status }}</td>
                <td>{{ e.reason || e.location || '—' }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- 应用 -->
        <div class="pg-apply">
          <button type="button" class="btn btn-primary" :disabled="applying" @click="apply">
            {{ applying ? '应用中…' : '⬇ 应用到本节点' }}
          </button>
          <span v-if="applyNote" class="pg-note text-muted text-sm">{{ applyNote }}</span>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
.persona-gen {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.pg-dual {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-3);
}

.pg-pane-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.pg-editor {
  width: 100%;
  min-height: 280px;
  padding: var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface-alt);
  color: var(--text);
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.6;
  resize: vertical;
  outline: none;
  tab-size: 2;
}

.pg-editor:focus {
  border-color: var(--accent);
}

.pg-btn {
  width: 100%;
}

/* preview */
.pg-preview-body {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.pg-kind-tag {
  font-size: var(--text-xs);
  font-weight: 400;
  color: var(--text-muted);
  margin-left: var(--space-2);
}

.pg-fields {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: var(--space-3);
}

.pg-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  font-size: var(--text-sm);
}

.pg-field > span {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.pg-field-sm {
  max-width: 120px;
}

.pg-field-wide {
  grid-column: 1 / -1;
}

.pg-files {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-3);
}

.pg-file-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-1);
  font-family: var(--font-mono);
}

.pg-md {
  width: 100%;
  min-height: 240px;
  padding: var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface-alt);
  color: var(--text);
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.6;
  resize: vertical;
  outline: none;
}

.pg-md:focus {
  border-color: var(--accent);
}

.pg-apply {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding-top: var(--space-2);
  border-top: 1px solid var(--border-light);
}

/* coverage */
.pg-coverage {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.pg-coverage-head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.pg-coverage-title {
  font-size: var(--text-sm);
  font-weight: 600;
}

.pg-coverage-rate {
  font-size: var(--text-lg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.pg-coverage-badge {
  font-size: var(--text-xs);
  padding: 2px var(--space-2);
  border-radius: var(--radius-full, 999px);
}

.pg-coverage-badge.is-ok {
  color: var(--success, #2e9e5b);
  background: color-mix(in srgb, var(--success, #2e9e5b) 12%, transparent);
}

.pg-coverage-badge.is-gap {
  color: var(--warning, #c07f00);
  background: color-mix(in srgb, var(--warning, #c07f00) 12%, transparent);
}

.pg-coverage-counts {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-3);
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.pg-coverage-bad-count.is-nonzero {
  color: var(--danger, #d64545);
  font-weight: 600;
}

.pg-coverage-gaps {
  margin: 0;
  padding-left: var(--space-4);
  font-size: var(--text-xs);
  color: var(--warning, #c07f00);
}

.pg-coverage-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-xs);
}

.pg-coverage-table th,
.pg-coverage-table td {
  text-align: left;
  padding: var(--space-1) var(--space-2);
  border-top: 1px solid var(--border-light);
}

.pg-coverage-table th {
  color: var(--text-muted);
  font-weight: 500;
}

.pg-mono {
  font-family: var(--font-mono);
}

.pg-note {
  font-style: italic;
}

@media (max-width: 900px) {
  .pg-dual,
  .pg-files {
    grid-template-columns: 1fr;
  }
}
</style>

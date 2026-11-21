<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

// 危险闸（2026-11-20）：确定性硬闸家族聚合页。审批是给「判断不准」的
// 操作准备的人工裁决，硬闸是给「明确不能干」的形态准备的确定性拦截
// （不经审批、不送 LLM 二审、命中即拒）。本期一个块「工具执行词表」：
// exec 工具内置 42 条 deny 词表元数据化逐条开关（deny_disabled 按 name
// 关条目，增量语义）+ 自定义正则追加（deny_extra，上限 50）。
// exec 工具每次执行前热读 config.json（runtime_snapshot）——保存即时
// 生效，无需重启。拦截落 WARN 日志 + 审计链；拒绝消息带规则名与人话
// 原因 + 本页解锁指引。
// 前瞻：注册表路径硬闸、URL 硬闸等同类确定性硬闸后续入页。
// 旧 custom_deny_patterns 整表替换键是高级出口（不在本页暴露），激活时
// 内置词表被整体替换、内置条目开关不生效（表内无内置条目），追加规则
// 仍有效——横幅诚实注记。

interface DenyEntry {
  name: string
  category: string
  severity: string
  reason: string
  pattern: string
  enabled: boolean
}

const EXTRA_MAX = 50

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)
const entries = ref<DenyEntry[]>([])
const extraDraft = ref<string[]>([])
const disabledDraft = ref<Set<string>>(new Set())
const customReplacementActive = ref(false)
// 服务端快照（保存成功后同步；保存失败回滚到这里）。ref 而非普通 let
// ——dirty computed 依赖快照对比，普通 let 不触发失效（保存后 dirty 卡
// 住的假态）。
const disabledSnapshot = ref<string[]>([])
const extraSnapshot = ref<string[]>([])

const newPattern = ref('')

const CATEGORY_LABELS: Record<string, string> = {
  destructive: '删除/破坏',
  system: '系统控制',
  privilege: '提权',
  exfiltration: '数据外传',
  remote: '远程访问',
  rce: '远程代码执行',
  package: '包管理',
  container: '容器',
  git: 'Git 操作',
  obfuscation: '混淆/不可见执行',
}

const SEVERITY_LABELS: Record<string, string> = {
  high: '高',
  medium: '中',
  low: '低',
}

// 按 category 分组（保持服务端条目序）；未知 category 兜底显示原值。
const groups = computed(() => {
  const out: { key: string; label: string; items: DenyEntry[] }[] = []
  const byKey = new Map<string, DenyEntry[]>()
  for (const e of entries.value) {
    let g = byKey.get(e.category)
    if (!g) {
      g = []
      byKey.set(e.category, g)
      out.push({ key: e.category, label: CATEGORY_LABELS[e.category] || e.category, items: g })
    }
    g.push(e)
  }
  return out
})

const dirty = computed(() => {
  if (extraDraft.value.length !== extraSnapshot.value.length) return true
  if (extraDraft.value.some((p, i) => p !== extraSnapshot.value[i])) return true
  const snap = new Set(disabledSnapshot.value)
  if (snap.size !== disabledDraft.value.size) return true
  for (const n of disabledDraft.value) if (!snap.has(n)) return true
  return false
})

function isEnabled(name: string): boolean {
  return !disabledDraft.value.has(name)
}

function toggleRule(name: string) {
  const next = new Set(disabledDraft.value)
  if (next.has(name)) next.delete(name)
  else next.add(name)
  disabledDraft.value = next
}

function addExtra() {
  const p = newPattern.value.trim()
  if (!p) return
  if (extraDraft.value.includes(p)) {
    toast.error('该规则已存在')
    return
  }
  if (extraDraft.value.length >= EXTRA_MAX) {
    toast.error(`自定义规则最多 ${EXTRA_MAX} 条`)
    return
  }
  extraDraft.value = [...extraDraft.value, p]
  newPattern.value = ''
}

function removeExtra(idx: number) {
  extraDraft.value = extraDraft.value.filter((_, i) => i !== idx)
}

function rollback() {
  disabledDraft.value = new Set(disabledSnapshot.value)
  extraDraft.value = [...extraSnapshot.value]
}

async function save() {
  saving.value = true
  try {
    await request('security', 'exec_deny_set', {
      disabled: [...disabledDraft.value],
      extra: extraDraft.value,
    })
    disabledSnapshot.value = [...disabledDraft.value]
    extraSnapshot.value = [...extraDraft.value]
    toast.success('已保存，下次工具执行即生效')
  } catch (e: any) {
    rollback()
    toast.error('保存失败: ' + e)
  } finally {
    saving.value = false
  }
}

onMounted(async () => {
  try {
    const data = await request('security', 'exec_deny_list', {})
    entries.value = data?.entries || []
    extraSnapshot.value = data?.extra || []
    extraDraft.value = [...extraSnapshot.value]
    disabledSnapshot.value = (data?.entries || [])
      .filter((e: DenyEntry) => !e.enabled)
      .map((e: DenyEntry) => e.name)
    disabledDraft.value = new Set(disabledSnapshot.value)
    customReplacementActive.value = !!data?.custom_replacement_active
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div class="page-hard-gates">
    <SecurityLayerHeader
      num="⑬"
      title="危险闸"
      duty="确定性硬闸家族聚合页：命中即拒、不经审批、不送语义二审——审批是给「判断不准」的操作准备的人工裁决，硬闸是给「明确不能干」的形态准备的确定性拦截。本期：工具执行词表（exec 命令内容级词表）；注册表路径硬闸、URL 硬闸等同类硬闸后续入页。"
      :status-label="`${entries.length - disabledDraft.size}/${entries.length} 条生效`"
      status-tone="on"
      effective="live"
    />
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- 高级出口激活注记：整表替换时内置开关不生效（诚实降级显示） -->
        <div v-if="customReplacementActive" class="card replacement-banner" style="margin-bottom: var(--space-4);" data-test="custom-replacement-banner">
          <div class="card-body">
            <div class="hint" style="margin: 0;">
              ⚠ 高级出口已激活：config.json 的 <code>tools.exec.custom_deny_patterns</code> 整表替换正在生效——内置词表已被自定义词表整体替换，下方内置条目开关<strong>不参与生效表</strong>；追加规则仍然有效。该出口不在本页暴露，如需恢复内置词表请清空该配置键。
            </div>
          </div>
        </div>

        <!-- 工具执行词表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>工具执行词表</h3>
            <span v-if="dirty" class="badge badge-warning" style="margin-left: var(--space-2);" data-test="dirty-badge">未保存修改</span>
          </div>
          <div class="card-body">
            <div class="hint" style="margin-top: 0;">
              exec 类工具执行前的内容级词表：命中任一规则即拒绝执行（拒绝消息带规则名与原因，可在对话里直接看到）。拦截同时落 WARN 日志与审计链。关闭某条 = 放开该形态（其余防线照常）；关闭立即生效，无需重启。
            </div>

            <div v-for="g in groups" :key="g.key" class="deny-group">
              <div class="deny-group-title">
                {{ g.label }}
                <span class="deny-group-count">{{ g.items.length }} 条</span>
              </div>
              <div class="table-wrap">
                <table :data-test="`deny-table-${g.key}`">
                  <thead>
                    <tr><th style="width: 52px;">开关</th><th style="width: 180px;">规则</th><th style="width: 64px;">级别</th><th>说明</th><th style="width: 260px;">匹配形态</th></tr>
                  </thead>
                  <tbody>
                    <tr v-for="e in g.items" :key="e.name" :class="{ 'rule-off': !isEnabled(e.name) }">
                      <td>
                        <div
                          class="toggle"
                          :class="{ active: isEnabled(e.name) }"
                          :data-test="`rule-toggle-${e.name}`"
                          :title="isEnabled(e.name) ? '点击关闭该规则' : '点击开启该规则'"
                          style="cursor: pointer;"
                          @click="toggleRule(e.name)"
                        ></div>
                      </td>
                      <td><code>{{ e.name }}</code></td>
                      <td>
                        <span class="layer-status" :class="'sev-' + e.severity">{{ SEVERITY_LABELS[e.severity] || e.severity }}</span>
                      </td>
                      <td class="rule-reason">{{ e.reason }}</td>
                      <td><code class="rule-pattern">{{ e.pattern }}</code></td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>

            <!-- 自定义追加规则 -->
            <div class="deny-group" data-test="extra-block">
              <div class="deny-group-title">
                自定义追加规则
                <span class="deny-group-count">{{ extraDraft.length }}/{{ EXTRA_MAX }} 条</span>
              </div>
              <div class="hint" style="margin-top: 0;">
                在内置词表之上追加自定义正则，匹配执行命令全文（命令统一按小写比对——规则里的字面量请用小写形态，或加 <code>(?i)</code> 前缀忽略大小写）。写错的正则保存时会被拒绝并指出条目序号，不会半保存。
              </div>
              <div class="extra-add-row">
                <input
                  v-model="newPattern"
                  class="form-input"
                  style="flex: 1; min-width: 240px; font-family: var(--font-mono, monospace);"
                  placeholder="正则表达式，如 forbidden\.exe"
                  data-test="extra-input"
                  @keyup.enter="addExtra"
                />
                <button class="btn" data-test="extra-add" @click="addExtra">添加</button>
              </div>
              <div v-if="extraDraft.length === 0" class="empty-state" style="padding: var(--space-3);">
                <p style="margin: 0;">暂无自定义规则。</p>
              </div>
              <div v-else class="extra-list" data-test="extra-list">
                <div v-for="(p, idx) in extraDraft" :key="p" class="extra-row">
                  <code class="rule-pattern">{{ p }}</code>
                  <button
                    class="btn btn-sm btn-danger"
                    :data-test="`extra-remove-${idx}`"
                    title="删除该规则"
                    @click="removeExtra(idx)"
                  >✕</button>
                </div>
              </div>
            </div>

            <!-- 保存 -->
            <div class="save-row">
              <button
                class="btn btn-primary"
                data-test="save-btn"
                :disabled="!dirty || saving"
                @click="save"
              >{{ saving ? '保存中…' : '保存' }}</button>
              <span v-if="dirty" class="hint" style="margin: 0;">有未保存修改——保存后立即生效（exec 每次执行前热读配置）。</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.hint {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  margin-top: var(--space-1);
  max-width: 720px;
}
.deny-group {
  margin-top: var(--space-4);
}
.deny-group-title {
  font-size: var(--text-sm);
  font-weight: 600;
  margin-bottom: var(--space-2);
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
}
.deny-group-count {
  font-size: var(--text-xs);
  font-weight: 400;
  color: var(--text-muted, #888);
}
tr.rule-off .rule-reason,
tr.rule-off code {
  opacity: 0.45;
}
.rule-reason {
  font-size: var(--text-xs);
}
.rule-pattern {
  font-size: var(--text-xs);
  word-break: break-all;
  display: inline-block;
  max-width: 250px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}
.layer-status {
  font-size: var(--text-xs);
  padding: 1px 10px;
  border-radius: 999px;
  border: 1px solid currentColor;
  white-space: nowrap;
}
.layer-status.sev-high {
  color: var(--error, #cf222e);
  background: color-mix(in srgb, var(--error, #cf222e) 12%, transparent);
}
.layer-status.sev-medium {
  color: var(--warning, #e5a00d);
  background: color-mix(in srgb, var(--warning, #e5a00d) 12%, transparent);
}
.layer-status.sev-low {
  color: var(--text-muted, #888);
  background: color-mix(in srgb, var(--text-muted, #888) 12%, transparent);
}
.extra-add-row {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-2);
  align-items: center;
}
.extra-list {
  margin-top: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.extra-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-md, 6px);
}
.save-row {
  margin-top: var(--space-4);
  display: flex;
  align-items: center;
  gap: var(--space-3);
}
</style>

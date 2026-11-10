<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

// ③ ABAC 访问控制专页（十层页面化 2026-11-10）：六族规则表行级编辑
// （file/dir/process/network/hardware/registry × 各自子类目，typed
// SecurityConfig 载体整体写回）+ Full Access 只读呈现（总览 editor 快照
// 同源——操作面在设置页【编辑器】TAB）+ 会话级放行说明（操作面在
// ChatPanel/SettingsView，本页只说明机制）。
//
// 规则形状 = {pattern, action, comment}（SecurityRule；comment 不可丢——
// CFG-01 同族教训：typed round-trip 缺字段即抹）。action 归一值集
// allow/ask/deny（auditor action_bucket 兼容别名，写入统一规范形）。

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)
const config = ref<any>({})

// 族/子类目注册表（与 nemesis-config FileSecurityRules 等六族 typed 结构
// 逐键对齐；dir 的 serde 别名 directory_rules 在读取侧由 serde 兜住）。
const FAMILIES = [
  {
    key: 'file_rules',
    label: '文件',
    buckets: [
      { key: 'read', label: 'read · 读取' },
      { key: 'write', label: 'write · 写入' },
      { key: 'delete', label: 'delete · 删除' },
    ],
  },
  {
    key: 'dir_rules',
    label: '目录',
    buckets: [
      { key: 'read', label: 'read · 列目录' },
      { key: 'create', label: 'create · 创建' },
      { key: 'delete', label: 'delete · 删除' },
    ],
  },
  {
    key: 'process_rules',
    label: '进程',
    buckets: [
      { key: 'exec', label: 'exec · 执行命令' },
      { key: 'spawn', label: 'spawn · 派生进程' },
      { key: 'kill', label: 'kill · 终止' },
      { key: 'suspend', label: 'suspend · 挂起' },
    ],
  },
  {
    key: 'network_rules',
    label: '网络',
    buckets: [
      { key: 'request', label: 'request · 请求' },
      { key: 'download', label: 'download · 下载' },
      { key: 'upload', label: 'upload · 上传' },
    ],
  },
  {
    key: 'hardware_rules',
    label: '硬件',
    buckets: [
      { key: 'i2c', label: 'i2c · I2C 总线' },
      { key: 'spi', label: 'spi · SPI 总线' },
      { key: 'gpio', label: 'gpio · GPIO 引脚' },
    ],
  },
  {
    key: 'registry_rules',
    label: '注册表',
    buckets: [
      { key: 'read', label: 'read · 读键值' },
      { key: 'write', label: 'write · 写键值' },
      { key: 'delete', label: 'delete · 删键' },
    ],
  },
]

const ACTIONS = [
  { value: 'allow', label: 'allow · 放行' },
  { value: 'ask', label: 'ask · 送审批卡' },
  { value: 'deny', label: 'deny · 硬拦' },
]

const familyKey = ref('file_rules')
const bucketKey = ref('read')

const family = computed(() => FAMILIES.find((f) => f.key === familyKey.value) ?? FAMILIES[0])
const bucket = computed(() => family.value.buckets.find((b) => b.key === bucketKey.value) ?? family.value.buckets[0])

// 归一 action（auditor 兼容 allow/allowed、ask/require_approval 等别名——
// 编辑器展示/写回统一规范形，配置不再积累别名形态）。
function normAction(a: unknown): string {
  const s = String(a ?? '').toLowerCase()
  if (['allow', 'allowed'].includes(s)) return 'allow'
  if (['ask', 'require_approval', 'approval', 'pending'].includes(s)) return 'ask'
  if (['deny', 'denied'].includes(s)) return 'deny'
  return ''
}

const rules = computed<any[]>(() => {
  const fam = config.value?.[familyKey.value]
  const arr = fam?.[bucketKey.value]
  return Array.isArray(arr) ? arr : []
})

function bucketRuleCount(famKey: string, bKey: string): number {
  const arr = config.value?.[famKey]?.[bKey]
  return Array.isArray(arr) ? arr.length : 0
}

function totalRuleCount(): number {
  let n = 0
  for (const f of FAMILIES) for (const b of f.buckets) n += bucketRuleCount(f.key, b.key)
  return n
}

function selectBucket(famKey: string, bKey: string) {
  familyKey.value = famKey
  bucketKey.value = bKey
}

function addRule() {
  const fam = config.value[familyKey.value] || (config.value[familyKey.value] = {})
  if (!Array.isArray(fam[bucketKey.value])) fam[bucketKey.value] = []
  fam[bucketKey.value].push({ pattern: '', action: 'deny', comment: '' })
}

/** 直接改 config 载体数组（computed rules 只是投影——在投影上 splice
 * 改的是临时副本，删行/移位必须落回真相源）。 */
function bucketArr(): any[] {
  const fam = config.value[familyKey.value] || (config.value[familyKey.value] = {})
  if (!Array.isArray(fam[bucketKey.value])) fam[bucketKey.value] = []
  return fam[bucketKey.value]
}

function removeRule(i: number) {
  bucketArr().splice(i, 1)
}

function moveRule(i: number, delta: number) {
  const arr = bucketArr()
  const j = i + delta
  if (j < 0 || j >= arr.length) return
  const [r] = arr.splice(i, 1)
  arr.splice(j, 0, r)
}

// 规则匹配按声明序首中即决（auditor for want in ["deny","ask"] 优先扫描 +
// allow 桶次之）——顺序有意义，提供上移/下移。
async function save() {
  saving.value = true
  try {
    await request('security', 'config.save', config.value)
    toast.success('已保存（重启网关生效）')
  } catch (e: any) {
    toast.error('保存失败: ' + e)
    await loadConfig()
  } finally {
    saving.value = false
  }
}

// Full Access 只读（security.overview editor 快照同源；操作面在设置页）。
const editorState = ref<{ injected: boolean; full_access: boolean; external_write: boolean }>({
  injected: false,
  full_access: false,
  external_write: false,
})

async function loadConfig() {
  try {
    config.value = (await request('security', 'config.get')) || {}
    normalizeActions()
  } catch { /* ignore */ }
}

/** 装载时把六族规则的 action 归一到 allow/ask/deny（auditor 兼容别名
 * 形态不进编辑器 select，保存后配置收敛到规范形）。 */
function normalizeActions() {
  for (const f of FAMILIES) {
    const fam = config.value?.[f.key]
    if (!fam || typeof fam !== 'object') continue
    for (const b of f.buckets) {
      const arr = fam[b.key]
      if (!Array.isArray(arr)) continue
      for (const r of arr) r.action = normAction(r.action) || 'deny'
    }
  }
}

async function loadOverview() {
  try {
    const o = await request('security', 'overview')
    const e = o?.editor
    if (e) editorState.value = e
  } catch { /* ignore */ }
}

onMounted(async () => {
  await Promise.all([loadConfig(), loadOverview()])
  loading.value = false
})
</script>

<template>
  <div class="page-security-abac">
    <SecurityLayerHeader
      num="③"
      title="ABAC 访问控制"
      duty="基于属性的访问控制策略引擎：按「族 × 子类目」声明 pattern 规则（支持通配），命中按动作放行/送审批/硬拦；匹配按声明序首中即决，未命中规则走默认姿态。随安全总闸常驻（无独立层开关）。"
      status-label="常驻"
      status-tone="on"
      effective="restart"
    />
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- 六族规则表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>规则表 <span class="hint-inline">共 {{ totalRuleCount() }} 条 · 默认姿态 {{ config.default_action || 'deny' }}</span></h3>
            <button class="btn btn-sm btn-primary" data-test="abac-save" :disabled="saving" @click="save">
              {{ saving ? '保存中…' : '保存' }}
            </button>
          </div>
          <div class="card-body" style="padding: 0;">
            <div class="abac-tabs" data-test="abac-family-tabs">
              <template v-for="f in FAMILIES" :key="f.key">
                <button
                  v-for="b in f.buckets"
                  :key="f.key + '.' + b.key"
                  class="abac-tab"
                  :class="{ active: familyKey === f.key && bucketKey === b.key }"
                  :data-test="'abac-tab-' + f.key + '-' + b.key"
                  @click="selectBucket(f.key, b.key)"
                >{{ f.label }} · {{ b.label.split(' ')[0] }} <span class="abac-count">{{ bucketRuleCount(f.key, b.key) }}</span></button>
              </template>
            </div>
            <div style="padding: var(--space-3);">
              <div class="hint" style="margin: 0 0 var(--space-2);">
                {{ family.label }} · {{ bucket.label }}。pattern 支持通配（如 <code>*</code>、<code>C:\Projects\**</code>）；action：allow 放行 / ask 弹审批卡 / deny 硬拦。保存前改动只在本地，整档写回。
              </div>
              <table v-if="rules.length" class="abac-table" data-test="abac-rules-table">
                <thead><tr><th style="width: 34%;">pattern</th><th style="width: 170px;">action</th><th>comment</th><th style="width: 130px;"></th></tr></thead>
                <tbody>
                  <tr v-for="(r, i) in rules" :key="i" :data-test="'abac-rule-row-' + i">
                    <td><input v-model="r.pattern" class="form-input mono" placeholder="路径 / 命令 / 域名 pattern" /></td>
                    <td>
                      <select v-model="r.action" class="form-select" style="width: 100%;">
                        <option v-for="a in ACTIONS" :key="a.value" :value="a.value">{{ a.label }}</option>
                      </select>
                    </td>
                    <td><input v-model="r.comment" class="form-input" placeholder="备注（可选）" /></td>
                    <td style="white-space: nowrap;">
                      <button class="btn btn-sm" title="上移" :disabled="i === 0" @click="moveRule(i, -1)">↑</button>
                      <button class="btn btn-sm" title="下移" :disabled="i === rules.length - 1" @click="moveRule(i, 1)">↓</button>
                      <button class="btn btn-sm danger" data-test="abac-rule-remove" @click="removeRule(i)">删</button>
                    </td>
                  </tr>
                </tbody>
              </table>
              <div v-else class="empty-state" style="padding: var(--space-4);">
                <p style="margin: 0;">此子类目暂无规则（未命中规则走默认姿态 {{ config.default_action || 'deny' }}）。</p>
              </div>
              <button class="btn btn-sm" data-test="abac-rule-add" style="margin-top: var(--space-2);" @click="addRule">+ 加规则</button>
            </div>
          </div>
        </div>

        <!-- Full Access 只读呈现 -->
        <div class="card" style="margin-bottom: var(--space-4);" data-test="editor-flags-card">
          <div class="card-header"><h3>编辑器 Full Access</h3></div>
          <div class="card-body">
            <div class="state-row">
              <span class="layer-status" :class="editorState.injected ? (editorState.full_access ? 'tone-warn' : 'tone-on') : 'tone-na'">
                {{ editorState.injected ? (editorState.full_access ? '已开启（工具面解锁）' : '关闭（工作区围栏）') : '未装配' }}
              </span>
              <span class="state-note">外部写删：{{ editorState.injected ? (editorState.external_write ? '允许' : '拒绝') : '--' }}</span>
              <router-link class="state-link" to="/settings">设置页【编辑器】TAB 操作 →</router-link>
            </div>
            <div class="hint" style="margin-top: var(--space-1);">
              Full Access 开启时 ABAC 文件/目录围栏对编辑器路径放行面同步放宽——安全审计照常落账。
            </div>
          </div>
        </div>

        <!-- 会话级放行说明（操作面在 ChatPanel/SettingsView） -->
        <div class="card" data-test="session-allow-card">
          <div class="card-header"><h3>会话级全部放行</h3></div>
          <div class="card-body">
            <div class="hint" style="margin: 0;">
              会话作用域的审批自动放行：开启后该会话内<strong>所有</strong>安全审批卡（含 CRITICAL）自动批准（审计归因 session_allow_all）；硬 deny / 自毁硬拦 / estop / fail-closed 不在射程。开关在聊天面板第三按钮（会话菜单），跨重启按会话 sidecar 保持。本页只说明机制，无会话上下文不提供切换。
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
}
.hint-inline {
  font-size: var(--text-xs);
  font-weight: 400;
  color: var(--text-muted, #888);
  margin-left: var(--space-2);
}
.mono {
  font-family: var(--font-mono, 'Consolas', monospace);
  font-size: var(--text-xs);
}
.layer-status {
  font-size: var(--text-xs);
  padding: 1px 10px;
  border-radius: 999px;
  border: 1px solid currentColor;
  white-space: nowrap;
}
.layer-status.tone-on {
  color: var(--success, #2da44e);
  background: color-mix(in srgb, var(--success, #2da44e) 12%, transparent);
}
.layer-status.tone-off {
  color: var(--error, #cf222e);
  background: color-mix(in srgb, var(--error, #cf222e) 12%, transparent);
}
.layer-status.tone-warn {
  color: var(--warning, #e5a00d);
  background: color-mix(in srgb, var(--warning, #e5a00d) 12%, transparent);
}
.layer-status.tone-na {
  color: var(--text-muted, #888);
  background: var(--bg-elev, rgba(128, 128, 128, 0.12));
}
.abac-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border, rgba(128, 128, 128, 0.2));
}
.abac-tab {
  border: 1px solid transparent;
  background: none;
  padding: 3px 10px;
  border-radius: 999px;
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  cursor: pointer;
}
.abac-tab.active {
  color: var(--text, #ddd);
  border-color: var(--border, rgba(128, 128, 128, 0.4));
  background: var(--bg-elev, rgba(128, 128, 128, 0.1));
}
.abac-count {
  margin-left: 4px;
  opacity: 0.7;
}
.abac-table {
  width: 100%;
}
.state-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.state-note {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
}
.state-link {
  font-size: var(--text-xs);
}
</style>

<script setup lang="ts">
/**
 * 凭据 Vault 管理页（L7c，2026-10-13）——放【配置】组、安全之下。
 *
 * 操作面：列表（vault.list）/ 新增（vault.set）/ 删除（vault.remove）/
 * 解锁测试（vault.unlock）。后端语义由 handlers/vault/{vault.rs,tests.rs}
 * 钉住；本页**永不显示 secret 值**（后端 list/status 本就不回值，新增表单
 * 用 password 输入，解锁测试口令只在本次请求内使用）。
 *
 * 覆盖（轮换）纪律：vault.set 对已存在别名会拒绝并提示 force:true——
 * 前端弹 confirm 后带 force 重发（镜像 CLI --force 纪律）。
 *
 * 凭据免打扰 + 统计 + 域绑定（2026-11-23 ABCD 批）：
 * - 凭据免打扰开关（vault.credential_status / credential_set）：dispatch
 *   凭据闸（credential_use 卡）的独立自动放行；全局免打扰（人工审批页）
 *   开启时凭据卡同样免卡（∨ 语义，后端闸侧裁决）。配置持久化，重启保持。
 * - 使用统计（vault.credential_stats）：本次运行凭据引用使用记录
 *   （方向=目标摘要 / 用途=工具+槽位+别名 / 次数=三主臂计数）——重启清零。
 * - 域绑定管理（vault.bindings / bind / unbind）：别名 → (工具,槽位)
 *   白名单，绑定即预授权（域内免卡直过；出域硬拒，任何开关压不掉）。
 */

import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

const toast = useToast()
const { request } = useWSAPI()

interface VaultEntry {
  alias: string
  domain: string
  description: string
  created_at: string | null
  rotated_at: string | null
}

interface VaultStatus {
  exists: boolean
  mode?: string
  unlocked?: boolean
  alias_count?: number
  path?: string
}

const loading = ref(false)
const status = ref<VaultStatus | null>(null)
const entries = ref<VaultEntry[]>([])

// 新增表单（value 用 password 输入，永不回显）。
const addAlias = ref('')
const addValue = ref('')
const addDomain = ref('')
const addDescription = ref('')
const adding = ref(false)

// 解锁测试（口令只在本次请求内使用，不存储）。
const unlockPass = ref('')
const unlocking = ref(false)
const lastUnlock = ref<null | { ok: boolean; detail: string }>(null)

// ── 凭据免打扰（ABCD 批 A）─────────────────────────────────────────
const credAutoStatus = ref<{
  injected: boolean
  enabled?: boolean
  auto_count?: number
  preauth_count?: number
  denied_count?: number
}>({ injected: false })
const credAutoSaving = ref(false)

// ── 使用统计（ABCD 批 B）───────────────────────────────────────────
interface CredRecord {
  ts_ms: number
  tool: string
  fields: string
  aliases: string
  target_summary: string
  outcome: string
  source: string
}
const credStats = ref<{
  injected: boolean
  enabled?: boolean
  counters?: { auto: number; preauth: number; denied: number }
  records?: CredRecord[]
}>({ injected: false })

const OUTCOME_LABELS: Record<string, string> = {
  auto: '开关免卡',
  preauth: '域内预授权',
  approved: '人工批准',
  denied: '人工拒绝',
  cooldown: '冷却静默',
  out_of_domain: '出域硬拒',
  channel_error: '通道故障',
}

function fmtTime(tsMs: number): string {
  const d = new Date(tsMs)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

// ── 域绑定管理（ABCD 批 C）─────────────────────────────────────────
interface DomainBinding {
  tool: string
  slot: string
}
const bindings = ref<Record<string, DomainBinding[]>>({})
const bindingsPath = ref('')
const bindForm = ref({ alias: '', tool: '', slot: '' })
const bindingBusy = ref(false)

const bindingAliases = computed(() => Object.keys(bindings.value).sort())

const lockedBanner = computed(() => {
  if (!status.value?.exists) return ''
  if (status.value.unlocked) return ''
  return 'Vault 已锁定（argon2 模式且进程未带口令环境变量）：列表可用，写入/轮换需要解锁。可用下方「解锁测试」验证口令，或重启 gateway 时带上 NEMESISBOT_VAULT_PASSPHRASE。'
})

async function load() {
  loading.value = true
  try {
    const [st, ls, ca, cs, bd] = await Promise.all([
      request('vault', 'status', {}, 8000),
      request('vault', 'list', {}, 8000),
      request('vault', 'credential_status', {}, 8000).catch(() => null),
      request('vault', 'credential_stats', {}, 8000).catch(() => null),
      request('vault', 'bindings', {}, 8000).catch(() => null),
    ])
    status.value = st ?? null
    entries.value = ls?.entries ?? []
    credAutoStatus.value = ca ?? { injected: false }
    credStats.value = cs ?? { injected: false }
    bindings.value = bd?.bindings ?? {}
    bindingsPath.value = bd?.path ?? ''
  } catch (e: any) {
    toast.error('加载凭据信息失败: ' + (e?.message || e))
  }
  loading.value = false
}
onMounted(load)

// 凭据免打扰开关：vault.credential_set 单命令写配置 + 热翻（后端
// update_security_config 同一事务入口；无需先读整档——与审批页
// config.save 全档回写不同，这里只动本键）。
async function onCredAutoToggle(ev: Event) {
  if (credAutoSaving.value) return
  const on = (ev.target as HTMLInputElement).checked
  credAutoSaving.value = true
  try {
    await request('vault', 'credential_set', { enabled: on }, 8000)
    toast.success(on
      ? '凭据免打扰已开启（凭据卡将自动放行；出域硬拒不受影响）'
      : '凭据免打扰已关闭（凭据引用恢复人工审批）')
    const ca = await request('vault', 'credential_status', {}, 8000).catch(() => null)
    if (ca) credAutoStatus.value = ca
  } catch (e: any) {
    ;(ev.target as HTMLInputElement).checked = !on
    toast.error('保存失败: ' + (e?.message || e))
  }
  credAutoSaving.value = false
}

async function addBinding() {
  if (bindingBusy.value) return
  const alias = bindForm.value.alias.trim()
  const tool = bindForm.value.tool.trim()
  const slot = bindForm.value.slot.trim()
  if (!alias || !tool || !slot) {
    toast.error('别名 / 工具 / 槽位均为必填')
    return
  }
  bindingBusy.value = true
  try {
    await request('vault', 'bind', { alias, tool, slot }, 8000)
    toast.success(`已绑定 ${alias} → ${tool}:${slot}`)
    bindForm.value = { alias: '', tool: '', slot: '' }
    const bd = await request('vault', 'bindings', {}, 8000).catch(() => null)
    if (bd) {
      bindings.value = bd.bindings ?? {}
      bindingsPath.value = bd.path ?? ''
    }
  } catch (e: any) {
    toast.error('绑定失败: ' + (e?.message || e))
  }
  bindingBusy.value = false
}

async function removeBinding(alias: string, b: DomainBinding) {
  if (!confirm(`解除绑定 ${alias} → ${b.tool}:${b.slot}？解除后该别名将回落人工审批卡路径。`)) return
  try {
    await request('vault', 'unbind', { alias, tool: b.tool, slot: b.slot }, 8000)
    toast.success(`已解除绑定 ${alias} → ${b.tool}:${b.slot}`)
    const bd = await request('vault', 'bindings', {}, 8000).catch(() => null)
    if (bd) {
      bindings.value = bd.bindings ?? {}
      bindingsPath.value = bd.path ?? ''
    }
  } catch (e: any) {
    toast.error('解除失败: ' + (e?.message || e))
  }
}

async function addEntry(force = false) {
  if (adding.value) return
  const alias = addAlias.value.trim()
  const value = addValue.value
  if (!alias) {
    toast.error('别名必填（如 openai/prod-key）')
    return
  }
  if (!value) {
    toast.error('secret 值必填且不能为空')
    return
  }
  adding.value = true
  try {
    const resp = await request('vault', 'set', {
      alias,
      value,
      domain: addDomain.value.trim(),
      description: addDescription.value.trim(),
      force,
    }, 8000)
    toast.success(resp?.rotated
      ? `「${alias}」已轮换（旧值被覆盖，不可撤销）`
      : `「${alias}」已写入`)
    addAlias.value = ''
    addValue.value = ''
    addDomain.value = ''
    addDescription.value = ''
    await load()
  } catch (e: any) {
    const msg = String(e?.message || e)
    // 覆盖（轮换）需显式确认——后端拒绝文案含 force:true，弹确认后重发。
    if (!force && msg.includes('force:true')) {
      if (confirm(`别名「${alias}」已存在。覆盖（轮换）不可撤销，确定覆盖？`)) {
        adding.value = false
        await addEntry(true)
        return
      }
    } else {
      toast.error('写入失败: ' + msg)
    }
  }
  adding.value = false
}

async function removeEntry(entry: VaultEntry) {
  if (!confirm(`确定删除凭据「${entry.alias}」？此操作不可撤销。`)) return
  try {
    await request('vault', 'remove', { alias: entry.alias }, 8000)
    toast.success(`「${entry.alias}」已删除`)
    await load()
  } catch (e: any) {
    toast.error('删除失败: ' + (e?.message || e))
  }
}

async function runUnlockTest() {
  if (unlocking.value) return
  const pw = unlockPass.value
  if (!pw) {
    toast.error('请先输入要测试的口令')
    return
  }
  unlocking.value = true
  lastUnlock.value = null
  try {
    const resp = await request('vault', 'unlock', { passphrase: pw }, 8000)
    lastUnlock.value = { ok: true, detail: `解锁成功（模式 ${resp?.mode ?? '?'}）` }
    toast.success('解锁测试通过')
    unlockPass.value = ''
    await load()
  } catch (e: any) {
    lastUnlock.value = { ok: false, detail: String(e?.message || e) }
    toast.error('解锁测试失败')
  }
  unlocking.value = false
}
</script>

<template>
  <div class="page-vault">
    <SecurityLayerHeader
      num=""
      title="凭据隔离"
      duty="凭据隔离（Vault）：API key 等凭据加密托管（AES-256-GCM，DPAPI/Argon2id 派生），工具凭据槽注入替代明文入参——凭据不进提示词、不进日志、不进上下文。基础设施面，不占闸门编号（⑬ 为危险闸）。"
    />
    <div class="page-body">
      <div style="display: flex; flex-direction: column; gap: var(--space-4);">

        <!-- 状态卡 -->
        <div class="card">
          <div class="card-header">
            <h3>状态</h3>
            <button class="btn btn-sm" :disabled="loading" @click="load">
              {{ loading ? '刷新中…' : '刷新' }}
            </button>
          </div>
          <div style="padding: var(--space-4);">
            <div v-if="!status || !status.exists" class="muted">
              尚未创建 vault 文件——写入第一条凭据时自动创建。
            </div>
            <template v-else>
              <div style="display: flex; gap: var(--space-3); flex-wrap: wrap; align-items: center;">
                <span class="badge" :class="status.unlocked ? 'badge-success' : 'badge-warning'">
                  {{ status.unlocked ? '已解锁' : '已锁定' }}
                </span>
                <span class="badge badge-info">{{ status.mode }}</span>
                <span class="muted">{{ status.alias_count }} 条凭据</span>
                <span class="muted mono" style="font-size: var(--text-xs);">{{ status.path }}</span>
              </div>
              <p v-if="lockedBanner" class="muted" style="margin-top: var(--space-3); font-size: var(--text-sm);">
                {{ lockedBanner }}
              </p>
            </template>
          </div>
        </div>

        <!-- 凭据免打扰卡（ABCD 批 A） -->
        <div class="card">
          <div class="card-header"><h3>凭据免打扰</h3></div>
          <div style="padding: var(--space-4);">
            <div class="auto-approve-row">
              <label class="auto-approve-row">
                <input
                  type="checkbox"
                  data-test="credential-auto-toggle"
                  :checked="credAutoStatus.enabled ?? false"
                  :disabled="credAutoSaving || !credAutoStatus.injected"
                  @change="onCredAutoToggle"
                />
                <strong>开启凭据引用自动放行（免打扰）</strong>
              </label>
            </div>
            <p v-if="credAutoStatus.injected" class="auto-approve-status">
              <template v-if="credAutoStatus.enabled">✅ 免打扰中：凭据引用将自动放行（本次运行 {{ credAutoStatus.auto_count ?? 0 }} 次免卡）</template>
              <template v-else>未开启：每次凭据引用都会弹审批卡</template>
            </p>
            <p v-else class="muted hint">
              ⚠ 统计载体未注入（本进程未装配凭据闸）——开关不可用。
            </p>
            <p class="muted hint">
              【人工审批页】的「全局免打扰」开启时凭据卡同样免卡（两者任一开启即生效）；
              全局关闭时此开关单独控制凭据卡。出域硬拒（见下方域绑定）不受任何开关影响。
            </p>
          </div>
        </div>

        <!-- 使用统计卡（ABCD 批 B） -->
        <div class="card">
          <div class="card-header">
            <h3>凭据使用统计</h3>
            <button class="btn btn-sm" :disabled="loading" @click="load">刷新</button>
          </div>
          <div style="padding: var(--space-4);">
            <div v-if="!credStats.injected" class="muted">
              统计载体未注入（本进程未装配凭据闸）——无统计数据。
            </div>
            <template v-else>
              <div style="display: flex; gap: var(--space-4); flex-wrap: wrap; margin-bottom: var(--space-3);">
                <span><strong>{{ credStats.counters?.auto ?? 0 }}</strong> <span class="muted">开关免卡</span></span>
                <span><strong>{{ credStats.counters?.preauth ?? 0 }}</strong> <span class="muted">域内预授权</span></span>
                <span><strong>{{ credStats.counters?.denied ?? 0 }}</strong> <span class="muted">拒绝/出域</span></span>
              </div>
              <div v-if="(credStats.records?.length ?? 0) === 0" class="muted">本次运行暂无凭据引用记录。</div>
              <table v-else class="table">
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>工具</th>
                    <th>槽位</th>
                    <th>别名</th>
                    <th>方向（目标摘要）</th>
                    <th>结果</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="(r, i) in credStats.records" :key="i">
                    <td class="mono muted">{{ fmtTime(r.ts_ms) }}</td>
                    <td class="mono">{{ r.tool }}</td>
                    <td class="mono muted">{{ r.fields || '—' }}</td>
                    <td class="mono">{{ r.aliases || '—' }}</td>
                    <td class="muted" :title="r.target_summary">{{ r.target_summary || '—' }}</td>
                    <td>
                      <span class="badge" :class="r.outcome === 'denied' || r.outcome === 'out_of_domain' ? 'badge-error' : (r.outcome === 'approved' ? 'badge-info' : 'badge-success')">
                        {{ OUTCOME_LABELS[r.outcome] ?? r.outcome }}
                      </span>
                    </td>
                  </tr>
                </tbody>
              </table>
              <p class="muted hint">
                仅记别名/槽位/目标摘要等元数据，凭据真值永不落统计或日志；记录环形缓存上限 200 条（旧的先淘汰）；
                统计为本次运行口径，重启清零——持久审计链（logs/security/ 审计事件）另记。
              </p>
            </template>
          </div>
        </div>

        <!-- 域绑定管理卡（ABCD 批 C） -->
        <div class="card">
          <div class="card-header">
            <h3>域绑定（预授权白名单）</h3>
            <span v-if="bindingsPath" class="muted mono" style="font-size: var(--text-xs);">{{ bindingsPath }}</span>
          </div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              绑定「别名 → (工具, 槽位)」后，该别名在该工具该槽位的使用免卡直过（域内预授权）；
              绑定之外的任何使用视为出域，在凭据闸被硬拒——免打扰开关压不掉这一层。
            </p>
            <div v-if="bindingAliases.length === 0" class="muted">暂无绑定。</div>
            <div v-for="alias in bindingAliases" :key="alias" style="display: flex; flex-direction: column; gap: var(--space-1);">
              <span class="mono" style="font-weight: 600;">{{ alias }}</span>
              <div v-for="b in bindings[alias]" :key="b.tool + ':' + b.slot" style="display: flex; gap: var(--space-2); align-items: center; padding-left: var(--space-4);">
                <span class="badge badge-info mono">{{ b.tool }} : {{ b.slot }}</span>
                <button class="btn btn-sm btn-danger" data-test="unbind-btn" @click="removeBinding(alias, b)">解除</button>
              </div>
            </div>
            <div style="display: flex; gap: var(--space-2); flex-wrap: wrap; align-items: center; border-top: 1px solid var(--border-color); padding-top: var(--space-3);">
              <input v-model="bindForm.alias" class="form-input" style="max-width: 220px;" placeholder="别名（如 env.PW104）" list="vault-alias-list" />
              <datalist id="vault-alias-list">
                <option v-for="e in entries" :key="e.alias" :value="e.alias" />
              </datalist>
              <input v-model="bindForm.tool" class="form-input" style="max-width: 180px;" placeholder="工具（如 exec）" />
              <input v-model="bindForm.slot" class="form-input" style="max-width: 180px;" placeholder="槽位（如 env）" />
              <button class="btn btn-sm btn-primary" data-test="bind-btn" :disabled="bindingBusy" @click="addBinding">
                {{ bindingBusy ? '绑定中…' : '添加绑定' }}
              </button>
            </div>
            <p class="muted hint">绑定写 workspace/config/config.vault.json（0o600）；别名须与 vault 中的凭据一致才生效。</p>
          </div>
        </div>

        <!-- 解锁测试卡 -->
        <div class="card">
          <div class="card-header"><h3>解锁测试</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              验证一把口令能否打开 vault（argon2 模式）。口令只在本次请求内使用，
              不存储、不落日志；解锁态不跨请求保留。
            </p>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <input
                v-model="unlockPass"
                class="form-input"
                type="password"
                style="max-width: 320px;"
                placeholder="vault 口令"
                @keyup.enter="runUnlockTest"
              />
              <button class="btn btn-sm btn-primary" :disabled="unlocking" @click="runUnlockTest">
                {{ unlocking ? '测试中…' : '测试解锁' }}
              </button>
            </div>
            <p v-if="lastUnlock" :class="lastUnlock.ok ? '' : 'muted'" style="font-size: var(--text-sm);">
              {{ lastUnlock.ok ? '✓ ' + lastUnlock.detail : '✗ ' + lastUnlock.detail }}
            </p>
          </div>
        </div>

        <!-- 新增卡 -->
        <div class="card">
          <div class="card-header"><h3>新增凭据</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <div style="display: flex; gap: var(--space-2); flex-wrap: wrap;">
              <input v-model="addAlias" class="form-input" style="max-width: 240px;" placeholder="别名（必填，如 openai/prod-key）" />
              <input v-model="addValue" class="form-input" type="password" style="max-width: 320px;" placeholder="secret 值（必填，不回显）" />
              <input v-model="addDomain" class="form-input" style="max-width: 160px;" placeholder="域（可选，如 openai）" />
              <input v-model="addDescription" class="form-input" style="max-width: 240px;" placeholder="备注（可选）" />
              <button class="btn btn-sm btn-primary" :disabled="adding" @click="addEntry()">
                {{ adding ? '写入中…' : '写入' }}
              </button>
            </div>
            <p class="muted" style="font-size: var(--text-sm);">
              agent 侧以 <span class="mono">vault:&lt;别名&gt;</span> 引用（如 web_fetch 的 headers 里
              <span class="mono">{"Authorization": "vault:openai/prod-key"}</span>），
              发往 LLM/日志的只有别名，真实值在工具调度最后一刻注入。
            </p>
          </div>
        </div>

        <!-- 列表卡 -->
        <div class="card">
          <div class="card-header"><h3>凭据列表</h3></div>
          <div style="padding: var(--space-4);">
            <div v-if="entries.length === 0 && !loading" class="muted">
              {{ status?.exists ? 'vault 为空。' : '尚未创建 vault 文件。' }}
            </div>
            <table v-else class="table">
              <thead>
                <tr>
                  <th>别名</th>
                  <th>域</th>
                  <th>备注</th>
                  <th>创建时间</th>
                  <th>轮换时间</th>
                  <th style="width: 100px;">操作</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="entry in entries" :key="entry.alias">
                  <td class="mono">{{ entry.alias }}</td>
                  <td>{{ entry.domain || '—' }}</td>
                  <td>{{ entry.description || '—' }}</td>
                  <td class="muted">{{ entry.created_at || '—' }}</td>
                  <td class="muted">{{ entry.rotated_at || '—' }}</td>
                  <td>
                    <button class="btn btn-sm btn-danger" @click="removeEntry(entry)">删除</button>
                  </td>
                </tr>
              </tbody>
            </table>
            <p class="muted" style="margin-top: var(--space-3); font-size: var(--text-xs);">
              出于安全设计，secret 值永不显示（写入后单向不可读）。
            </p>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
.auto-approve-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  user-select: none;
}
.auto-approve-status {
  margin-top: var(--space-2);
  color: var(--color-success, #2e7d32);
  font-size: var(--text-sm);
}
.hint {
  margin-top: var(--space-2);
  font-size: var(--text-sm);
}
</style>

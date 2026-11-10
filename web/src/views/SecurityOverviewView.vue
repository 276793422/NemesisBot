<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 安全总览页（十层页面化 2026-11-10）：十层状态表（行跳对应页）+ 只读
// 状态区（estop/Plan/Full Access/免打扰——用户拍板 2：总览只显示状态，
// 操作面留侧栏/ChatPanel/审批页）+ 全局总闸（即时生效，编辑走设置页）+
// 审计链校验 + 高级 JSON 编辑器（自旧 SecurityView 迁移，唯一写入口）。
//
// 数据面 = `security.overview` 聚合快照（只读）+ `security.config.get`
// （JSON 编辑器载体）。层开关语义为 raw 投影（缺席=启用），展示不再二次
// 猜测；生效时机除总闸外一律「重启网关生效」。

const { request } = useWSAPI()
const toast = useToast()
const router = useRouter()

const loading = ref(true)
const overview = ref<any>(null)
const config = ref<any>({})
const editing = ref(false)
const editConfig = ref('')

const layers = computed(() => overview.value?.layers ?? {})
const intercepts = computed(() => overview.value?.intercepts ?? { total: 0, denied: 0, by_policy: {} })
const chain = computed(() => overview.value?.chain ?? { present: false, segments: 0 })

// ── 十层行注册表 ─────────────────────────────────────────────────────
interface LayerRow {
  num: string
  title: string
  to: string
  status: () => { label: string; tone: 'on' | 'off' | 'na' | 'warn' }
  summary: () => string
}

const ROWS: LayerRow[] = [
  {
    num: '①',
    title: '注入检测',
    to: '/security/layer/injection',
    status: () => pick(layers.value.injection?.enabled, '已启用', '已关闭'),
    summary: () => `判定阈值 ${layers.value.injection?.threshold ?? 0.7}${layers.value.injection?.threshold_configured ? '' : '（默认）'}`,
  },
  {
    num: '②',
    title: '命令守卫',
    to: '/security/layer/command',
    status: () => pick(layers.value.command_guard?.enabled, '已启用', '已关闭'),
    summary: () => `内置黑名单硬拦危险命令 · 事件 ${policyCount('command_guard')} 条`,
  },
  {
    num: '③',
    title: 'ABAC 访问控制',
    to: '/security/layer/abac',
    status: () => ({ label: '常驻', tone: 'on' }),
    summary: () => '六族规则策略引擎（随总闸，无独立开关）· 事件 ' + policyCount('abac') + ' 条',
  },
  {
    num: '④',
    title: '凭据扫描',
    to: '/security/layer/credential',
    status: () => pick(layers.value.credential?.enabled, '已启用', '已关闭'),
    summary: () => `内置 32 条规则 · vault 引用豁免 · 事件 ${policyCount('credential_scanner')} 条`,
  },
  {
    num: '⑤',
    title: 'DLP 防泄漏',
    to: '/security/layer/dlp',
    status: () => pick(layers.value.dlp?.enabled, '已启用', '已关闭'),
    summary: () => {
      const d = layers.value.dlp ?? {}
      const n = (d.rules ?? []).length
      return `处置 ${d.action || 'block'}（高置信） · 规则${n ? ` ${n} 条（过滤）` : ' 全部 32 条'}`
    },
  },
  {
    num: '⑥',
    title: 'SSRF 防护',
    to: '/security/layer/ssrf',
    status: () => pick(layers.value.ssrf?.enabled, '已启用', '已关闭'),
    summary: () => {
      const n = (layers.value.ssrf?.allowed_hosts ?? []).length
      return `内网/环回目标拦截 · 白名单 ${n} 项`
    },
  },
  {
    num: '⑦',
    title: '病毒扫描',
    to: '/scanner',
    status: () => {
      const s = overview.value?.scanner ?? {}
      if (!s.enabled) return { label: '已关闭', tone: 'off' }
      if ((s.enabled_count ?? 0) > 0 && (s.ready_count ?? 0) === 0) return { label: '未就绪', tone: 'warn' }
      return { label: '已启用', tone: 'on' }
    },
    summary: () => {
      const s = overview.value?.scanner ?? {}
      return `引擎 ${s.total ?? 0} 个 · 启用 ${s.enabled_count ?? 0} · 就绪 ${s.ready_count ?? 0}`
    },
  },
  {
    num: '⑧',
    title: '审计链',
    to: '/security/layer/audit',
    status: () => pick(layers.value.audit?.log_file_enabled, '已启用', '已关闭'),
    summary: () => `日志文件 ${layers.value.audit?.log_file_enabled ? '开' : '关'} · Merkle 链 ${layers.value.audit?.chain_enabled ? `开（${chain.value.segments} 段）` : '关'}`,
  },
  {
    num: '⑨',
    title: '沙盒隔离',
    to: '/sandbox',
    status: () => {
      const e = overview.value?.executor ?? {}
      if (!e.available) return { label: '不可用', tone: 'na' }
      if (e.enabled && e.sandbox) return { label: '已启用', tone: 'on' }
      return { label: '已关闭', tone: 'off' }
    },
    summary: () => {
      const e = overview.value?.executor ?? {}
      return e.enabled ? (e.sandbox ? 'Sandboxie 盒级隔离（Layer 2）' : '子进程剥离（Layer 1，未套盒）') : '执行体隔离未开启（config executor.enabled）'
    },
  },
  {
    num: '⑩',
    title: '虚拟化',
    to: '/vm',
    status: () => {
      const e = overview.value?.executor ?? {}
      if (!e.available) return { label: '不可用', tone: 'na' }
      return e.vm ? { label: '已启用', tone: 'on' } : { label: '已关闭', tone: 'off' }
    },
    summary: () => '桌面级 VM 执行环境（config executor.vm.enabled）',
  },
]

function pick(on: unknown, onLabel: string, offLabel: string): { label: string; tone: 'on' | 'off' } {
  return on ? { label: onLabel, tone: 'on' } : { label: offLabel, tone: 'off' }
}

function policyCount(policy: string): number {
  return intercepts.value.by_policy?.[policy]?.total ?? 0
}

function rowStatus(row: LayerRow) {
  return row.status()
}

// ── 只读状态区（用户拍板 2：总览只显示状态，操作面别处）──────────────
const estop = computed(() => overview.value?.estop ?? { injected: false })
const planMode = computed(() => overview.value?.plan_mode ?? { available: false, mode: null })
const editor = computed(() => overview.value?.editor ?? { injected: false })
const approvals = computed(() => overview.value?.approvals ?? { injected: false })
const signature = computed(() => overview.value?.signature ?? { injected: false })
const globalSwitch = computed(() => overview.value?.global ?? { available: false })

function modeLabel(mode: unknown): string {
  if (mode === 'plan') return 'Plan（只读）'
  if (mode === 'build') return 'Build'
  return String(mode ?? '--')
}

// ── 审计链校验（logs 模块；层页同款按钮）──────────────────────────────
const chainVerifyResult = ref<any>(null)
const chainVerifying = ref(false)

async function verifyChain() {
  chainVerifying.value = true
  chainVerifyResult.value = null
  try {
    chainVerifyResult.value = await request('logs', 'chain_verify')
  } catch (e: any) {
    toast.error('校验失败: ' + e)
  } finally {
    chainVerifying.value = false
  }
}

// ── 高级 JSON 编辑器（旧 SecurityView 迁移；唯一任意键写入口）─────────
async function loadConfig() {
  try {
    const data = await request('security', 'config.get')
    config.value = data || {}
    editConfig.value = JSON.stringify(data, null, 2)
  } catch { /* ignore */ }
}

async function saveConfig() {
  try {
    const parsed = JSON.parse(editConfig.value)
    await request('security', 'config.save', parsed)
    toast.success('已保存（层开关类改动重启网关生效）')
    editing.value = false
    await Promise.all([loadConfig(), loadOverview()])
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function loadOverview() {
  try {
    overview.value = (await request('security', 'overview')) || {}
  } catch { /* ignore */ }
}

function formatDate(ts?: string): string {
  if (!ts) return '--'
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN')
}

onMounted(async () => {
  await Promise.all([loadOverview(), loadConfig()])
  loading.value = false
})
</script>

<template>
  <div class="page-security-overview">
    <div class="page-header">
      <h2>安全</h2>
      <span class="layer-status" :class="globalSwitch.available ? (globalSwitch.enabled ? 'tone-on' : 'tone-off') : 'tone-na'" data-test="global-switch">
        {{ globalSwitch.available ? (globalSwitch.enabled ? '总闸开启' : '总闸关闭') : '总闸不可读' }}
      </span>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- 十层状态表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>十层防线</h3></div>
          <div class="card-body" style="padding: 0;">
            <div class="table-wrap">
              <table data-test="layers-table">
                <thead><tr><th style="width: 36px;">层</th><th>名称</th><th>状态</th><th>摘要</th><th style="width: 110px;">事件</th></tr></thead>
                <tbody>
                  <tr
                    v-for="row in ROWS"
                    :key="row.num"
                    class="layer-row"
                    :data-test="'layer-row-' + row.num"
                    @click="router.push(row.to)"
                  >
                    <td class="layer-num">{{ row.num }}</td>
                    <td>
                      <span class="layer-name">{{ row.title }}</span>
                      <span class="row-arrow">→</span>
                    </td>
                    <td>
                      <span class="layer-status" :class="'tone-' + rowStatus(row).tone" :data-test="'layer-status-' + row.num">{{ rowStatus(row).label }}</span>
                    </td>
                    <td class="layer-summary">{{ row.summary() }}</td>
                    <td class="layer-events">
                      <template v-if="row.num === '⑧'">
                        {{ intercepts.total }} 总 / {{ intercepts.denied }} 拦
                      </template>
                      <template v-else>{{ row.summary().match(/事件 (\d+) 条/)?.[1] ?? '--' }}</template>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 只读状态区（操作面在别处）+ 全局总闸 + 链校验 -->
        <div class="overview-grid">
          <div class="card">
            <div class="card-header"><h3>运行状态</h3></div>
            <div class="card-body">
              <div class="state-grid" data-test="runtime-states">
                <div class="state-row">
                  <span class="state-key">安全总闸</span>
                  <span class="layer-status" :class="globalSwitch.available ? (globalSwitch.enabled ? 'tone-on' : 'tone-off') : 'tone-na'">
                    {{ globalSwitch.available ? (globalSwitch.enabled ? '开启' : '关闭') : '不可读' }}
                  </span>
                  <span class="effective-chip live">即时生效</span>
                  <router-link class="state-link" to="/settings">设置页编辑 →</router-link>
                </div>
                <div class="state-row">
                  <span class="state-key">急停（estop）</span>
                  <span class="layer-status" :class="estop.injected ? (estop.engaged ? 'tone-off' : 'tone-on') : 'tone-na'">
                    {{ estop.injected ? (estop.engaged ? '已触发' : '未触发') : '未装配' }}
                  </span>
                  <span class="state-note">操作在侧栏底部按钮</span>
                </div>
                <div class="state-row">
                  <span class="state-key">Plan 模式</span>
                  <span class="layer-status" :class="planMode.available ? (planMode.mode === 'plan' ? 'tone-warn' : 'tone-on') : 'tone-na'">
                    {{ planMode.available ? modeLabel(planMode.mode) : '不可用' }}
                  </span>
                  <span class="state-note">切换在聊天面板徽标 / /plan /build</span>
                </div>
                <div class="state-row">
                  <span class="state-key">Full Access</span>
                  <span class="layer-status" :class="editor.injected ? (editor.full_access ? 'tone-warn' : 'tone-on') : 'tone-na'">
                    {{ editor.injected ? (editor.full_access ? '已开启' : '关闭') : '未装配' }}
                  </span>
                  <span class="state-note">设置页【编辑器】TAB</span>
                </div>
                <div class="state-row">
                  <span class="state-key">免打扰</span>
                  <span class="layer-status" :class="approvals.injected ? (approvals.enabled ? 'tone-warn' : 'tone-on') : 'tone-na'">
                    {{ approvals.injected ? (approvals.enabled ? `开（本次已自动批 ${approvals.auto_approved_count || 0}）` : '关') : '未装配' }}
                  </span>
                  <router-link class="state-link" to="/security/approval">审批页编辑 →</router-link>
                </div>
                <div class="state-row" v-if="signature.injected">
                  <span class="state-key">签名验证</span>
                  <span class="layer-status" :class="signature.last_result === 'Valid' ? 'tone-on' : signature.last_result === 'Tampered' ? 'tone-off' : 'tone-warn'">
                    {{ signature.last_result || '未验证' }}
                  </span>
                  <span class="state-note">模式 {{ signature.mode || '--' }}{{ signature.locked ? '（锁定版）' : '' }}</span>
                </div>
              </div>
            </div>
          </div>

          <div class="card">
            <div class="card-header">
              <h3>审计链校验</h3>
              <button class="btn btn-sm" data-test="overview-chain-verify" :disabled="!chain.present || chainVerifying" @click="verifyChain">
                {{ chainVerifying ? '校验中…' : '校验链完整性' }}
              </button>
            </div>
            <div class="card-body">
              <div class="chain-line">
                链{{ chain.present ? `在场 · ${chain.segments} 段` : '不在场（⑧ 审计链页开启 Merkle 后生成）' }}
              </div>
              <div class="hint" data-test="overview-chain-result">
                <template v-if="chainVerifyResult">
                  {{ chainVerifyResult.valid ? `✅ 通过（${chainVerifyResult.total_segments ?? 0} 段未被改写）` : `❌ 失败：首破段 #${chainVerifyResult.first_broken_index ?? '?'} · 破坏 ${chainVerifyResult.broken_count ?? '?'} 段` }}
                </template>
                <template v-else>审计事件 {{ intercepts.total }} 条 / 拦截 {{ intercepts.denied }} 条（最近 100 条见⑧ 审计链页）</template>
              </div>
              <div class="hint">最近事件：
                <span v-if="intercepts.by_policy" class="mono">
                  {{ Object.entries(intercepts.by_policy).slice(0, 4).map(([k, v]: any) => `${k}×${v.total}`).join(' · ') || '暂无' }}
                </span>
              </div>
              <div class="hint" style="margin-top: var(--space-1);" v-if="layers.default_action">默认姿态（未命中规则）：{{ layers.default_action }}</div>
            </div>
          </div>
        </div>

        <!-- 高级 JSON 编辑器（唯一任意键写入口） -->
        <div class="card" style="margin-top: var(--space-4);">
          <div class="card-header">
            <h3>安全策略 JSON（高级）</h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" data-test="json-edit" @click="editing = true">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" data-test="json-save" @click="saveConfig">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <div class="hint" style="margin: 0 0 var(--space-2);">
              完整 security 配置档（config.security.json）整体写回。层开关/策略下拉等常规编辑在各层页；这里兜底任意键编辑。保存后重启网关生效。
            </div>
            <textarea
              v-if="editing"
              v-model="editConfig"
              class="form-textarea"
              style="min-height: 50vh; font-family: var(--font-mono); font-size: var(--text-xs);"
              data-test="json-editor"
            ></textarea>
            <pre v-else class="json-preview" data-test="json-preview">{{ editConfig }}</pre>
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
.layer-num {
  font-size: var(--text-md, 15px);
  font-weight: 600;
}
.layer-row {
  cursor: pointer;
}
.layer-row:hover {
  background: var(--bg-elev, rgba(128, 128, 128, 0.08));
}
.layer-name {
  font-weight: 500;
}
.row-arrow {
  margin-left: var(--space-2);
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  opacity: 0;
  transition: opacity 0.15s;
}
.layer-row:hover .row-arrow {
  opacity: 1;
}
.layer-summary {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
}
.layer-events {
  font-size: var(--text-xs);
  font-family: var(--font-mono, 'Consolas', monospace);
  white-space: nowrap;
}
.overview-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
  gap: var(--space-4);
}
@media (max-width: 900px) {
  .overview-grid {
    grid-template-columns: 1fr;
  }
}
.state-grid {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.state-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.state-key {
  font-size: var(--text-sm);
  min-width: 96px;
}
.state-note {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
}
.state-link {
  font-size: var(--text-xs);
}
.effective-chip {
  font-size: var(--text-xs);
  padding: 0 8px;
  border-radius: 999px;
  border: 1px dashed var(--success, #2da44e);
  color: var(--success, #2da44e);
}
.effective-chip.live {
  border-color: var(--success, #2da44e);
}
.chain-line {
  font-size: var(--text-sm);
}
.json-preview {
  font-family: var(--font-mono, 'Consolas', monospace);
  font-size: var(--text-xs);
  max-height: 40vh;
  overflow: auto;
  background: var(--bg-elev, rgba(128, 128, 128, 0.06));
  padding: var(--space-2);
  border-radius: var(--radius-md, 8px);
  margin: 0;
}
</style>

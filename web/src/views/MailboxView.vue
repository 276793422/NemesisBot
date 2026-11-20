<script setup lang="ts">
// 邮箱页（邮件基础设施 B3，`mail` feature / VITE_FEATURE_MAIL 门控）：
// mail.* 命令的 Dashboard 管理面。
//
// 布局三段：顶部状态条（stats 分库卡 + 地址注册制 chips + 写信入口）→
// 左列表（当前库邮件，未读实心点；mailbox 下拉 + 只看未读开关）→
// 右读信/写信面板（读信含标记已读/删除——治理面命令，agent 工具面不供给）。
//
// 实时性：SMTP 落信 → 插件 notify_mail_in → SSE `mailbox.updated` 广播 →
// 300ms 防抖静默刷新（数据换新不闪 loading，board-changed 同款纪律）；
// 发信回执走 `mail.sent`（toast 提示 message_id）。
//
// 降级：mail.* 全命令在槽空（插件未装载/总闸关）时后端诚实报错——整页
// 中央空态卡呈现（检查清单 + 重试），操作面整块不渲染，绝不假成功。
import { ref, reactive, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { on as onSSE, off as offSSE } from '../composables/useSSE'

const { request } = useWSAPI()
const toast = useToast()

interface MailItem {
  id: string
  mailbox: string
  seen: boolean
  from: string
  subject: string
  date: string
  message_id: string
  size: number
}
interface MailboxStat { mailbox: string; total: number; unread: number; bytes: number }
interface ServerFace { running: boolean; bind?: string; port?: number; tls?: boolean }
interface MailStatus { loaded: boolean; hint?: string; smtp?: ServerFace; pop3?: ServerFace }
interface ReadResult {
  meta: { id?: string; from?: string; subject?: string; date?: string; mailbox?: string }
  body: string
}

const notLoaded = ref(false)
const status = ref<MailStatus | null>(null)
const stats = ref<MailboxStat[]>([])
const addresses = ref<string[]>([])
const defaultAddr = ref('')
const currentMailbox = ref('')   // '' = 默认主库
const unreadOnly = ref(false)
const items = ref<MailItem[]>([])
const loading = ref(false)
const selected = ref<ReadResult | null>(null)
const selectedId = ref('')

// ---- 写信表单 ----
const composeOpen = ref(false)
const composeTo = ref('')
const composeSubject = ref('')
const composeBody = ref('')
const sending = ref(false)

// ---- 地址注册管理（＋注册 / chip 注销）----
const addrOpen = ref(false)
const addrInput = ref('')
const addrBusy = ref(false)

// ---- 配置模态（mail.config_get / config_set；插件缺席也可配——先于句柄闸）----
const cfgOpen = ref(false)
const cfgBusy = ref(false)
const cfgSaving = ref(false)
const cfgPath = ref('')
// 读到的原始 config 整体保留：保存时以它为底只覆写表单管辖键——表单是
// 「已知键视图 + 未知键保留」，防整体落盘静默抹掉 notify 等表单外段。
const cfgRaw = ref<any>({})
// 扁平编辑态（嵌套键与表单互转集中在 applyToForm / formToConfig 两处，防散落）。
const cfg = reactive({
  enabled: false,
  smtpEnabled: false, smtpBind: '127.0.0.1', smtpPort: 2525,
  smtpDomains: '', smtpMaxMsgs: 20, smtpMaxRcptFails: 10, smtpIdle: 300,
  smtpTls: false, smtpTlsCert: '', smtpTlsKey: '',
  pop3Enabled: false, pop3Bind: '127.0.0.1', pop3Port: 1110,
  pop3User: '', pop3Pass: '', pop3Plaintext: false, pop3Idle: 600,
  pop3Tls: false, pop3TlsCert: '', pop3TlsKey: '',
  cliEnabled: false, cliHost: '', cliPort: 465,
  cliUser: '', cliPass: '', cliFrom: '', cliCa: '',
})

async function openConfig() {
  cfgOpen.value = true
  cfgBusy.value = true
  try {
    const v: any = await request('mail', 'config_get')
    cfgPath.value = v?.path ?? ''
    cfgRaw.value = v?.config ?? {}
    applyToForm(cfgRaw.value)
  } catch (e: any) {
    toast.error(`读配置失败：${e?.message ?? e}`)
    cfgOpen.value = false
  } finally {
    cfgBusy.value = false
  }
}

function applyToForm(c: any) {
  cfg.enabled = !!c.enabled
  cfg.smtpEnabled = !!c.smtp?.enabled
  cfg.smtpBind = c.smtp?.bind ?? '127.0.0.1'
  cfg.smtpPort = c.smtp?.port ?? 2525
  cfg.smtpDomains = Array.isArray(c.smtp?.local_domains) ? c.smtp.local_domains.join(', ') : (c.smtp?.local_domains ?? '')
  cfg.smtpMaxMsgs = c.smtp?.max_messages_per_connection ?? 20
  cfg.smtpMaxRcptFails = c.smtp?.max_rcpt_failures ?? 10
  cfg.smtpIdle = c.smtp?.idle_timeout_secs ?? 300
  cfg.smtpTls = !!c.smtp?.tls?.enabled
  cfg.smtpTlsCert = c.smtp?.tls?.cert_path ?? ''
  cfg.smtpTlsKey = c.smtp?.tls?.key_path ?? ''
  cfg.pop3Enabled = !!c.pop3?.enabled
  cfg.pop3Bind = c.pop3?.bind ?? '127.0.0.1'
  cfg.pop3Port = c.pop3?.port ?? 1110
  cfg.pop3User = c.pop3?.username ?? ''
  cfg.pop3Pass = c.pop3?.password ?? ''
  cfg.pop3Plaintext = !!c.pop3?.allow_plaintext_auth
  cfg.pop3Idle = c.pop3?.idle_timeout_secs ?? 600
  cfg.pop3Tls = !!c.pop3?.tls?.enabled
  cfg.pop3TlsCert = c.pop3?.tls?.cert_path ?? ''
  cfg.pop3TlsKey = c.pop3?.tls?.key_path ?? ''
  cfg.cliEnabled = !!c.client?.enabled
  cfg.cliHost = c.client?.smtp_host ?? ''
  cfg.cliPort = c.client?.smtp_port ?? 465
  cfg.cliUser = c.client?.username ?? ''
  cfg.cliPass = c.client?.password ?? ''
  cfg.cliFrom = c.client?.from ?? ''
  cfg.cliCa = c.client?.ca_cert_path ?? ''
}

function formToConfig(): any {
  const c = JSON.parse(JSON.stringify(cfgRaw.value ?? {}))
  c.enabled = cfg.enabled
  c.smtp = {
    ...(c.smtp ?? {}),
    enabled: cfg.smtpEnabled,
    bind: cfg.smtpBind,
    port: cfg.smtpPort,
    // 中英文逗号/空白混排宽容拆分（用户手填常态）
    local_domains: cfg.smtpDomains.split(/[,，\s]+/).map((s: string) => s.trim()).filter(Boolean),
    tls: { enabled: cfg.smtpTls, cert_path: cfg.smtpTlsCert, key_path: cfg.smtpTlsKey },
    max_messages_per_connection: cfg.smtpMaxMsgs,
    max_rcpt_failures: cfg.smtpMaxRcptFails,
    idle_timeout_secs: cfg.smtpIdle,
  }
  c.pop3 = {
    ...(c.pop3 ?? {}),
    enabled: cfg.pop3Enabled,
    bind: cfg.pop3Bind,
    port: cfg.pop3Port,
    username: cfg.pop3User,
    password: cfg.pop3Pass,
    allow_plaintext_auth: cfg.pop3Plaintext,
    idle_timeout_secs: cfg.pop3Idle,
    tls: { enabled: cfg.pop3Tls, cert_path: cfg.pop3TlsCert, key_path: cfg.pop3TlsKey },
  }
  c.client = {
    ...(c.client ?? {}),
    enabled: cfg.cliEnabled,
    smtp_host: cfg.cliHost,
    smtp_port: cfg.cliPort,
    username: cfg.cliUser,
    password: cfg.cliPass,
    from: cfg.cliFrom,
    ca_cert_path: cfg.cliCa,
  }
  return c
}

async function saveConfig() {
  cfgSaving.value = true
  try {
    // 响应驱动 note（2026-11-20 热装载通路）：enabled:true 且未装载 = 后端
    // 当场热装载（note「已保存并即时装载」/ load_error 引导重试）；已装载
    // 或停用 = 重启语义 note。保存成功且装载活 = 整页即时拉齐（空态卡
    // 消失、邮箱可用，无需重启网关）。
    const v: any = await request('mail', 'config_set', { config: formToConfig() })
    if (v?.load_error) {
      toast.error(`已保存；即时装载失败：${v.load_error}`)
    } else {
      toast.success(v?.note ?? '配置已保存')
    }
    cfgOpen.value = false
    await loadAll()
  } catch (e: any) {
    toast.error(`保存失败：${e?.message ?? e}`)
  } finally {
    cfgSaving.value = false
  }
}

// ---- 空态卡总闸直置开关（2026-11-20 热装载通路）----
// 未装载态最常见真因 = 总闸未开（enabled 缺省 false）。开关勾选 = 以盘上
// config 为底只覆写 enabled:true → 后端热装载（勾选即用，不等重启）；
// 取消勾选 = 只落盘（未装载态无运行面可停）。开关态来自盘上 config_get
// （空态卡出现时预取；配置模态保存后同步）。
const emptySwitch = ref(false)
const emptySwitchBusy = ref(false)

async function prefetchEmptySwitch() {
  try {
    const v: any = await request('mail', 'config_get')
    cfgRaw.value = v?.config ?? {} // 开关保存以盘上 config 为底（未知键保留）
    emptySwitch.value = !!cfgRaw.value?.enabled
  } catch {
    emptySwitch.value = false
  }
}

async function toggleEmptySwitch(on: boolean) {
  if (emptySwitchBusy.value) return
  emptySwitchBusy.value = true
  try {
    const raw = JSON.parse(JSON.stringify(cfgRaw.value ?? {}))
    raw.enabled = on
    const v: any = await request('mail', 'config_set', { config: raw })
    if (on && v?.loaded) {
      toast.success('已启用并即时装载（热生效）')
      await loadAll() // 整页活：空态卡消失、邮箱可用
      return
    }
    if (on && v?.load_error) {
      // 装载失败（DLL 缺席/签名拒载等）——盘已保存，开关留在勾选态
      // （配置正确性已成立，原因不在总闸），错误弹 toast 引导。
      toast.error(`已保存；即时装载失败：${v.load_error}`)
      return
    }
    if (!on) {
      toast.success(v?.note ?? '已保存')
    } else {
      // on 且无 load_error 且未 loaded：闭包未注入形态（非 gateway 装配）
      // ——落盘已成立，重启生效。
      toast.success(v?.note ?? '已保存；重启网关后生效')
    }
  } catch (e: any) {
    toast.error(`保存失败：${e?.message ?? e}`)
    emptySwitch.value = !on // 失败回退开关视觉态
  } finally {
    emptySwitchBusy.value = false
  }
}

const mailboxOptions = computed(() => [
  { value: '', label: defaultAddr.value ? `默认主库（${defaultAddr.value}）` : '默认主库' },
  ...addresses.value.filter(a => a !== defaultAddr.value).map(a => ({ value: a, label: a })),
])

function mailboxLabel(m: string): string {
  if (!m) return '默认主库'
  return m
}

// 分库卡点击：默认主库归一化为 ''（与下拉 option value 对齐——默认库的
// option value 是 ''，赋具体地址会导致 select 无匹配显示空白），并显式
// 刷新列表（程序化赋值不触发 select 的 @change）。
function selectMailbox(m: string) {
  currentMailbox.value = m === defaultAddr.value ? '' : m
  loadList()
}

// 状态探测先行：缺席（loaded:false / 请求失败）时短路返回，不发其余三个
// 注定失败的请求（也消除并行竞态——loadList 失败先到时 notLoaded 尚未置位
// 会弹一次多余的「列信失败」toast）。重试恢复路径：status 恢复 loaded:true
// 后继续全量拉齐。
async function loadAll() {
  await loadStatus()
  if (notLoaded.value) {
    await prefetchEmptySwitch() // 空态卡总闸开关态（盘上 enabled 真值）
    return
  }
  await Promise.all([loadStats(), loadAddr(), loadList()])
}

// 服务端运行态（mail.status：槽空 = loaded:false 的 Ok，不进 catch）。
async function loadStatus() {
  try {
    const v: any = await request('mail', 'status')
    if (v?.loaded === false) {
      notLoaded.value = true
      status.value = v
      return
    }
    notLoaded.value = false
    status.value = { loaded: true, smtp: v?.smtp, pop3: v?.pop3 }
  } catch (e: any) {
    // status 正常不报错；兜底按未装载渲染。
    notLoaded.value = true
    status.value = { loaded: false, hint: e?.message ?? String(e) }
  }
}

async function loadStats() {
  try {
    const v: any = await request('mail', 'stats')
    notLoaded.value = false
    stats.value = (v?.mailboxes ?? []) as MailboxStat[]
  } catch (e: any) {
    notLoaded.value = true
  }
}

async function loadAddr() {
  try {
    const v: any = await request('mail', 'addr_list')
    addresses.value = (v?.addresses ?? []) as string[]
    defaultAddr.value = (v?.default ?? '') as string
  } catch {
    // 槽空时 stats 已置 notLoaded，这里静默
  }
}

async function loadList() {
  loading.value = true
  try {
    const v: any = await request('mail', 'list', {
      mailbox: currentMailbox.value,
      unread_only: unreadOnly.value,
      limit: 200,
      offset: 0,
    })
    items.value = (v?.items ?? []) as MailItem[]
    notLoaded.value = false
  } catch (e: any) {
    items.value = []
    if (!notLoaded.value) toast.error(`列信失败：${e?.message ?? e}`)
  } finally {
    loading.value = false
  }
}

async function openMessage(it: MailItem) {
  try {
    const v: any = await request('mail', 'read', { id: it.id })
    selected.value = { meta: v?.meta ?? {}, body: v?.body ?? '' } as ReadResult
    selectedId.value = it.id
    if (!it.seen) {
      // 打开即标已读（maildir new→cur 语义在插件侧；前端乐观刷新）。
      it.seen = true
      try { await request('mail', 'mark_read', { id: it.id }) } catch { /* 静默——列表已乐观更新 */ }
      loadStats()
    }
  } catch (e: any) {
    toast.error(`读信失败：${e?.message ?? e}`)
  }
}

async function markRead(it: MailItem) {
  // mark_read 是单向置读（插件侧无未读回置）；列表内已读信不再提供操作。
  try {
    await request('mail', 'mark_read', { id: it.id })
    it.seen = true
    loadStats()
  } catch (e: any) {
    toast.error(`标记失败：${e?.message ?? e}`)
  }
}

async function deleteMessage(it: MailItem) {
  if (!window.confirm(`确认删除邮件「${it.subject || it.id}」？删除不可恢复。`)) return
  try {
    await request('mail', 'delete', { id: it.id })
    toast.success('已删除')
    if (selectedId.value === it.id) { selected.value = null; selectedId.value = '' }
    loadList()
    loadStats()
  } catch (e: any) {
    toast.error(`删除失败：${e?.message ?? e}`)
  }
}

async function sendMail() {
  if (!composeTo.value.trim()) { toast.error('收件人不能为空'); return }
  sending.value = true
  try {
    const v: any = await request('mail', 'send', {
      to: composeTo.value.trim(),
      subject: composeSubject.value,
      body: composeBody.value,
    })
    toast.success(`已发送（message_id: ${v?.message_id ?? '?'}）`)
    composeOpen.value = false
    composeTo.value = ''
    composeSubject.value = ''
    composeBody.value = ''
    loadStats()
  } catch (e: any) {
    toast.error(`发送失败：${e?.message ?? e}`)
  } finally {
    sending.value = false
  }
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

// ---- 地址注册管理 ----
async function addAddress() {
  const address = addrInput.value.trim()
  if (!address) { toast.error('地址不能为空'); return }
  addrBusy.value = true
  try {
    await request('mail', 'addr_add', { address })
    toast.success(`已注册 ${address}`)
    addrOpen.value = false
    addrInput.value = ''
    loadAddr()
    loadStats()
  } catch (e: any) {
    // 插件侧校验诚实报错回显（域不在 local_domains / 形态非法等）。
    toast.error(`注册失败：${e?.message ?? e}`)
  } finally {
    addrBusy.value = false
  }
}

async function removeAddress(a: string) {
  if (!window.confirm(`注销地址「${a}」？该地址将停止收信（信件保留不删除）。`)) return
  try {
    await request('mail', 'addr_remove', { address: a })
    toast.success(`已注销 ${a}`)
    if (currentMailbox.value === a) currentMailbox.value = ''
    loadAddr()
    loadList()
    loadStats()
  } catch (e: any) {
    toast.error(`注销失败：${e?.message ?? e}`)
  }
}

// ---- SSE 实时刷新（mailbox.updated 防抖 300ms；mail.sent 回执 toast）----
let debounceTimer: ReturnType<typeof setTimeout> | null = null
function onMailboxUpdated() {
  if (notLoaded.value) return // 空态卡：无操作面可刷新，不发注定失败的请求
  if (debounceTimer !== null) clearTimeout(debounceTimer)
  debounceTimer = setTimeout(() => {
    debounceTimer = null
    loadList()
    loadStats()
  }, 300)
}
function onMailSent(data: any) {
  toast.success(`发信回执：${data?.to ?? ''}（${data?.message_id ?? ''}）`)
}
onSSE('mailbox.updated', onMailboxUpdated)
onSSE('mail.sent', onMailSent)

onMounted(loadAll)
onUnmounted(() => {
  offSSE('mailbox.updated', onMailboxUpdated)
  offSSE('mail.sent', onMailSent)
  if (debounceTimer !== null) clearTimeout(debounceTimer)
})
</script>

<template>
  <div class="page mailbox-page">
    <div class="page-header">
      <h1>邮箱</h1>
      <div v-if="!notLoaded" class="header-actions">
        <button class="btn primary" @click="composeOpen = true">✉ 写信</button>
        <button class="btn" @click="addrOpen = true">＋ 注册地址</button>
        <button class="btn" @click="openConfig">⚙ 配置</button>
        <button class="btn" @click="loadAll">↻ 刷新</button>
      </div>
    </div>

    <!-- 插件缺席空态卡（三层语义的中间层）：编译态（mail feature 裁掉）整页
         不存在；运行时缺席（DLL 缺席/总闸关/名册关）= 此卡引导；总闸开关
         勾选 = 后端热装载（勾选即用，不等重启——2026-11-20 热装载通路）；
         在场 = 完整邮箱。 -->
    <div v-if="notLoaded" class="mail-empty">
      <div class="empty-icon">✉</div>
      <h2>邮箱插件未装载</h2>
      <p class="empty-hint">{{ status?.hint || 'plugin_mail DLL 缺席、config.mail.json 总闸未开或插件名册未启用。' }}</p>
      <label class="empty-switch-row" :class="{ busy: emptySwitchBusy }">
        <input
          type="checkbox"
          :checked="emptySwitch"
          :disabled="emptySwitchBusy"
          @change="toggleEmptySwitch(($event.target as HTMLInputElement).checked)"
        />
        <span>启用邮箱插件（勾选即时装载，无需重启网关）</span>
      </label>
      <ul class="empty-checklist">
        <li>确认 <code>plugins/plugin_mail.dll</code>（exe 旁 plugins/ 目录）存在</li>
        <li>总闸 <code>config.mail.json</code> <code>{"enabled": true}</code>——上方开关勾选即自动写入并热装载</li>
        <li>插件名册 <code>config.plugin.native.json</code> 未将 plugin_mail 关闭（缺文件 = 默认启用）</li>
        <li>DLL / 名册修改需重启网关生效；总闸开关勾选即时生效</li>
      </ul>
      <div class="empty-actions">
        <button class="btn primary" @click="loadAll">↻ 重试</button>
        <!-- 配置双入口之一（先于句柄闸：插件缺席也要能配——SMTP/POP3/客户端
             全量配置，保存时总闸开 = 即时热装载） -->
        <button class="btn" @click="openConfig">⚙ 邮箱配置</button>
      </div>
    </div>

    <!-- 服务端运行态卡（SMTP 收信 / POP3 取信 各自 running/bind/port/tls）：
         tls 徽标 = STARTTLS（SMTP）或 POP3S；运行无 TLS 时小字「明文」提示
         （公网部署信号——后端准入闸已兜底，此处只做可感知） -->
    <div v-if="!notLoaded && status" class="srv-strip">
      <div class="srv-card" :class="{ up: status.smtp?.running }">
        <span class="srv-dot">{{ status.smtp?.running ? '●' : '○' }}</span>
        <b>SMTP 收信</b>
        <template v-if="status.smtp?.running">
          <span class="srv-ep">{{ status.smtp.bind }}:{{ status.smtp.port }}</span>
          <span v-if="status.smtp.tls" class="srv-tls">🔒STARTTLS</span>
          <span v-else class="srv-plain">明文</span>
        </template>
        <span v-else class="srv-off">未启用（config.mail.json smtp.enabled）</span>
      </div>
      <div class="srv-card" :class="{ up: status.pop3?.running }">
        <span class="srv-dot">{{ status.pop3?.running ? '●' : '○' }}</span>
        <b>POP3 取信</b>
        <template v-if="status.pop3?.running">
          <span class="srv-ep">{{ status.pop3.bind }}:{{ status.pop3.port }}</span>
          <span v-if="status.pop3.tls" class="srv-tls">🔒POP3S</span>
          <span v-else class="srv-plain">明文</span>
        </template>
        <span v-else class="srv-off">未启用（config.mail.json pop3.enabled）</span>
      </div>
    </div>

    <!-- 状态条：分库统计 + 地址注册制（插件缺席时空态卡顶替，整块不渲染） -->
    <template v-if="!notLoaded">
    <div class="stat-strip">
      <div v-for="s in stats" :key="s.mailbox" class="stat-card" :class="{ active: currentMailbox === s.mailbox || (!currentMailbox && s.mailbox === defaultAddr) }" @click="selectMailbox(s.mailbox)">
        <div class="stat-name">{{ mailboxLabel(s.mailbox) }}</div>
        <div class="stat-nums">{{ s.total }} 封 <span v-if="s.unread" class="unread-badge">{{ s.unread }} 未读</span></div>
        <div class="stat-bytes">{{ fmtSize(s.bytes) }}</div>
      </div>
      <div v-if="!stats.length" class="stat-card empty">（未注册任何地址）</div>
    </div>
    <div v-if="addresses.length" class="addr-chips">
      <span class="chip" v-for="a in addresses" :key="a" :class="{ def: a === defaultAddr }" :title="a === defaultAddr ? '默认主库' : ''">
        {{ a }}<span v-if="a === defaultAddr"> ·默认</span>
        <button class="chip-x" title="注销该地址" @click="removeAddress(a)">×</button>
      </span>
    </div>
    </template>

    <!-- 主区：列表 + 读信 -->
    <div v-if="!notLoaded" class="mail-main">
      <div class="list-pane">
        <div class="list-tools">
          <select v-model="currentMailbox" @change="loadList" class="sel">
            <option v-for="o in mailboxOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
          </select>
          <label class="chk"><input type="checkbox" v-model="unreadOnly" @change="loadList" /> 只看未读</label>
        </div>
        <div v-if="loading" class="muted pad">加载中…</div>
        <div v-else-if="!items.length" class="muted pad">（此库暂无邮件）</div>
        <div v-else class="mail-list">
          <div v-for="it in items" :key="it.id" class="mail-row" :class="{ unread: !it.seen, selected: selectedId === it.id }" @click="openMessage(it)">
            <span class="dot">{{ it.seen ? '' : '●' }}</span>
            <span class="from">{{ it.from }}</span>
            <span class="subject">{{ it.subject || '（无主题）' }}</span>
            <span class="date">{{ it.date }}</span>
            <button class="row-btn" title="标记已读" v-if="!it.seen" @click.stop="markRead(it)">✓</button>
            <button class="row-btn danger" title="删除" @click.stop="deleteMessage(it)">🗑</button>
          </div>
        </div>
      </div>

      <div class="reader-pane">
        <template v-if="selected">
          <div class="reader-head">
            <div class="reader-subject">{{ selected.meta.subject || '（无主题）' }}</div>
            <div class="reader-meta">
              发件人：{{ selected.meta.from || '?' }}
              <span v-if="selected.meta.date"> · {{ selected.meta.date }}</span>
              <span v-if="selected.meta.mailbox"> · 库：{{ mailboxLabel(selected.meta.mailbox || '') }}</span>
            </div>
          </div>
          <pre class="reader-body">{{ selected.body }}</pre>
        </template>
        <div v-else class="muted pad">← 从左侧选择一封邮件阅读</div>
      </div>
    </div>

    <!-- 写信模态 -->
    <div v-if="composeOpen" class="modal-mask" @click.self="composeOpen = false">
      <div class="modal compose-modal">
        <h3>写信</h3>
        <label class="fld">收件人
          <input v-model="composeTo" type="text" placeholder="name@your.domain" />
        </label>
        <label class="fld">主题
          <input v-model="composeSubject" type="text" placeholder="（可空）" />
        </label>
        <label class="fld">正文
          <textarea v-model="composeBody" rows="10" placeholder="邮件正文…"></textarea>
        </label>
        <div class="modal-actions">
          <button class="btn" @click="composeOpen = false">取消</button>
          <button class="btn primary" :disabled="sending" @click="sendMail">{{ sending ? '发送中…' : '发送' }}</button>
        </div>
      </div>
    </div>
    <!-- 注册地址模态（域校验在插件侧：不在 local_domains 的域注册后收不到信，
         SMTP 投递时 550 诚实报错） -->
    <div v-if="addrOpen" class="modal-mask" @click.self="addrOpen = false">
      <div class="modal addr-modal">
        <h3>注册收信地址</h3>
        <label class="fld">地址
          <input v-model="addrInput" type="text" placeholder="name@your.domain" @keyup.enter="addAddress" />
        </label>
        <p class="addr-hint">域需已在 config.mail.json 的 smtp.local_domains 内，否则 SMTP 投递将被 550 拒绝。</p>
        <div class="modal-actions">
          <button class="btn" @click="addrOpen = false">取消</button>
          <button class="btn primary" :disabled="addrBusy" @click="addAddress">{{ addrBusy ? '注册中…' : '注册' }}</button>
        </div>
      </div>
    </div>
    <!-- 邮箱配置模态（config.mail.json 表单视图；config_get/config_set 先于
         句柄闸——插件缺席也可配；表单外键（notify 等）保存时原样保留。 -->
    <div v-if="cfgOpen" class="modal-mask" @click.self="cfgOpen = false">
      <div class="modal cfg-modal">
        <h3>邮箱配置</h3>
        <p class="cfg-path">{{ cfgPath || 'workspace/config/config.mail.json' }} · 保存后需重启网关生效</p>
        <div v-if="cfgBusy" class="muted pad">加载中…</div>
        <div v-else class="cfg-body">
          <section class="cfg-sec">
            <h4>总闸</h4>
            <label class="chk-row"><input type="checkbox" v-model="cfg.enabled" /> 启用邮箱插件（总闸关闭 = 全部收发面停用）</label>
          </section>
          <section class="cfg-sec">
            <h4>SMTP 收信（外部邮件投递入口）</h4>
            <label class="chk-row"><input type="checkbox" v-model="cfg.smtpEnabled" /> 启用 SMTP 收信服务</label>
            <div class="cfg-grid">
              <label class="fld">绑定地址
                <input v-model="cfg.smtpBind" placeholder="127.0.0.1（公网收信改 0.0.0.0）" />
              </label>
              <label class="fld">端口
                <input v-model.number="cfg.smtpPort" type="number" min="1" max="65535" placeholder="2525" />
              </label>
              <label class="fld cfg-wide">收信域名（逗号分隔；空 = 拒绝全部投递）
                <input v-model="cfg.smtpDomains" placeholder="corp.example, mail.corp.example" />
              </label>
              <label class="fld">单连接投递上限
                <input v-model.number="cfg.smtpMaxMsgs" type="number" min="0" placeholder="20（0=不限）" />
              </label>
              <label class="fld">单连接 RCPT 拒绝上限
                <input v-model.number="cfg.smtpMaxRcptFails" type="number" min="0" placeholder="10（0=不限）" />
              </label>
              <label class="fld">会话空闲超时（秒）
                <input v-model.number="cfg.smtpIdle" type="number" min="0" placeholder="300（0=不限）" />
              </label>
            </div>
            <label class="chk-row"><input type="checkbox" v-model="cfg.smtpTls" /> STARTTLS（RFC 3207；需证书，配了才广告 250-STARTTLS）</label>
            <div class="cfg-grid" v-if="cfg.smtpTls">
              <label class="fld">证书路径（PEM）
                <input v-model="cfg.smtpTlsCert" placeholder="cert.pem" />
              </label>
              <label class="fld">私钥路径（PEM）
                <input v-model="cfg.smtpTlsKey" placeholder="key.pem" />
              </label>
            </div>
            <p class="cfg-note">配证书后装配失败将拒绝启动（fail-closed）；非回环绑址 + 明文形态会启动 WARN。</p>
          </section>
          <section class="cfg-sec">
            <h4>POP3 取信（客户端拉取）</h4>
            <label class="chk-row"><input type="checkbox" v-model="cfg.pop3Enabled" /> 启用 POP3 取信服务</label>
            <div class="cfg-grid">
              <label class="fld">绑定地址
                <input v-model="cfg.pop3Bind" placeholder="127.0.0.1" />
              </label>
              <label class="fld">端口
                <input v-model.number="cfg.pop3Port" type="number" min="1" max="65535" placeholder="1110（995=标准 POP3S）" />
              </label>
              <label class="fld">认证用户名
                <input v-model="cfg.pop3User" placeholder="（空 = 免认证并集，仅限回环）" />
              </label>
              <label class="fld">认证密码
                <input v-model="cfg.pop3Pass" type="password" placeholder="（空 = 免认证）" />
              </label>
              <label class="fld">会话空闲超时（秒）
                <input v-model.number="cfg.pop3Idle" type="number" min="0" placeholder="600（0=不限）" />
              </label>
            </div>
            <label class="chk-row"><input type="checkbox" v-model="cfg.pop3Tls" /> POP3S 整面 implicit TLS（非回环必须；需证书）</label>
            <div class="cfg-grid" v-if="cfg.pop3Tls">
              <label class="fld">证书路径（PEM）
                <input v-model="cfg.pop3TlsCert" placeholder="cert.pem" />
              </label>
              <label class="fld">私钥路径（PEM）
                <input v-model="cfg.pop3TlsKey" placeholder="key.pem" />
              </label>
            </div>
            <label class="chk-row"><input type="checkbox" v-model="cfg.pop3Plaintext" /> 允许非回环明文认证（逃生阀；默认拒启，LAN 内网自担）</label>
          </section>
          <section class="cfg-sec">
            <h4>出站发信（发往公共邮箱如 QQ / 163）</h4>
            <label class="chk-row"><input type="checkbox" v-model="cfg.cliEnabled" /> 启用出站发信</label>
            <div class="cfg-grid">
              <label class="fld">SMTP 服务器
                <input v-model="cfg.cliHost" placeholder="smtp.qq.com / smtp.163.com" />
              </label>
              <label class="fld">端口
                <input v-model.number="cfg.cliPort" type="number" min="1" max="65535" placeholder="465（implicit TLS）" />
              </label>
              <label class="fld">登录账号
                <input v-model="cfg.cliUser" placeholder="完整邮箱地址（如 12345@qq.com）" />
              </label>
              <label class="fld">登录密码 / 授权码
                <input v-model="cfg.cliPass" type="password" placeholder="QQ/163 用授权码（非登录密码）" />
              </label>
              <label class="fld">发件人（From）
                <input v-model="cfg.cliFrom" placeholder="缺省 = 登录账号" />
              </label>
              <label class="fld">自定义 CA 证书（PEM）
                <input v-model="cfg.cliCa" placeholder="留空 = 公共根内置（公共邮箱直连）" />
              </label>
            </div>
            <p class="cfg-note">QQ 邮箱：设置→账户→开启 SMTP 服务→生成授权码；163 同理。公共邮箱 CA 证书留空即可（内置公共根）。</p>
          </section>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="cfgOpen = false">取消</button>
          <button class="btn primary" :disabled="cfgBusy || cfgSaving" @click="saveConfig">{{ cfgSaving ? '保存中…' : '保存' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 主题 token 全走 theme.css 真相源（--accent/--text/--text-muted/--border/
   --surface 族）——不引用 --nb-*（皮肤自定义名，主题态无定义点，静默吃
   fallback 是白底白字事故同款反模式）；亮色主题自动跟随。 */
.mailbox-page { display: flex; flex-direction: column; gap: 12px; height: 100%; overflow-y: auto; }
.page-header { display: flex; align-items: center; justify-content: space-between; }
.header-actions { display: flex; gap: 8px; }
.stat-strip { display: flex; gap: 10px; flex-wrap: wrap; }
.stat-card { border: 1px solid var(--border); border-radius: var(--radius-md); padding: var(--space-2) var(--space-4); cursor: pointer; min-width: 140px; }
.stat-card.active { border-color: var(--accent); }
.stat-card.empty { color: var(--text-muted); cursor: default; }
.stat-name { font-weight: 600; font-size: var(--text-sm); }
.stat-nums { font-size: var(--text-xs); color: var(--text-secondary); }
.stat-bytes { font-size: 11px; color: var(--text-muted); }
.unread-badge { color: var(--accent); font-weight: 600; }
.addr-chips { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
.chip { font-size: var(--text-xs); border: 1px solid var(--border); border-radius: var(--radius-full); padding: 2px 6px 2px 10px; color: var(--text-muted); display: inline-flex; align-items: center; gap: 4px; }
.chip.def { color: var(--accent); border-color: var(--accent); }
.chip-x { background: none; border: none; cursor: pointer; color: var(--text-muted); font-size: var(--text-sm); line-height: 1; padding: 0 2px; }
.chip-x:hover { color: var(--error); }
.srv-strip { display: flex; gap: 10px; flex-wrap: wrap; }
.srv-card { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); border: 1px solid var(--border); border-radius: var(--radius-md); padding: 6px var(--space-4); color: var(--text-muted); }
.srv-card.up { color: var(--text-secondary); }
.srv-dot { color: var(--text-muted); }
.srv-card.up .srv-dot { color: var(--success); }
.srv-ep { font-family: var(--font-mono); color: var(--accent); }
.srv-tls { font-size: 11px; color: var(--success); }
.srv-plain { font-size: 11px; color: var(--warning); }
.srv-off { font-size: var(--text-xs); color: var(--text-muted); }
.addr-modal { display: flex; flex-direction: column; gap: 10px; }
.addr-modal h3 { margin: 0; }
.addr-hint { margin: 0; font-size: var(--text-xs); color: var(--text-muted); }
.mail-empty .empty-actions { display: flex; gap: var(--space-2); }
.cfg-modal { display: flex; flex-direction: column; gap: 8px; width: min(720px, 94vw); }
.cfg-modal h3 { margin: 0; }
.cfg-path { margin: 0; font-size: var(--text-xs); color: var(--text-muted); font-family: var(--font-mono); word-break: break-all; }
.cfg-body { max-height: 60vh; overflow-y: auto; display: flex; flex-direction: column; gap: 12px; padding-right: 4px; }
.cfg-sec { border: 1px solid var(--border-light); border-radius: var(--radius-md); padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; }
.cfg-sec h4 { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }
.cfg-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px 12px; }
.cfg-grid .fld input { width: 100%; box-sizing: border-box; }
.cfg-wide { grid-column: 1 / -1; }
.chk-row { display: flex; align-items: center; gap: 6px; font-size: var(--text-sm); color: var(--text-secondary); }
.cfg-note { margin: 0; font-size: var(--text-xs); color: var(--text-muted); }
.mail-main { display: grid; grid-template-columns: minmax(320px, 5fr) 7fr; gap: 12px; flex: 1; min-height: 320px; }
.list-pane, .reader-pane { border: 1px solid var(--border); border-radius: var(--radius-md); display: flex; flex-direction: column; overflow: hidden; }
.list-tools { display: flex; align-items: center; gap: 10px; padding: var(--space-2) 10px; border-bottom: 1px solid var(--border); }
.sel { background: var(--surface); color: var(--text-secondary); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 4px var(--space-2); font-size: var(--text-sm); }
.chk { font-size: var(--text-sm); color: var(--text-muted); display: flex; align-items: center; gap: 4px; }
.mail-list { overflow-y: auto; flex: 1; }
.mail-row { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) 10px; border-bottom: 1px solid var(--border-light); cursor: pointer; font-size: var(--text-sm); }
.mail-row:hover { background: var(--surface-hover); }
.mail-row.selected { background: var(--surface-active); }
.mail-row.unread .subject { font-weight: 700; color: var(--text); }
.mail-row .dot { width: 12px; color: var(--accent); flex: none; }
.mail-row .from { max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); flex: none; }
.mail-row .subject { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }
.mail-row .date { font-size: 11px; color: var(--text-muted); flex: none; }
.row-btn { background: none; border: none; cursor: pointer; font-size: var(--text-sm); padding: 2px 4px; color: var(--text-muted); }
.row-btn:hover { color: var(--text); }
.row-btn.danger:hover { color: var(--error); }
.reader-head { padding: 12px 14px; border-bottom: 1px solid var(--border); }
.reader-subject { font-size: var(--text-lg); font-weight: 700; color: var(--text); }
.reader-meta { font-size: var(--text-xs); color: var(--text-muted); margin-top: 4px; }
.reader-body { flex: 1; overflow-y: auto; padding: 14px; margin: 0; font-size: var(--text-sm); line-height: 1.6; white-space: pre-wrap; word-break: break-word; font-family: inherit; color: var(--text-secondary); }
.muted { color: var(--text-muted); }
.pad { padding: var(--space-4); font-size: var(--text-sm); }
.modal-mask { position: fixed; inset: 0; background: rgba(0,0,0,.55); display: flex; align-items: center; justify-content: center; z-index: 200; }
.modal { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 18px; min-width: 420px; max-width: 640px; }
.compose-modal { display: flex; flex-direction: column; gap: 10px; width: min(640px, 92vw); }
.compose-modal h3 { margin: 0; }
.fld { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-sm); color: var(--text-muted); }
.fld input, .fld textarea { background: var(--bg-secondary); color: var(--text-secondary); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 10px; font-size: var(--text-sm); font-family: inherit; }
.modal-actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: 6px; }
.mail-empty { border: 1px dashed var(--border); border-radius: 10px; padding: 40px 24px; display: flex; flex-direction: column; align-items: center; gap: 10px; text-align: center; margin-top: var(--space-2); }
.mail-empty .empty-icon { font-size: 42px; opacity: .45; }
.mail-empty h2 { margin: 0; font-size: 17px; color: var(--text-secondary); }
.mail-empty .empty-hint { margin: 0; font-size: var(--text-sm); color: var(--text-muted); max-width: 560px; }
.mail-empty .empty-checklist { margin: 6px 0 2px; padding: 0; list-style: none; text-align: left; font-size: var(--text-sm); color: var(--text-muted); display: flex; flex-direction: column; gap: 6px; max-width: 560px; }
.mail-empty .empty-checklist code { font-size: var(--text-xs); color: var(--accent); word-break: break-all; }
.mail-empty .empty-switch-row { display: flex; align-items: center; gap: 8px; font-size: var(--text-sm); color: var(--text-secondary); cursor: pointer; user-select: none; margin: 4px 0 2px; padding: 8px 14px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-secondary); }
.mail-empty .empty-switch-row input { accent-color: var(--accent); width: 15px; height: 15px; cursor: pointer; }
.mail-empty .empty-switch-row.busy { opacity: .6; pointer-events: none; }
</style>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'

// 紫微系统（【万物皆工具】）——三方程序受控接管。
// M0：profile 列表 + 状态 + 门关横幅；M1/M2：会话视图（attach/invoke/
// detach）+ 动态工具 refresh；M3：配置向导六步（选目标→快照→分析→
// 候选与标注→结构/工具→提交）。
const { request } = useWSAPI()

const loading = ref(true)
const gateClosed = ref(false)
const gateMsg = ref('')
const profiles = ref<any[]>([])
const status = ref<any>(null)
const selected = ref<any>(null)
const selectedNotes = ref('')
const busy = ref(false)
// 会话面（M2）
const attachPid = ref('')
const attachAddrJson = ref('')
const sessionMsg = ref('')
const invokeSnapshot = ref<any>(null)
const toolCache = ref<Record<string, string[]>>({})

const activeCount = computed(() => profiles.value.filter((p: any) => p.status === 'active').length)
const staleCount = computed(() => profiles.value.filter((p: any) => p.status === 'stale').length)
const sessions = computed<any[]>(() => status.value?.sessions ?? [])
const staleProfiles = computed(() => profiles.value.filter((p: any) => p.status === 'stale'))

// ── 失效找回（M5 T5.2/T5.3）：横幅双按钮 + 报告模态 + 逐函数确认 ──
const recover = ref<{
  open: boolean
  profile_id: string
  verdict: string
  anchors: any[]
  md: string
  accepted: Record<string, boolean>
  busy: boolean
  msg: string
}>({ open: false, profile_id: '', verdict: '', anchors: [], md: '', accepted: {}, busy: false, msg: '' })

async function recoverStart(p: any) {
  recover.value = { open: true, profile_id: p.profile_id, verdict: '', anchors: [], md: '', accepted: {}, busy: true, msg: '逐锚重扫中…' }
  try {
    const r = await request('ziwei', 'recover', { profile_id: p.profile_id })
    const v = r?.report?.verdict ?? {}
    recover.value.verdict = v.verdict ?? '?'
    recover.value.anchors = v.per_anchor ?? []
    recover.value.md = r?.report_md ?? ''
    for (const a of recover.value.anchors) {
      if (a.hit) recover.value.accepted[a.function] = true
    }
    recover.value.msg = ''
  } catch (e: any) {
    recover.value.msg = `✗ ${e}`
  } finally {
    recover.value.busy = false
  }
}

async function recoverConfirm() {
  const accept = Object.entries(recover.value.accepted).filter(([, v]) => v).map(([k]) => k)
  if (!accept.length) {
    recover.value.msg = '至少勾选一个要找回的函数（全部不恢复请直接重配）'
    return
  }
  recover.value.busy = true
  recover.value.msg = '确认入库中…'
  try {
    const r = await request('ziwei', 'recover_confirm', { profile_id: recover.value.profile_id, accept })
    recover.value.msg = r?.all_recovered
      ? '✓ 全部恢复：profile 已回 active（新 hash 基准已锚定）'
      : '部分恢复：未勾选/未命中的函数保持 stale'
    recover.value.open = false
    await refresh()
  } catch (e: any) {
    recover.value.msg = `✗ ${e}`
  } finally {
    recover.value.busy = false
  }
}

async function verifyProfile(p: any) {
  busy.value = true
  sessionMsg.value = ''
  try {
    const r = await request('ziwei', 'verify', { profile_id: p.profile_id })
    sessionMsg.value = `✓ 验证 ${p.profile_id}：裁决 ${r?.report?.verdict ?? '?'}`
    await refresh()
  } catch (e: any) {
    sessionMsg.value = `✗ ${e}`
  } finally {
    busy.value = false
  }
}

async function refresh() {
  loading.value = true
  try {
    const st = await request('ziwei', 'status').catch((e: string) => {
      if (e && e.includes('未启用')) {
        gateClosed.value = true
        gateMsg.value = e
      }
      return null
    })
    if (st) {
      gateClosed.value = false
      status.value = st
      const r = await request('ziwei', 'list').catch(() => null)
      profiles.value = r?.profiles ?? []
      // 会话视图：拉活动会话的子工具名（invoke 按钮面）。
      for (const s of st.sessions ?? []) {
        const d = await request('ziwei', 'show', { profile_id: s.profile_id }).catch(() => null)
        toolCache.value[s.profile_id] = (d?.profile?.tools ?? []).map((t: any) => t.tool_name)
      }
    }
  } finally {
    loading.value = false
  }
}

async function showProfile(p: any) {
  const r = await request('ziwei', 'show', { profile_id: p.profile_id }).catch((e: string) => {
    return { __err: e }
  })
  if (r && r.__err) return
  selected.value = r?.profile ?? null
  selectedNotes.value = r?.notes ?? ''
}

async function removeProfile(p: any) {
  if (!confirm(`删除 profile「${p.name}」及其全部快照/运行记录？`)) return
  busy.value = true
  try {
    await request('ziwei', 'remove', { profile_id: p.profile_id })
    if (selected.value?.profile_id === p.profile_id) selected.value = null
    await refresh()
  } finally {
    busy.value = false
  }
}

// ── 会话面（M2）──
async function attachProfile(p: any) {
  const pid = Number(attachPid.value)
  if (!pid) {
    sessionMsg.value = '请先在上方填目标进程 PID'
    return
  }
  let structAddrs: Record<string, number> = {}
  if (attachAddrJson.value.trim()) {
    try {
      structAddrs = JSON.parse(attachAddrJson.value)
    } catch {
      sessionMsg.value = '结构地址 JSON 解析失败'
      return
    }
  }
  busy.value = true
  sessionMsg.value = ''
  try {
    const r = await request('ziwei', 'attach', {
      profile_id: p.profile_id,
      pid,
      struct_addrs: structAddrs,
    })
    sessionMsg.value = `✓ 已附加 pid=${r?.session?.pid ?? pid}（${r?.session?.state ?? ''}）`
    selected.value = p
    await refresh()
  } catch (e: any) {
    sessionMsg.value = `✗ ${e}`
  } finally {
    busy.value = false
  }
}

async function detachSession(s: any) {
  if (!confirm(`卸载会话 ${s.profile_id}（pid=${s.pid}）？payload 将自卸。`)) return
  busy.value = true
  try {
    const r = await request('ziwei', 'detach', { profile_id: s.profile_id })
    sessionMsg.value = `✓ ${r?.message ?? '已卸载'}`
    invokeSnapshot.value = null
    await refresh()
  } catch (e: any) {
    sessionMsg.value = `✗ ${e}`
  } finally {
    busy.value = false
  }
}

async function invokeTool(p: any, toolName: string) {
  busy.value = true
  sessionMsg.value = ''
  try {
    const r = await request('ziwei', 'invoke', { profile_id: p.profile_id, tool: toolName })
    invokeSnapshot.value = r
    sessionMsg.value = `✓ ${toolName} 快照已取得`
  } catch (e: any) {
    sessionMsg.value = `✗ ${e}`
    invokeSnapshot.value = null
  } finally {
    busy.value = false
  }
}

async function refreshTools() {
  busy.value = true
  try {
    const r = await request('ziwei', 'refresh')
    sessionMsg.value = `✓ 动态工具重注册：${r?.registered ?? 0} 个`
  } catch (e: any) {
    sessionMsg.value = `✗ ${e}`
  } finally {
    busy.value = false
  }
}

function sessionOf(profileId: string): any {
  return sessions.value.find((s: any) => s.profile_id === profileId)
}

// ── 配置向导（M3 六步：选目标→快照→分析→候选与标注→结构/工具→提交）──
const wiz = ref({
  open: false,
  step: 1,
  target: '',
  info: null as any,
  analysis: null as any,
  candidates: null as any,
  diff: null as any,
  waitSecs: '5',
  structs: [] as string[],
  tools: [] as string[],
  msg: '',
})
const ann = ref({ rva: '', semantic: '', kind: 'byte_pattern', pattern: '', text: '', value: '', imports: '', conv: 'ms_x64', note: '' })
const ds = ref({ name: '', size: '', fields: '' })
const dt = ref({ tool_name: '', description: '', primitive: 'poll_report', addr: '', mode: 'on_change' })

function wizMsg(m: string) {
  wiz.value.msg = m
}

async function wizOpen() {
  wiz.value.open = true
  wiz.value.msg = ''
  try {
    const st = await request('ziwei', 'setup.status', {})
    wiz.value.info = st ?? null
    if (st?.profile_id) {
      wiz.value.step = 4 // 草稿在即从标注步恢复（快照类步骤可重跑）
      wizMsg(`检测到未收尾向导草稿（${st.profile_id}）——已恢复；快照类步骤需重选目标`)
    }
  } catch {
    wiz.value.info = null
  }
}

function wizReset() {
  wiz.value = { open: false, step: 1, target: '', info: null, analysis: null, candidates: null, diff: null, waitSecs: '5', structs: [], tools: [], msg: '' }
}

async function wizSelectTarget() {
  const t = wiz.value.target.trim()
  if (!t) return wizMsg('填目标进程名或 PID')
  busy.value = true
  try {
    const data: any = /^\d+$/.test(t) ? { pid: Number(t) } : { exe: t }
    wiz.value.info = await request('ziwei', 'setup.select_target', data)
    wiz.value.step = 2
    wizMsg(`✓ 草稿 ${wiz.value.info?.profile_id} 已建（pid=${wiz.value.info?.pid}，${wiz.value.info?.exe_name}）`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizSnapshot() {
  busy.value = true
  try {
    await request('ziwei', 'setup.snapshot', {})
    wizMsg('✓ 快照已落盘（头页 + .text/.rdata/.data）')
    const r = await request('ziwei', 'setup.analyze', {})
    wiz.value.analysis = r
    wiz.value.step = 4
    wizMsg(`✓ 分析完成：函数 ${r?.functions}，CRT 折叠 ${r?.crt_folded}（${r?.analysis_json}）`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizAnalyze() {
  busy.value = true
  try {
    const r = await request('ziwei', 'setup.analyze', {})
    wiz.value.analysis = r
    wizMsg(`✓ 分析完成：函数 ${r?.functions}，CRT 折叠 ${r?.crt_folded}${r?.llm_suggest_error ? `（LLM 建议：${r.llm_suggest_error}）` : r?.llm_suggestions ? `（LLM 建议 ${r.llm_suggestions.length} 条）` : ''}`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizCandidates() {
  busy.value = true
  try {
    wiz.value.candidates = await request('ziwei', 'setup.candidates', {})
    wizMsg(`✓ 候选 ${wiz.value.candidates?.count} 条——点行内「标注」进入 Step 4b`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizDiff() {
  busy.value = true
  try {
    const wait = Number(wiz.value.waitSecs) || 5
    wiz.value.diff = await request('ziwei', 'setup.diff_window', { wait_secs: wait })
    const pages = wiz.value.diff?.data_pages?.length ?? 0
    wizMsg(`✓ 差分窗口（等 ${wait}s）：变化页 ${wiz.value.diff?.pages?.length ?? 0}，可写数据页 ${pages}（DataArea 线索）`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

function prefillAnnotate(c: any) {
  ann.value = { rva: c.function_rva != null ? String(c.function_rva) : '', semantic: '', kind: c.kind, pattern: c.kind === 'byte_pattern' ? c.payload : '', text: c.kind === 'str_ref' ? c.payload : '', value: c.kind === 'imm_const' ? c.payload : '', imports: c.kind === 'import_call_seq' ? c.payload : '', conv: 'ms_x64', note: '' }
  wiz.value.step = 4
  wizMsg(`已预填候选（${c.kind} @ ${c.function_rva != null ? '0x' + Number(c.function_rva).toString(16).toUpperCase() : '?'}）——补语义名后提交标注`)
}

function annAnchor(): any {
  const k = ann.value.kind
  if (k === 'byte_pattern') return { kind: k, pattern: ann.value.pattern.trim() }
  if (k === 'str_ref') return { kind: k, text: ann.value.text }
  if (k === 'imm_const') return { kind: k, value: Number(BigInt(ann.value.value.trim())) }
  if (k === 'import_call_seq') return { kind: k, imports: ann.value.imports }
  return { kind: k }
}

async function wizSubmitAnnotate() {
  busy.value = true
  try {
    const r = await request('ziwei', 'setup.annotate', {
      rva: Number(BigInt(ann.value.rva.trim())),
      semantic: ann.value.semantic.trim(),
      conv: ann.value.conv || 'ms_x64',
      note: ann.value.note || undefined,
      anchors: [annAnchor()],
    })
    wizMsg(`✓ 已标注 ${r?.annotated}（锚 ${r?.anchors}）——继续下一个函数或进 Step 5`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizDefineStruct() {
  busy.value = true
  try {
    let fields: any
    try {
      fields = JSON.parse(ds.value.fields)
    } catch {
      return wizMsg('✗ fields JSON 解析失败（形如 [{"offset":0,"name":"magic","type":"u32"}]）')
    }
    const r = await request('ziwei', 'setup.define_struct', { name: ds.value.name.trim(), size: Number(ds.value.size), fields })
    if (!wiz.value.structs.includes(r?.struct)) wiz.value.structs.push(r?.struct)
    wizMsg(`✓ 结构 ${r?.struct} 已定义`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizDefineTool() {
  busy.value = true
  try {
    await request('ziwei', 'setup.define_tool', {
      tool_name: dt.value.tool_name.trim(),
      description: dt.value.description.trim(),
      action: { primitive: dt.value.primitive, addr: dt.value.addr.trim(), mode: dt.value.mode },
    })
    if (!wiz.value.tools.includes(dt.value.tool_name.trim())) wiz.value.tools.push(dt.value.tool_name.trim())
    wizMsg(`✓ 工具 ${dt.value.tool_name.trim()} 已定义`)
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizCommit() {
  if (!confirm('提交草稿为正式 profile？（全量校验 + 迁移入库 + 动态工具注册）')) return
  busy.value = true
  try {
    const r = await request('ziwei', 'setup.commit', {})
    wizMsg(`✓ 已提交 ${r?.committed}（函数 ${r?.functions}/工具 ${r?.tools}/锚 ${r?.anchors}，动态工具 ${r?.tools_registered ?? 0} 个）`)
    wizReset()
    await refresh()
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

async function wizAbort() {
  if (!confirm('作废当前向导草稿？（草稿目录与快照一并删除）')) return
  busy.value = true
  try {
    const r = await request('ziwei', 'setup.abort', {})
    wizMsg(`✓ 已作废 ${r?.aborted}`)
    wizReset()
  } catch (e: any) {
    wizMsg(`✗ ${e}`)
  } finally {
    busy.value = false
  }
}

function toolNamesOf(profileId: string): string[] {
  return toolCache.value[profileId] ?? []
}

function statusBadge(s: string): string {
  if (s === 'active') return 'badge-ok'
  if (s === 'stale') return 'badge-warn'
  return 'badge-off'
}
function statusLabel(s: string): string {
  if (s === 'active') return '可用'
  if (s === 'stale') return '已失效'
  if (s === 'revoked') return '已吊销'
  return s
}

onMounted(refresh)
</script>

<template>
  <div class="page ziwei-page">
    <div class="page-header">
      <div>
        <h1>紫微系统</h1>
        <p class="subtitle">三方程序受控接管 —— 无源码、无接口，分析 → 附加 → 采集 → 调用</p>
      </div>
      <button class="btn" :disabled="loading" @click="refresh">刷新</button>
    </div>

    <div v-if="gateClosed" class="banner banner-warn">
      紫微系统未启用（config ziwei.enabled=false）。该功能涉及进入三方进程内部，默认关闭；
      在设置中确认风险后手动开启。
    </div>

    <template v-else>
      <div class="stat-row">
        <div class="stat-card">
          <div class="stat-num">{{ profiles.length }}</div>
          <div class="stat-label">Profile 总数</div>
        </div>
        <div class="stat-card">
          <div class="stat-num ok">{{ activeCount }}</div>
          <div class="stat-label">可用</div>
        </div>
        <div class="stat-card">
          <div class="stat-num warn">{{ staleCount }}</div>
          <div class="stat-label">已失效（待找回）</div>
        </div>
      </div>

      <div v-if="staleCount > 0" class="banner banner-warn">
        <span>有 {{ staleCount }} 个 profile 已失效（目标软件已更新）。可手动引导重配，或授权自主找回（找回结果需逐条确认后才入库）。</span>
        <span class="row-actions">
          <button v-for="p in staleProfiles" :key="p.profile_id" class="btn btn-sm" :disabled="busy" @click="recoverStart(p)">
            授权找回（{{ p.profile_id }}）
          </button>
          <button class="btn btn-sm" @click="wizOpen">引导重配</button>
        </span>
      </div>

      <!-- 找回报告模态（M5 T5.3：逐函数确认入库） -->
      <div v-if="recover.open" class="detail-card" style="border-color: var(--warn, #b58900)">
        <div class="section-title wizard-head">
          授权找回 — {{ recover.profile_id }}
          <button class="btn btn-sm" @click="recover.open = false">关闭</button>
        </div>
        <div class="dim">总判定：{{ recover.verdict }}（勾选要找回的函数；锚未命中的函数不可恢复）</div>
        <table v-if="recover.anchors.length" class="data-table">
          <thead>
            <tr><th></th><th>函数</th><th>锚种类</th><th>命中</th><th>位置</th><th>注记</th></tr>
          </thead>
          <tbody>
            <tr v-for="(a, i) in recover.anchors" :key="i">
              <td><input v-if="a.hit" v-model="recover.accepted[a.function]" type="checkbox" /></td>
              <td>{{ a.function }}</td>
              <td>{{ a.kind }}</td>
              <td><span class="badge" :class="a.hit ? 'badge-ok' : 'badge-warn'">{{ a.hit ? '✓' : '✗' }}</span></td>
              <td class="mono">{{ a.found_rva != null ? '0x' + a.found_rva.toString(16) : '' }}</td>
              <td class="dim">{{ a.note }}</td>
            </tr>
          </tbody>
        </table>
        <div v-if="recover.md" class="dim footnote">报告：{{ recover.md }}</div>
        <div v-if="recover.msg" class="dim">{{ recover.msg }}</div>
        <div class="row-actions">
          <button class="btn btn-sm" :disabled="recover.busy" @click="recoverConfirm">确认入库（恢复 active）</button>
        </div>
      </div>

      <div class="detail-card">
        <div class="section-title wizard-head">
          配置向导（六步）
          <button v-if="!wiz.open" class="btn btn-sm" @click="wizOpen">启动 / 恢复向导</button>
          <button v-else class="btn btn-sm" @click="wiz.open = false">收起</button>
        </div>
        <template v-if="wiz.open">
          <div class="steps dim">① 选目标 → ② 快照 → ③ 分析 → ④ 候选与标注 → ⑤ 结构/工具 → ⑥ 提交</div>
          <div v-if="wiz.msg" class="wiz-msg dim">{{ wiz.msg }}</div>

          <div class="step-block">
            <div class="step-name">① 选目标</div>
            <div class="inline-form">
              <input v-model="wiz.target" class="mono" placeholder="exe 名（如 dummy.exe）或 PID" />
              <button class="btn btn-sm" :disabled="busy" @click="wizSelectTarget">选择目标</button>
              <span v-if="wiz.info?.profile_id" class="mono dim">
                草稿 {{ wiz.info.profile_id }} · pid {{ wiz.info.pid }} · {{ wiz.info.exe_name }} · sha {{ (wiz.info.file_sha256 || '').slice(0, 8) }}
              </span>
            </div>
          </div>

          <div v-if="wiz.info?.profile_id" class="step-block">
            <div class="step-name">②③ 快照 + 分析</div>
            <div class="inline-form">
              <button class="btn btn-sm" :disabled="busy" @click="wizSnapshot">快照并分析</button>
              <button class="btn btn-sm" :disabled="busy" @click="wizAnalyze">仅重跑分析</button>
            </div>
            <div v-if="wiz.analysis" class="dim small">
              函数 {{ wiz.analysis.functions }} · CRT 折叠 {{ wiz.analysis.crt_folded }} · 导入 DLL {{ wiz.analysis.import_dlls }} · 字符串 {{ wiz.analysis.strings }}
              <div v-if="wiz.analysis.llm_suggestions?.length" class="llm-sugg">
                LLM 建议：
                <span v-for="s in wiz.analysis.llm_suggestions" :key="s.rva" class="mono sugg-chip" @click="prefillAnnotate({ kind: 'byte_pattern', function_rva: parseInt(s.rva, 16), payload: '' })">{{ s.rva }} {{ s.name }}</span>
              </div>
            </div>
          </div>

          <div v-if="wiz.step >= 4" class="step-block">
            <div class="step-name">④ 候选与标注</div>
            <div class="inline-form">
              <button class="btn btn-sm" :disabled="busy" @click="wizCandidates">拉取候选</button>
              <label class="dim">diff 等待 <input v-model="wiz.waitSecs" class="mono tiny" /> 秒</label>
              <button class="btn btn-sm" :disabled="busy" @click="wizDiff">差分窗口</button>
            </div>
            <table v-if="wiz.candidates?.candidates?.length" class="data-table">
              <thead><tr><th>KIND</th><th>函数 RVA</th><th>载荷</th><th></th></tr></thead>
              <tbody>
                <tr v-for="(c, i) in wiz.candidates.candidates" :key="i">
                  <td>{{ c.kind }}</td>
                  <td class="mono">{{ c.function_rva != null ? '0x' + Number(c.function_rva).toString(16).toUpperCase() : '—' }}</td>
                  <td class="mono payload">{{ c.payload }}</td>
                  <td><button class="btn btn-sm" @click="prefillAnnotate(c)">标注</button></td>
                </tr>
              </tbody>
            </table>
            <div v-if="wiz.diff?.data_pages?.length" class="small dim">
              数据页变化（DataArea 线索）：
              <span v-for="(p, i) in wiz.diff.data_pages" :key="i" class="mono sugg-chip">{{ p.section }} @0x{{ Number(p.rva).toString(16).toUpperCase() }}（{{ p.bytes_changed }}B）</span>
            </div>
            <div class="ann-form">
              <div class="inline-form">
                <label>RVA <input v-model="ann.rva" class="mono tiny" placeholder="0x2A10" /></label>
                <label>语义名 <input v-model="ann.semantic" class="mono" placeholder="push_record" /></label>
                <label>锚
                  <select v-model="ann.kind">
                    <option value="byte_pattern">byte_pattern</option>
                    <option value="str_ref">str_ref</option>
                    <option value="imm_const">imm_const</option>
                    <option value="import_call_seq">import_call_seq</option>
                  </select>
                </label>
                <input v-if="ann.kind === 'byte_pattern'" v-model="ann.pattern" class="mono wide" placeholder="B8 8D 7C 6B 5A" />
                <input v-if="ann.kind === 'str_ref'" v-model="ann.text" class="mono wide" placeholder="ZIWEI_DUMMY_DRAW_V1" />
                <input v-if="ann.kind === 'imm_const'" v-model="ann.value" class="mono" placeholder="0x5A6B7C8D" />
                <input v-if="ann.kind === 'import_call_seq'" v-model="ann.imports" class="mono wide" placeholder="CreateCompatibleDC,DeleteDC" />
                <button class="btn btn-sm" :disabled="busy" @click="wizSubmitAnnotate">提交标注</button>
              </div>
            </div>
          </div>

          <div v-if="wiz.step >= 4" class="step-block">
            <div class="step-name">⑤ 结构 / 工具定义</div>
            <div class="inline-form">
              <label>结构名 <input v-model="ds.name" class="mono" placeholder="DataHeader" /></label>
              <label>大小 <input v-model="ds.size" class="mono tiny" placeholder="24" /></label>
              <label>字段 JSON <input v-model="ds.fields" class="mono xl" placeholder='[{"offset":0,"name":"magic","type":"u32"}]' /></label>
              <button class="btn btn-sm" :disabled="busy" @click="wizDefineStruct">定义结构</button>
            </div>
            <div class="inline-form">
              <label>工具名 <input v-model="dt.tool_name" class="mono" placeholder="dummy_area_read" /></label>
              <label>描述 <input v-model="dt.description" class="mono xl" placeholder="读 DataHeader+记录" /></label>
              <label>原语
                <select v-model="dt.primitive">
                  <option value="poll_report">poll_report</option>
                  <option value="call_fn">call_fn</option>
                  <option value="hook_capture">hook_capture</option>
                </select>
              </label>
              <label>地址 <input v-model="dt.addr" class="mono" placeholder="struct:DataHeader / fn:push_record" /></label>
              <button class="btn btn-sm" :disabled="busy" @click="wizDefineTool">定义工具</button>
            </div>
            <div v-if="wiz.structs.length || wiz.tools.length" class="small dim">
              本页已定义：结构 {{ wiz.structs.join(', ') || '—' }} · 工具 {{ wiz.tools.join(', ') || '—' }}
            </div>
          </div>

          <div v-if="wiz.info?.profile_id" class="step-block">
            <div class="step-name">⑥ 提交</div>
            <div class="inline-form">
              <button class="btn btn-sm" :disabled="busy" @click="wizCommit">提交为正式 profile</button>
              <button class="btn btn-sm danger" :disabled="busy" @click="wizAbort">作废草稿</button>
            </div>
          </div>
        </template>
      </div>

      <div class="section-title">Profile 列表</div>
      <div class="inline-form">
        <label>目标 PID <input v-model="attachPid" class="mono" placeholder="如 12345" /></label>
        <label>结构地址 JSON（可选，M3 起锚解析）
          <input v-model="attachAddrJson" class="mono wide" placeholder='{"MonitorState": "0x..."}' />
        </label>
        <button class="btn btn-sm" :disabled="busy" @click="refreshTools">重注册动态工具</button>
        <span v-if="sessionMsg" class="dim">{{ sessionMsg }}</span>
      </div>
      <table class="data-table">
        <thead>
          <tr><th>ID</th><th>状态</th><th>名称</th><th>目标</th><th>架构</th><th>子工具</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-if="!profiles.length">
            <td colspan="7" class="empty">暂无 profile。用上方配置向导从目标进程生成一个。</td>
          </tr>
          <tr v-for="p in profiles" :key="p.profile_id">
            <td class="mono">{{ p.profile_id }}</td>
            <td><span class="badge" :class="statusBadge(p.status)">{{ statusLabel(p.status) }}</span></td>
            <td>{{ p.name }}</td>
            <td class="mono">{{ p.exe_name }}</td>
            <td>{{ p.machine }}</td>
            <td>{{ p.tool_count }}</td>
            <td class="row-actions">
              <button class="btn btn-sm" @click="showProfile(p)">详情</button>
              <button class="btn btn-sm" :disabled="busy" @click="verifyProfile(p)">验证</button>
              <button v-if="!sessionOf(p.profile_id)" class="btn btn-sm" :disabled="busy" @click="attachProfile(p)">附加</button>
              <button v-else class="btn btn-sm warn" :disabled="busy" @click="detachSession(sessionOf(p.profile_id))">卸载</button>
              <button class="btn btn-sm danger" :disabled="busy" @click="removeProfile(p)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="sessions.length" class="section-title">活动会话</div>
      <table v-if="sessions.length" class="data-table">
        <thead>
          <tr><th>PROFILE</th><th>状态</th><th>架构</th><th>PID</th><th>目标</th><th>子工具调用</th></tr>
        </thead>
        <tbody>
          <tr v-for="s in sessions" :key="s.profile_id">
            <td class="mono">{{ s.profile_id }}</td>
            <td><span class="badge" :class="s.state === 'ready' ? 'badge-ok' : 'badge-warn'">{{ s.state }}</span></td>
            <td>{{ s.arch }}</td>
            <td class="mono">{{ s.pid }}</td>
            <td class="mono">{{ s.target_exe }}</td>
            <td class="row-actions">
              <button
                v-for="toolName in toolNamesOf(s.profile_id)"
                :key="toolName"
                class="btn btn-sm"
                :disabled="busy"
                @click="invokeTool({ profile_id: s.profile_id }, toolName)"
              >{{ toolName }}</button>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="invokeSnapshot" class="detail-card">
        <div class="section-title">最近调用快照 <span class="mono dim">{{ invokeSnapshot.tool }}</span></div>
        <pre class="snapshot">{{ JSON.stringify(invokeSnapshot, null, 2) }}</pre>
      </div>

      <div v-if="selected" class="detail-card">
        <div class="section-title">
          {{ selected.name }}
          <span class="mono dim">{{ selected.profile_id }}</span>
        </div>
        <div class="kv"><span class="k">目标</span><span class="mono">{{ selected.target?.exe_name }}（{{ selected.target?.machine }}）</span></div>
        <div class="kv"><span class="k">函数</span><span>{{ selected.functions?.length ?? 0 }} 个</span></div>
        <div class="kv"><span class="k">数据结构</span><span>{{ selected.data_structures?.length ?? 0 }} 个</span></div>
        <div class="kv"><span class="k">子工具</span><span>{{ selected.tools?.length ?? 0 }} 个</span></div>
        <div v-if="selectedNotes" class="notes"><pre>{{ selectedNotes }}</pre></div>
      </div>

      <div v-if="status" class="dim footnote">
        配置期引擎：{{ status.setup_engine?.available ? '可用' : (status.setup_engine?.note || '未装配') }}
        · 会话管理器：{{ status.manager_present ? '已装配' : '未装配' }}
        · poll 报警经总线出站（【紫微系统】前缀），capture 回流保留 {{ 7 }} 天
      </div>
    </template>
  </div>
</template>

<style scoped>
.ziwei-page { display: flex; flex-direction: column; gap: 16px; padding: 16px; overflow-y: auto; }
.page-header { display: flex; justify-content: space-between; align-items: center; }
.subtitle { color: var(--text-secondary); margin-top: 4px; font-size: 13px; }
.banner { padding: 10px 14px; border-radius: 8px; font-size: 13px; }
.banner-warn { background: rgba(230, 162, 60, 0.12); border: 1px solid rgba(230, 162, 60, 0.4); color: var(--text-primary); }
.stat-row { display: flex; gap: 12px; }
.stat-card { flex: 1; padding: 12px 16px; border: 1px solid var(--border-color); border-radius: 8px; }
.stat-num { font-size: 24px; font-weight: 600; }
.stat-num.ok { color: var(--success, #34c77b); }
.stat-num.warn { color: var(--warning, #e6a23c); }
.stat-label { color: var(--text-secondary); font-size: 12px; margin-top: 2px; }
.section-title { font-weight: 600; margin: 8px 0; }
.data-table { width: 100%; border-collapse: collapse; font-size: 13px; }
.data-table th, .data-table td { text-align: left; padding: 8px 10px; border-bottom: 1px solid var(--border-color); }
.data-table .empty { color: var(--text-secondary); text-align: center; padding: 24px; }
.row-actions { white-space: nowrap; }
.badge { padding: 2px 8px; border-radius: 10px; font-size: 12px; }
.badge-ok { background: rgba(52, 199, 123, 0.15); color: var(--success, #34c77b); }
.badge-warn { background: rgba(230, 162, 60, 0.15); color: var(--warning, #e6a23c); }
.badge-off { background: var(--border-color); color: var(--text-secondary); }
.detail-card { border: 1px solid var(--border-color); border-radius: 8px; padding: 12px 16px; }
.kv { display: flex; gap: 12px; padding: 3px 0; font-size: 13px; }
.kv .k { width: 72px; color: var(--text-secondary); }
.mono { font-family: var(--font-mono, monospace); }
.dim { color: var(--text-secondary); }
.footnote { font-size: 12px; }
.notes pre { background: var(--bg-secondary, rgba(128,128,128,0.08)); padding: 8px; border-radius: 6px; font-size: 12px; white-space: pre-wrap; }
.inline-form { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; font-size: 13px; }
.inline-form label { display: flex; align-items: center; gap: 6px; color: var(--text-secondary); }
.inline-form input { padding: 4px 8px; border: 1px solid var(--border-color); border-radius: 6px; background: var(--bg-primary, transparent); color: var(--text-primary); }
.inline-form input.wide { width: 260px; }
.btn.warn { color: var(--warning, #e6a23c); }
.snapshot { background: var(--bg-secondary, rgba(128,128,128,0.08)); padding: 10px; border-radius: 6px; font-size: 12px; overflow-x: auto; max-height: 320px; }
/* 向导（M3 六步） */
.wizard-head { display: flex; align-items: center; gap: 12px; }
.steps { font-size: 12px; margin-bottom: 6px; }
.wiz-msg { font-size: 13px; margin: 4px 0; }
.step-block { border-top: 1px dashed var(--border-color); padding: 8px 0; display: flex; flex-direction: column; gap: 6px; }
.step-name { font-weight: 600; font-size: 13px; }
.small { font-size: 12px; }
.payload { max-width: 360px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.inline-form select { padding: 4px 8px; border: 1px solid var(--border-color); border-radius: 6px; background: var(--bg-primary, transparent); color: var(--text-primary); }
.inline-form input.tiny { width: 90px; }
.inline-form input.xl { width: 320px; }
.inline-form textarea { width: 100%; }
.llm-sugg, .sugg-chip { font-size: 12px; }
.sugg-chip { display: inline-block; padding: 1px 8px; margin: 2px 4px 2px 0; border: 1px solid var(--border-color); border-radius: 10px; cursor: pointer; }
.ann-form { margin-top: 6px; }
</style>

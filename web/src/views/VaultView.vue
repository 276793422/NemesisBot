<script setup lang="ts">
/**
 * 凭据 Vault 管理页（L7c，2026-10-13）——放【配置】组、安全之下。
 *
 * 四操作：列表（vault.list）/ 新增（vault.set）/ 删除（vault.remove）/
 * 解锁测试（vault.unlock）。后端语义由 handlers/vault/{vault.rs,tests.rs}
 * 钉住；本页**永不显示 secret 值**（后端 list/status 本就不回值，新增表单
 * 用 password 输入，解锁测试口令只在本次请求内使用）。
 *
 * 覆盖（轮换）纪律：vault.set 对已存在别名会拒绝并提示 force:true——
 * 前端弹 confirm 后带 force 重发（镜像 CLI --force 纪律）。
 */

import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

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

const lockedBanner = computed(() => {
  if (!status.value?.exists) return ''
  if (status.value.unlocked) return ''
  return 'Vault 已锁定（argon2 模式且进程未带口令环境变量）：列表可用，写入/轮换需要解锁。可用下方「解锁测试」验证口令，或重启 gateway 时带上 NEMESISBOT_VAULT_PASSPHRASE。'
})

async function load() {
  loading.value = true
  try {
    const [st, ls] = await Promise.all([
      request('vault', 'status', {}, 8000),
      request('vault', 'list', {}, 8000),
    ])
    status.value = st ?? null
    entries.value = ls?.entries ?? []
  } catch (e: any) {
    toast.error('加载凭据信息失败: ' + (e?.message || e))
  }
  loading.value = false
}
onMounted(load)

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
    <div class="page-header"><h2>凭据 Vault</h2></div>
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

<script setup lang="ts">
/**
 * 代理设置页（2026-09-17）——放【配置】组、二次开发之下。
 *
 * 信息面 + 编辑面：
 *  - per-model 代理一览（models.proxy_overview 只读总览）；行内编辑走
 *    models.update_field(field=proxy)；默认模型保存后自动跟发 set_default
 *    热切（代理在 provider 构造时消费，不热切就要等重启）。
 *  - 进程环境变量代理（config.proxy.json env 段，2026-10-31 起可编辑；
 *    网关启动时注入进程环境，重启生效）；进程当前读数只读列。
 *  - lane 支持说明（CLI 型 lane 不吃 per-model proxy，走环境变量）。
 */

import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const toast = useToast()
const { request } = useWSAPI()

interface ProxyRow {
  model_name: string
  model: string
  protocol: string
  proxy: string
  is_default: boolean
}

interface EnvSection {
  http_proxy: string
  https_proxy: string
  all_proxy: string
  no_proxy: string
}

function emptyEnv(): EnvSection {
  return { http_proxy: '', https_proxy: '', all_proxy: '', no_proxy: '' }
}

const loading = ref(false)
const rows = ref<ProxyRow[]>([])
const laneSupport = ref<{ lane: string; per_model_proxy: boolean; note?: string }[]>([])
const notes = ref<string[]>([])
// 行内编辑状态：key = model_name
const editing = ref<string | null>(null)
const editValue = ref('')
const saving = ref<string | null>(null)

// config.proxy.json（2026-10-31 代理设置三项改造批B）：env 段可编辑 +
// 进程当前读数只读。
const proxyPath = ref('')
const proxyFileExists = ref(false)
const proxyParseError = ref<string | null>(null)
const envFile = ref<EnvSection>(emptyEnv())
const envProcess = ref<EnvSection>(emptyEnv())
const envSaving = ref(false)

async function load() {
  loading.value = true
  try {
    const resp = await request('models', 'proxy_overview', {}, 8000)
    rows.value = resp?.models ?? []
    laneSupport.value = resp?.lane_support ?? []
    notes.value = resp?.notes ?? []
  } catch (e: any) {
    toast.error('加载代理信息失败: ' + (e?.message || e))
  }
  loading.value = false
}

async function loadProxyFile() {
  try {
    const resp = await request('proxy', 'get', {}, 8000)
    proxyPath.value = resp?.path ?? ''
    proxyFileExists.value = !!resp?.file_exists
    proxyParseError.value = resp?.parse_error ?? null
    const ef = resp?.env_file ?? {}
    envFile.value = {
      http_proxy: ef.http_proxy ?? '',
      https_proxy: ef.https_proxy ?? '',
      all_proxy: ef.all_proxy ?? '',
      no_proxy: ef.no_proxy ?? '',
    }
    const ep = resp?.env_process ?? {}
    envProcess.value = {
      http_proxy: ep.http_proxy ?? '',
      https_proxy: ep.https_proxy ?? '',
      all_proxy: ep.all_proxy ?? '',
      no_proxy: ep.no_proxy ?? '',
    }
  } catch (e: any) {
    toast.error('加载代理配置文件失败: ' + (e?.message || e))
  }
}

onMounted(() => {
  load()
  loadProxyFile()
})

function startEdit(row: ProxyRow) {
  editing.value = row.model_name
  editValue.value = row.proxy || ''
}
function cancelEdit() {
  editing.value = null
  editValue.value = ''
}

async function saveEdit(row: ProxyRow) {
  if (saving.value) return
  const val = editValue.value.trim()
  if (val && !/^(https?|socks|socks5|socks5h):\/\//.test(val)) {
    toast.error('代理必须以 http:// https:// socks:// socks5:// socks5h:// 开头（留空 = 清除）')
    return
  }
  saving.value = row.model_name
  try {
    await request('models', 'update_field', {
      name: row.model_name,
      field: 'proxy',
      value: val,
    }, 8000)
    // 默认模型：代理在 provider 构造时消费——跟发 set_default 同名热切，
    // 让新代理立即生效（非默认模型天然在下次激活时消费新配置）。
    if (row.is_default) {
      await request('models', 'set_default', { name: row.model_name }, 8000)
    }
    toast.success(`「${row.model_name}」代理已保存${row.is_default ? '（已热切生效）' : ''}`)
    cancelEdit()
    await load()
  } catch (e: any) {
    toast.error('保存失败: ' + (e?.message || e))
  }
  saving.value = null
}

const envEntries: { key: keyof EnvSection; label: string }[] = [
  { key: 'http_proxy', label: 'HTTP_PROXY' },
  { key: 'https_proxy', label: 'HTTPS_PROXY' },
  { key: 'all_proxy', label: 'ALL_PROXY' },
  { key: 'no_proxy', label: 'NO_PROXY' },
]

async function saveEnv() {
  if (envSaving.value) return
  for (const e of envEntries) {
    const val = (envFile.value[e.key] || '').trim()
    if (val && e.key !== 'no_proxy' && !/^(https?|socks|socks5|socks5h):\/\//.test(val)) {
      toast.error(`${e.label} 必须以 http:// https:// socks:// socks5:// socks5h:// 开头（留空 = 不设置该项）`)
      return
    }
  }
  envSaving.value = true
  try {
    await request('proxy', 'save', { env: { ...envFile.value } }, 8000)
    toast.success('环境变量代理已保存，重启网关后生效')
    await loadProxyFile()
  } catch (e: any) {
    toast.error('保存失败: ' + (e?.message || e))
  }
  envSaving.value = false
}
</script>

<template>
  <div class="page-proxy-settings">
    <div class="page-header"><h2>代理设置</h2></div>
    <div class="page-body">
      <div style="display: flex; flex-direction: column; gap: var(--space-4);">

        <!-- 说明卡 -->
        <div class="card">
          <div class="card-header"><h3>出站代理</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              每个模型可单独配置出站代理（http/socks5），留空 = 清除模型级代理。
              per-model 代理优先于进程环境变量（HTTP_PROXY 等，reqwest 隐式消费）。
              2026-09-17 起三个 HTTP 协议 lane 全接线——此前模型管理页填的代理是
              「配置可见但执行失效」的死配置（factory 整合期回归，已根修）。
            </p>
          </div>
        </div>

        <!-- 模型代理表 -->
        <div class="card">
          <div class="card-header">
            <h3>模型代理</h3>
            <button class="btn btn-sm" :disabled="loading" @click="load">
              {{ loading ? '刷新中…' : '刷新' }}
            </button>
          </div>
          <div style="padding: var(--space-4);">
            <div v-if="rows.length === 0 && !loading" class="muted">
              尚未配置任何模型——先到「模型管理」添加模型。
            </div>
            <div v-else class="table-wrap">
              <table class="table">
                <thead>
                  <tr>
                    <th>模型</th>
                    <th>协议</th>
                    <th>代理</th>
                    <th style="width: 220px;">操作</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="row in rows" :key="row.model_name">
                    <td>
                      <div style="display: flex; align-items: center; gap: var(--space-2);">
                        <span>{{ row.model_name }}</span>
                        <span v-if="row.is_default" class="badge badge-success">默认</span>
                      </div>
                      <div class="muted" style="font-size: var(--text-xs);">{{ row.model }}</div>
                    </td>
                    <td>
                      <span v-if="row.protocol" class="badge badge-info">{{ row.protocol }}</span>
                      <span v-else class="muted">自动推断</span>
                    </td>
                    <td>
                      <template v-if="editing === row.model_name">
                        <input
                          v-model="editValue"
                          class="form-input"
                          style="width: 100%; min-width: 220px;"
                          placeholder="http://host:port 或 socks5://host:port，留空 = 直连"
                          @keyup.enter="saveEdit(row)"
                          @keyup.esc="cancelEdit"
                        />
                      </template>
                      <template v-else>
                        <span v-if="row.proxy" class="mono">{{ row.proxy }}</span>
                        <span v-else class="muted">未设置</span>
                      </template>
                    </td>
                    <td>
                      <template v-if="editing === row.model_name">
                        <button class="btn btn-sm btn-primary" :disabled="saving === row.model_name" @click="saveEdit(row)">
                          {{ saving === row.model_name ? '保存中…' : '保存' }}
                        </button>
                        <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="cancelEdit">取消</button>
                      </template>
                      <template v-else>
                        <button class="btn btn-sm" @click="startEdit(row)">
                          {{ row.proxy ? '修改' : '设置代理' }}
                        </button>
                        <button
                          v-if="row.proxy"
                          class="btn btn-sm"
                          style="margin-left: var(--space-2);"
                          :disabled="saving === row.model_name"
                          @click="editing = row.model_name; editValue = '';"
                        >清除</button>
                      </template>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p class="form-hint" style="margin-top: var(--space-3);">
              默认模型保存后自动热切（set_default）立即生效；非默认模型在下次激活时生效。
            </p>
          </div>
        </div>

        <!-- 环境变量（config.proxy.json env 段，可编辑；进程读数只读） -->
        <div class="card">
          <div class="card-header">
            <h3>进程环境变量代理</h3>
            <button class="btn btn-sm btn-primary" :disabled="envSaving" @click="saveEnv">
              {{ envSaving ? '保存中…' : '保存' }}
            </button>
          </div>
          <div style="padding: var(--space-4);">
            <p class="muted" style="margin: 0 0 var(--space-3);">
              写入 config.proxy.json 的 env 段，网关启动时注入进程环境——保存后<b>重启网关生效</b>。
              reqwest 对未配置 per-model 代理的请求隐式消费这些变量；右列为当前进程实际读数。
            </p>
            <div v-if="proxyParseError" class="badge badge-warn" style="margin-bottom: var(--space-3);">
              配置文件解析失败：{{ proxyParseError }}（保存将以空配置重建）
            </div>
            <div class="table-wrap">
              <table class="table">
                <thead>
                  <tr><th style="width: 140px;">变量</th><th>文件配置</th><th>进程当前读数</th></tr>
                </thead>
                <tbody>
                  <tr v-for="e in envEntries" :key="e.key">
                    <td class="mono">{{ e.label }}</td>
                    <td>
                      <input
                        v-model="envFile[e.key]"
                        class="form-input"
                        style="width: 100%; min-width: 220px;"
                        placeholder="留空 = 不设置该项"
                        @keyup.enter="saveEnv"
                      />
                    </td>
                    <td>
                      <span v-if="envProcess[e.key]" class="mono">{{ envProcess[e.key] }}</span>
                      <span v-else class="muted">（未设置）</span>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p class="form-hint" style="margin-top: var(--space-3);">
              {{ proxyPath || 'config.proxy.json（工作区 config 目录）' }}{{ proxyFileExists ? '' : '——尚未创建，首次保存自动生成' }}
            </p>
          </div>
        </div>

        <!-- lane 支持 -->
        <div class="card">
          <div class="card-header"><h3>各协议 lane 支持情况</h3></div>
          <div style="padding: var(--space-4);">
            <div class="table-wrap">
              <table class="table">
                <thead>
                  <tr><th>lane</th><th>per-model 代理</th><th>说明</th></tr>
                </thead>
                <tbody>
                  <tr v-for="l in laneSupport" :key="l.lane">
                    <td>{{ l.lane }}</td>
                    <td>
                      <span :class="l.per_model_proxy ? 'badge badge-success' : 'badge badge-warn'">
                        {{ l.per_model_proxy ? '支持' : '走环境变量' }}
                      </span>
                    </td>
                    <td><span v-if="l.note" class="muted">{{ l.note }}</span></td>
                  </tr>
                </tbody>
              </table>
            </div>
            <ul v-if="notes.length" style="margin-top: var(--space-3); padding-left: 18px;">
              <li v-for="(n, i) in notes" :key="i" class="form-hint" style="margin-bottom: 4px;">{{ n }}</li>
            </ul>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
/* 与 board/cluster 组件同款局部工具类（全局样式表不含 .muted/.mono） */
.muted {
  color: var(--text-muted);
}
.mono {
  font-family: var(--font-mono, monospace);
  font-size: 0.92em;
}
</style>

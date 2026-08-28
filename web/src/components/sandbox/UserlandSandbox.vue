<script setup lang="ts">
// G7 (D1/D2/D3)：用户态沙盒 UI（Linux landlock/bwrap / macOS Seatbelt）。
// 仅在 VITE_USERLAND_SANDBOX=1 的构建中被 SandboxView 按需加载
// （Windows/Android 工具链构建 tree-shake 掉，二进制不含此组件）。
//
// 职责切分：开关/刷新等请求逻辑留在 SandboxView（本组件 emit 事件）；
// 自检（sandbox.self_test）是用户态专属能力，由本组件直接持有。
import { ref } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

const props = defineProps<{
  busy: string | null
  executorOn: boolean
  strictOn: boolean
  allowNetwork: boolean
  sandboxReady: boolean
  backends: any[]
  selectedBackend: string | null
  strictHint: string
}>()

const emit = defineEmits<{
  (e: 'refresh'): void
  (e: 'enable-exec'): void
  (e: 'disable-exec'): void
  (e: 'toggle-network'): void
  (e: 'toggle-strict'): void
}>()

const { request } = useWSAPI()

function availText(a: string): string {
  return a === 'full' ? '可用' : a === 'partial' ? '部分可用' : '不可用'
}
function availColor(a: string): string {
  return a === 'full' ? 'var(--success)' : a === 'partial' ? 'var(--warning, orange)' : 'var(--danger, #ef4444)'
}

// --- G7 (D2)：沙盒自检（一次性子进程探针） ---
const selfTesting = ref(false)
const selfTest = ref<any>(null)
const selfTestError = ref('')

async function runSelfTest() {
  selfTesting.value = true
  selfTestError.value = ''
  selfTest.value = null
  try {
    selfTest.value = await request('sandbox', 'self_test', undefined, 0)
  } catch (e: any) {
    selfTestError.value = e?.message ?? String(e)
  } finally {
    selfTesting.value = false
  }
}

// 判定语义：隔离探针（workspace 外写入 / 网络出站）以 blocked=true（已拦截）
// 为目标；对照组（workspace 内写入）以 blocked=false 为目标，拦截即异常。
function isControlCheck(c: any): boolean {
  return String(c?.name ?? '').includes('对照')
}
function checkVerdict(c: any): { text: string; cls: string } {
  if (isControlCheck(c)) {
    return c.blocked
      ? { text: '❌ 异常：对照组被拦', cls: 'chk-bad' }
      : { text: '✅ 正常', cls: 'chk-good' }
  }
  return c.blocked
    ? { text: '✅ 已拦截', cls: 'chk-good' }
    : { text: '⚠️ 允许（隔离未覆盖）', cls: 'chk-warn' }
}
</script>

<template>
  <!-- 刷新 -->
  <div style="display: flex; justify-content: flex-end; margin-bottom: var(--space-3);">
    <button class="btn btn-sm" data-test="userland-refresh" @click="emit('refresh')" :disabled="!!props.busy || selfTesting">刷新</button>
  </div>

  <!-- 联动开关（同 Windows 启动/停止按钮模式：enabled+sandbox 一起翻） -->
  <div class="card" style="margin-bottom: var(--space-3);">
    <div class="card-header"><h3 style="margin: 0;">执行沙盒（用户态）</h3></div>
    <div class="card-body">
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
        <div>
          <span style="font-weight: 500;">沙盒执行</span>
          <span
            style="margin-left: var(--space-2); font-size: var(--text-sm);"
            :style="{ color: executorOn ? 'var(--success)' : 'var(--text-secondary)' }"
          >{{ executorOn ? '● 已启用' : '○ 已停用' }}</span>
        </div>
        <div style="display: flex; gap: var(--space-2);">
          <button v-if="!executorOn" class="btn btn-sm btn-primary" @click="emit('enable-exec')" :disabled="!!props.busy">启用沙盒执行</button>
          <button v-else class="btn btn-sm btn-danger" @click="emit('disable-exec')" :disabled="!!props.busy">停用沙盒执行</button>
        </div>
      </div>
      <div style="font-size: var(--text-xs); color: var(--text-secondary);">
        联动开关：executor.enabled + executor.sandbox 一起翻（与 Windows 启动/停止按钮同模式）。executor.enabled 需重启 Agent 生效；sandbox 对后续工具调用实时生效。
      </div>
    </div>
  </div>

  <!-- 后端探测（landlock / bwrap 逐个探测 + 实际选用） -->
  <div class="card" style="margin-bottom: var(--space-3);">
    <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h3 style="margin: 0;">后端探测</h3>
      <span style="font-size: var(--text-sm); color: var(--text-secondary);">
        实际选用：<span :style="{ color: selectedBackend ? 'var(--success)' : 'var(--danger, #ef4444)' }">{{ selectedBackend ?? '无（沙盒不可用）' }}</span>
      </span>
    </div>
    <div class="card-body" style="display: flex; flex-direction: column; gap: var(--space-2); font-size: var(--text-sm);">
      <div v-for="b in backends" :key="b.name" style="display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap;">
        <span style="font-weight: 500; min-width: 96px;">{{ b.name }}</span>
        <span style="font-size: var(--text-xs); color: var(--text-secondary);">({{ b.form === 'SelfApply' ? '进程内自装' : '包装命令' }})</span>
        <span :style="{ color: availColor(b.availability), fontSize: 'var(--text-sm)' }">{{ availText(b.availability) }}</span>
        <span v-if="b.detail?.length" style="color: var(--text-secondary); font-size: var(--text-xs);">{{ b.detail.join('；') }}</span>
        <span v-if="selectedBackend === b.name" style="font-size: var(--text-xs); color: var(--accent);">✓ 已选用</span>
      </div>
      <div v-if="!backends.length" style="color: var(--text-secondary);">本平台无用户态沙盒后端（探测为空）。</div>
    </div>
  </div>

  <!-- G7 (D2)：沙盒自检 —— 一次性子进程实测隔离真的拦得住吗 -->
  <div class="card" style="margin-bottom: var(--space-3);">
    <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h3 style="margin: 0;">沙盒自检</h3>
      <button class="btn btn-sm btn-primary" data-test="run-selftest" @click="runSelfTest" :disabled="selfTesting || !!props.busy">
        {{ selfTesting ? '自检中…' : '运行自检' }}
      </button>
    </div>
    <div class="card-body" style="font-size: var(--text-sm);">
      <div style="color: var(--text-secondary); margin-bottom: var(--space-2);">
        在一次性子进程内实测三条探针：workspace 外写入 / 网络出站 / workspace 内写入（对照组）。
        探针在子进程内自装沙盒（landlock）或被后端包装（bwrap / Seatbelt）后运行——<b>绝不改动网关进程自身</b>。
      </div>

      <!-- 请求失败 -->
      <div v-if="selfTestError" class="selftest-err" data-test="selftest-error">自检失败：{{ selfTestError }}</div>

      <!-- 不支持（Windows / 无用户态后端）-->
      <template v-if="selfTest && selfTest.supported === false">
        <div class="selftest-note" data-test="selftest-unsupported">
          {{ selfTest.note || '本机无可用用户态沙盒后端，无法自检。' }}
        </div>
      </template>

      <!-- 自检结果 -->
      <template v-if="selfTest && selfTest.supported">
        <div style="display: flex; gap: var(--space-4); flex-wrap: wrap; margin-bottom: var(--space-2); font-size: var(--text-xs); color: var(--text-secondary);">
          <span>后端：<b style="color: var(--text-primary, inherit);">{{ selfTest.backend }}</b></span>
          <span>方式：{{ selfTest.form === 'self_apply' ? '子进程内自装' : '包装命令' }}</span>
          <span>allow_network：{{ selfTest.allow_network ? '开' : '关' }}</span>
          <span :style="{ color: selfTest.probe_ok ? 'var(--success)' : 'var(--danger, #ef4444)' }">
            {{ selfTest.probe_ok ? '探针进程正常退出' : '探针进程异常' }}
          </span>
        </div>
        <div v-if="selfTest.error" class="selftest-err">探针进程报错：{{ selfTest.error }}</div>
        <table v-if="selfTest.checks?.length" class="selftest-table" data-test="selftest-checks">
          <thead>
            <tr><th>探针</th><th>结果</th><th>证据</th></tr>
          </thead>
          <tbody>
            <tr v-for="(c, i) in selfTest.checks" :key="i">
              <td>{{ c.name }}</td>
              <td><span :class="checkVerdict(c).cls">{{ checkVerdict(c).text }}</span></td>
              <td class="selftest-evidence">{{ c.evidence }}</td>
            </tr>
          </tbody>
        </table>
        <div style="font-size: var(--text-xs); color: var(--text-secondary); margin-top: var(--space-2);">
          判定基准：隔离探针以「已拦截」为目标（网络出站对 landlock 后端「允许」是已知能力缺口，如实展示）；对照组以「正常」为目标。
        </div>
      </template>

      <!-- D3：安装指引常驻折叠行（未跑自检时也可展开） -->
      <details class="selftest-guide" data-test="install-guide">
        <summary>后端安装指引（bwrap / landlock / seatbelt）</summary>
        <ul>
          <li><b>bwrap（bubblewrap）</b>：<code>sudo apt install bubblewrap</code>（Debian/Ubuntu）或 <code>sudo dnf install bubblewrap</code>（Fedora）</li>
          <li><b>landlock</b>：内核 ≥ 5.13 且 LSM 启用（<code>lsm=</code> 启动参数需含 landlock）；无包可装，内核满足即自动可用</li>
          <li><b>seatbelt</b>：macOS 内置（sandbox-exec），无需安装</li>
        </ul>
      </details>
    </div>
  </div>

  <!-- 沙盒内联网（Linux 无 Sandboxie.ini 副作用，直接走 set_config） -->
  <div class="card" style="margin-bottom: var(--space-3);">
    <div class="card-header"><h3 style="margin: 0;">沙盒内联网</h3></div>
    <div class="card-body">
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
        <span style="font-weight: 500;">executor.allow_network</span>
        <button
          class="btn btn-sm"
          :class="{ 'btn-primary': allowNetwork }"
          @click="emit('toggle-network')"
          :disabled="!!props.busy"
        >
          {{ allowNetwork ? '已开启' : '已关闭' }}
        </button>
      </div>
      <div style="font-size: var(--text-xs); color: var(--text-secondary);">
        {{ allowNetwork ? '沙盒内程序允许联网（需后端支持网络隔离才有意义：bwrap --unshare-net / Seatbelt deny network）' : '沙盒内程序禁止联网（bwrap --unshare-net / Seatbelt deny network；landlock 本身不覆盖网络）' }}
      </div>
    </div>
  </div>

  <!-- P5-2 严格模式 — 按钮旁写清 landlock/bwrap 探测结果 -->
  <div class="card" style="margin-bottom: var(--space-3);">
    <div class="card-header"><h3 style="margin: 0;">严格模式（fail-closed）</h3></div>
    <div class="card-body">
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
        <span style="font-weight: 500;">executor.strict</span>
        <button
          class="btn btn-sm"
          :class="{ 'btn-primary': strictOn }"
          @click="emit('toggle-strict')"
          :disabled="!!props.busy"
        >
          {{ strictOn ? '已开启' : '已关闭' }}
        </button>
      </div>
      <div style="font-size: var(--text-sm); color: var(--text-secondary); margin-bottom: var(--space-1);">
        {{ strictOn
          ? '要求沙盒的调用：后端不可用即拒绝执行（不静默降级）'
          : '要求沙盒的调用：后端不可用时降级为无盒执行（warn，现状默认）' }}
      </div>
      <div style="font-size: var(--text-xs); color: var(--text-secondary);">{{ strictHint }}</div>
    </div>
  </div>

  <!-- 诚实说明卡（机制边界，如实写明） -->
  <div class="card">
    <div class="card-header"><h3 style="margin: 0;">机制说明（诚实边界）</h3></div>
    <div class="card-body" style="font-size: var(--text-sm); color: var(--text-secondary);">
      <ul style="margin: 0; padding-left: var(--space-5); display: flex; flex-direction: column; gap: var(--space-1);">
        <li>文件系统隔离 = 内核强制（Linux: landlock；macOS: Seatbelt 配置文件）</li>
        <li>网络隔离需要 bwrap（landlock 不覆盖网络；Seatbelt 经 deny network 规则）</li>
        <li>无「写捕获 / 待提交」模型：越界写 = <b>直接拒绝</b>，不是关进盒等手动提交（与 Windows Sandboxie 语义不同）</li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.selftest-err {
  color: var(--danger, #ef4444);
  font-size: var(--text-sm);
  margin-bottom: var(--space-2);
}
.selftest-note {
  color: var(--text-secondary);
  background: var(--bg-secondary, rgba(0, 0, 0, 0.04));
  border-left: 3px solid var(--warning, orange);
  padding: var(--space-2) var(--space-3);
  border-radius: 4px;
  margin-bottom: var(--space-2);
}
.selftest-guide {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  margin-top: var(--space-2);
}
.selftest-guide summary {
  cursor: pointer;
  user-select: none;
}
.selftest-guide ul {
  margin: var(--space-2) 0 0;
  padding-left: var(--space-5);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}
.selftest-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-xs);
}
.selftest-table th,
.selftest-table td {
  text-align: left;
  padding: var(--space-1) var(--space-2);
  border-bottom: 1px solid var(--border);
  vertical-align: top;
}
.selftest-evidence {
  color: var(--text-secondary);
  font-family: var(--font-mono);
  word-break: break-all;
}
.chk-good { color: var(--success); font-weight: 500; }
.chk-warn { color: var(--warning, orange); font-weight: 500; }
.chk-bad { color: var(--danger, #ef4444); font-weight: 500; }
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; }
</style>

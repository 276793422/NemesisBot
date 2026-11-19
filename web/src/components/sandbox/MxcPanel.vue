<script setup lang="ts">
/**
 * MxcPanel — MXC 护栏后端面板（2026-11-14，docs/PLAN/2026-11-14_mxc-sandbox-integration.md
 * D11-D13）。Windows 在 MXC 顶层 TAB 内、Linux/macOS 平铺复用同一组件。
 *
 * - 状态卡：默认极简（✓/✗ + 一句话原因），「详情」展开能力面（D11）
 * - 「重新检测」= sandbox.mxc_probe（绕缓存 fresh，D13）
 * - ⚠️ 警告横幅常驻（D5 文案单一真相源在后端，本组件零硬编码）
 * - 配置区：网络 open/locked + egress 白名单（仅 mxc 后端生效时；native
 *   模式置灰——切换回 native 时配置仍可看不可改）
 * - 未编译 = 置灰「未编译」卡（D12 诚实告知，不隐藏）
 */
import { ref, computed, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const props = defineProps<{
  mxc: any | null
  busy: string | null
  /** 当前生效后端（native 模式下配置区置灰——策略仅 mxc 后端消费） */
  currentBackend: string
  /** 隐藏组件内警告横幅（Windows 挂点用——父视图 TAB 行下已挂常驻横幅，
   * 两处同渲染会在 MXC TAB 上叠两层同文案卡；Linux/macOS 平铺挂点保持
   * 默认 false——组件内横幅是那边唯一的警告落点） */
  hideWarning?: boolean
}>()

const { request } = useWSAPI()
const toast = useToast()
const emit = defineEmits<{ (e: 'probe'): void }>()

const mxcCompiled = computed(() => !!props.mxc?.compiled)
const probe = computed<any>(() => props.mxc?.probe ?? null)
const ready = computed(() => probe.value?.status === 'ready')
const warning = computed(() => props.mxc?.warning ?? '')
const detailOpen = ref(false)

// 配置区草稿（父视图 overview.mxc.config 变化时同步）
const networkDraft = ref('open')
const egressDraft = ref('')
watch(
  () => props.mxc?.config,
  (c: any) => {
    networkDraft.value = c?.network ?? 'open'
    egressDraft.value = (c?.egress_allow ?? []).join('\n')
  },
  { immediate: true },
)

async function probeNow() {
  emit('probe')
}

async function savePolicy() {
  // open 模式**不动** allowlist（审查二轮发现：整包发空数组会把已存的
  // 白名单静默抹掉——切 open 只是「暂不消费」，不是「删除」）。
  const payload =
    networkDraft.value === 'locked'
      ? {
          mxc_network: 'locked',
          mxc_egress_allow: egressDraft.value
            .split('\n')
            .map((s: string) => s.trim())
            .filter(Boolean),
        }
      : { mxc_network: 'open' }
  try {
    await request('sandbox', 'set_config', payload)
    toast.success('MXC 网络策略已保存 · 下次 MXC spawn 即生效')
  } catch (e: any) {
    toast.error('保存失败: ' + (e?.message ?? e))
  }
}
</script>

<template>
  <!-- 未编译：置灰诚实卡（D12） -->
  <div v-if="!mxcCompiled" class="card" data-test="mxc-not-compiled" style="opacity: 0.6;">
    <div class="card-header"><h3 style="margin: 0;">MXC（微软执行容器 · 护栏）</h3></div>
    <div class="card-body" style="color: var(--text-secondary); font-size: var(--text-sm);">
      本构建未编译 <code>sandbox-mxc</code> feature——MXC 后端不可选不可用（配置
      <code>executor.backend="mxc"</code> 会诚实回落原生路径）。完整桌面构建默认包含。
    </div>
  </div>

  <template v-else>
    <!-- ⚠️ 常驻警告（D5：护栏非沙盒定性；hideWarning 时由父视图横幅承担） -->
    <div v-if="!hideWarning" class="card" style="padding: var(--space-3) var(--space-4); border-color: var(--warning, #d97706); background: rgba(217,119,6,0.06);" data-test="mxc-warning-card">
      <span style="font-size: var(--text-sm); color: var(--warning, #d97706);">⚠️ {{ warning }}</span>
    </div>

    <!-- 状态卡（D11 极简 + 详情展开；D13 重新检测） -->
    <div class="card" data-test="mxc-status-card">
      <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
        <h3 style="margin: 0;">检测状态 <span style="font-size: var(--text-xs); font-weight: 400; color: var(--text-secondary);">引擎 v{{ props.mxc?.version ?? '—' }}</span></h3>
        <button class="btn btn-sm" data-test="mxc-probe" @click="probeNow" :disabled="!!busy">重新检测</button>
      </div>
      <div class="card-body" style="display: flex; flex-direction: column; gap: var(--space-2); font-size: var(--text-sm);">
        <div>
          <span :style="{ color: ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ ready ? '●' : '○' }}</span>
          <span style="margin-left: var(--space-2);" data-test="mxc-status-line">
            {{ ready ? '✓ 可用' : '✗ 不可用' }}（{{ probe?.platform_backend ?? '—' }}）{{ probe?.detail ? ' — ' + probe.detail : '' }}
          </span>
        </div>
        <div>
          <button class="btn btn-sm" data-test="mxc-detail-toggle" @click="detailOpen = !detailOpen">
            {{ detailOpen ? '收起详情' : '详情' }}
          </button>
        </div>
        <div v-if="detailOpen" style="padding: var(--space-2) var(--space-3); background: var(--bg-secondary, rgba(0,0,0,0.03)); border-radius: 4px; font-size: var(--text-xs); color: var(--text-secondary); display: flex; flex-direction: column; gap: var(--space-1);" data-test="mxc-detail">
          <div>探测状态：<code>{{ probe?.status ?? '—' }}</code></div>
          <div>平台后端：<code>{{ probe?.platform_backend ?? '—' }}</code>（Windows=ProcessContainer / Linux=bubblewrap / macOS=seatbelt，按宿主自动解析）</div>
          <div>原因明细：<span>{{ probe?.detail ?? '—' }}</span></div>
          <div>隔离机制：ACL 最小授权——授权路径真写（无写虚拟化）；网络=逐容器 egress 过滤；UI=剪贴板/输入注入全禁</div>
          <div style="color: var(--warning, #d97706);">护栏非沙盒：授权内后果不可撤销。引爆场景（事后审查）请切回 SandboxIE。</div>
        </div>
      </div>
    </div>

    <!-- 配置区（network + egress allowlist；native 模式置灰） -->
    <div class="card" data-test="mxc-config-card">
      <div class="card-header"><h3 style="margin: 0;">MXC 策略</h3></div>
      <div class="card-body" style="display: flex; flex-direction: column; gap: var(--space-3);">
        <div style="display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap;">
          <span style="font-weight: 500;">网络策略</span>
          <select class="form-input" style="max-width: 220px;" data-test="mxc-network" v-model="networkDraft" :disabled="currentBackend !== 'mxc' || !!busy">
            <option value="open">open（全放行 · 与原生一致）</option>
            <option value="locked">locked（deny-default + 白名单）</option>
          </select>
          <span v-if="currentBackend !== 'mxc'" style="font-size: var(--text-xs); color: var(--text-secondary);">
            当前生效后端为 {{ currentBackend }}——本策略仅 MXC 后端消费（置灰只读）
          </span>
        </div>
        <div v-if="networkDraft === 'locked'" style="display: flex; flex-direction: column; gap: var(--space-1);">
          <span style="font-size: var(--text-sm); font-weight: 500;">Egress 白名单（每行一条：<code>cidr</code> 或 <code>cidr:port</code>，缺协议 = tcp）</span>
          <textarea
            class="form-input"
            data-test="mxc-egress"
            rows="4"
            style="font-family: var(--font-mono); max-width: 560px;"
            v-model="egressDraft"
            placeholder="140.82.112.0/20:443&#10;10.0.0.0/8"
            :disabled="currentBackend !== 'mxc' || !!busy"
          ></textarea>
        </div>
        <div style="display: flex; align-items: center; gap: var(--space-3);">
          <button class="btn btn-sm btn-primary" data-test="mxc-policy-save" @click="savePolicy" :disabled="currentBackend !== 'mxc' || !!busy">保存策略</button>
          <span style="font-size: var(--text-xs); color: var(--text-secondary);">策略改动对下一次 MXC spawn 即时生效（无需重启；后端切换才需重启）</span>
        </div>
      </div>
    </div>
  </template>
</template>

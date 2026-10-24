<script setup lang="ts">
// 跨网漫游卡片（2026-10-23 与 QUIC 解耦改版：自独立 TAB 收编为
// 【集群设置】右列顶卡片）——roaming 开关（集群行为：是否消费中继地址
// 组网）+ 名册同步观测（rendezvous 段）。连接参数（自建中继地址/客户端
// 令牌）是 QUIC 层资产，在【通道 → QUIC 中继】页配置，不在此卡。
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const switching = ref(false)

// cluster.roaming.status 载荷。available=false = 管理面未就绪（防御性降级
// 面板；M4 起 feature 已删恒编译）；requires_restart = config 开关与运行态不一致。
const status = ref<any>(null)

const available = computed(() => status.value?.available === true)
const enabled = computed(() => status.value?.enabled === true)
const active = computed(() => status.value?.active === true)
const needsRestart = computed(() => status.value?.requires_restart === true)
const nodeId = computed(() => (status.value?.node_id as string) || '')
const fullAddress = computed(() => (nodeId.value ? `iroh:${nodeId.value}` : ''))

// 名册同步（rendezvous 自动发现）：自建中继形态下接入即报到、周期拉名册
// ——同 token 设备自动互见，无需手工写 peers.toml。Null = 循环未挂（N0
// 公共中继形态 / roaming 未开），段不渲染（诚实降级）。
const rendezvous = computed(() => (status.value?.rendezvous ?? null) as
  | { registry_url: string; last_sync_ok: boolean; last_sync_ms: number; member_count: number; last_error: string }
  | null)

// 集群本体闸链（cluster_gate，加法字段；旧后端缺席 = 视为开，回落旧行为）。
// roaming 的装配在 gateway 启动路径被集群本体三道闸前置——闸未开时「重启网关」
// 永远不生效，横幅不能再说「重启网关后生效」误导用户无限重启（2026-10-22 实录）。
const gateClosed = computed(
  () => status.value?.cluster_gate != null && !status.value.cluster_gate.open,
)
const gateTokenBroken = computed(() => status.value?.cluster_gate?.token_broken === true)
const gateHint = computed(() => {
  if (!enabled.value || active.value) return ''
  if (gateTokenBroken.value) {
    return 'RPC 引用 token 异常（vault 凭据缺失或不一致）——网关启动时集群装配被跳过，漫游层未随之装配。请修复集群 token 后重启网关。'
  }
  if (gateClosed.value) {
    return '集群本体未启用（config.json cluster.enabled 或 config.cluster.json enabled 未开，两道闸）——漫游层随集群装配，仅重启不会生效。请先打开上方「启用集群」并在【集群概览】启动集群（或 CLI nemesisbot cluster enable 一步开全），再重启网关。'
  }
  return ''
})

// 运行态徽标词表：active=漫游层在跑；enabled 但被闸链挡住=集群未启用/token
// 异常（重启无用）；enabled 且闸开但没跑=改了开关没重启；都没有=停用。
const stateLabel = computed(() => {
  if (!available.value) return '未就绪'
  if (active.value) return '运行中'
  if (enabled.value && gateTokenBroken.value) return 'token 异常'
  if (enabled.value && gateClosed.value) return '集群未启用'
  if (enabled.value) return '待重启生效'
  return '未启用'
})
const stateClass = computed(() => {
  if (!available.value) return 'muted'
  if (active.value) return 'pass'
  if (enabled.value) return 'warn'
  return 'muted'
})

async function loadStatus() {
  try {
    status.value = await request('cluster', 'roaming.status')
  } catch (e: any) {
    toast.error('加载漫游状态失败: ' + (e || '未知错误'))
  }
  loading.value = false
}

async function setEnabled(next: boolean) {
  switching.value = true
  try {
    const res = await request('cluster', next ? 'roaming.enable' : 'roaming.disable')
    toast.success(
      next
        ? `跨网漫游已开启${res?.key_created ? '（已生成本节点漫游身份）' : ''}，重启网关后生效`
        : '跨网漫游已关闭，重启网关后生效',
    )
    await loadStatus()
  } catch (e: any) {
    toast.error('操作失败: ' + (e || '未知错误'))
  }
  switching.value = false
}

async function copyAddress() {
  if (!fullAddress.value) return
  try {
    await navigator.clipboard.writeText(fullAddress.value)
    toast.success('完整地址已复制')
  } catch {
    toast.error('复制失败，请手动复制地址文本')
  }
}

onMounted(loadStatus)
</script>

<template>
  <div class="card">
    <div class="card-header">
      <h3>跨网漫游</h3>
      <span class="roam-badge" :class="stateClass">{{ stateLabel }}</span>
    </div>
    <div class="card-body">
      <div v-if="loading" style="text-align:center;padding:var(--space-4)">
        <div class="spinner" style="margin:0 auto" />
      </div>

      <!-- 管理面未就绪：诚实降级（卡片恒在，能力缺席可见；防御性保留） -->
      <div v-else-if="!available" class="roam-summary muted">
        跨网漫游管理面未就绪（管理面恒编译，此形态仅防御性保留）。
      </div>

      <template v-else>
        <div class="form-group">
          <label class="form-label">
            启用跨网漫游
            <span class="form-hint" title="iroh QUIC 叠加层：两台 NAT 后设备无需端口转发即可互连成集群。局域网场景不受影响（TCP 主通路不变）。">ⓘ</span>
          </label>
          <div
            class="toggle"
            :class="{ active: enabled }"
            style="cursor:pointer"
            @click="!switching && setEnabled(!enabled)"
          />
          <div v-if="gateHint" class="roam-restart-hint">
            ⚠ {{ gateHint }}
          </div>
          <div v-else-if="needsRestart" class="roam-restart-hint">
            ⚠ 开关已修改但网关未重启，当前{{ active ? '仍在运行（重启后按新配置）' : '尚未生效' }}。重启网关后生效。
          </div>
        </div>

        <div class="form-group">
          <label class="form-label">
            本节点漫游身份
            <span class="form-hint" title="由 cluster/roaming.key 派生的节点身份，传输层自证（QUIC-TLS 握手即验身）。开启漫游时自动生成。">ⓘ</span>
          </label>
          <template v-if="nodeId">
            <div style="display:flex;gap:var(--space-2);align-items:center">
              <span class="roam-node-id" :title="fullAddress">iroh:{{ nodeId }}</span>
              <button class="btn btn-sm" @click="copyAddress">复制</button>
            </div>
          </template>
          <div v-else class="roam-summary muted">
            尚未生成漫游身份（开启开关后自动生成，或执行
            <code>nemesisbot quic keygen</code>）。
          </div>
        </div>

        <div class="form-group" v-if="rendezvous">
          <label class="form-label">
            名册同步（自动发现）
            <span class="form-hint" title="自建中继形态：接入即向中结名册报到（30s 周期），同令牌设备自动互相发现——无需手工交换地址。名册只供地址，入网仍要过传输身份 + 集群 Token + 信任名单三关。">ⓘ</span>
          </label>
          <div style="display:flex;gap:var(--space-2);align-items:center">
            <span class="roam-badge" :class="rendezvous.last_sync_ok ? 'pass' : 'warn'">
              {{ rendezvous.last_sync_ok ? '已同步' : '同步失败' }}
            </span>
            <span style="font-size:var(--text-sm)">名册 {{ rendezvous.member_count }} 个成员（含本机）</span>
          </div>
          <div class="roam-node-id" :title="rendezvous.registry_url" style="margin-top:var(--space-1);max-width:280px">
            {{ rendezvous.registry_url }}
          </div>
          <div v-if="rendezvous.last_error" class="roam-restart-hint">
            ⚠ {{ rendezvous.last_error }}（限频重试中；静态 peers 不受影响）
          </div>
        </div>

        <div class="form-group">
          <label class="form-label">信任名单</label>
          <template v-if="status?.trust?.enabled">
            <span style="font-size:var(--text-sm)">
              已启用（放行 {{ status.trust.trusted }} 个 / 吊销 {{ status.trust.revoked }} 个）
            </span>
          </template>
          <span v-else style="font-size:var(--text-sm);color:var(--text-secondary)">
            未启用（认证 Token 单层语义）
          </span>
        </div>

        <!-- 对端接入说明（可折叠，默认收起——组网指引随卡片可触达） -->
        <details class="roam-details">
          <summary>对端接入说明（手工组网指引）</summary>
          <div class="roam-steps">
            <div class="roam-step">
              <div class="roam-step-title">1. 双方都开启跨网漫游</div>
              <div class="roam-step-body">
                各自打开本卡开关并重启网关（身份密钥自动生成，无需端口转发、无需公网 IP）。
              </div>
            </div>
            <div class="roam-step">
              <div class="roam-step-title">2. 交换地址（自建中继组网可跳过）</div>
              <div class="roam-step-body">
                所有设备填写同一台自建中继地址（【通道 → QUIC 中继】客户端中继地址）时，接入即自动互见（名册同步），
                <strong>无需本步与下一步</strong>。以下仅 n0 公共中继或手工钉址场景需要——
                把本节点完整地址（上方「复制」按钮）通过任意渠道发给对方。两种写法：
                <pre class="roam-cmd"># 供发现（推荐，走中继自动打洞）
iroh:&lt;对端64位hex身份&gt;

# 直连钉址（已知对端公网出口时）
iroh:&lt;对端64位hex身份&gt;@&lt;ip:port&gt;</pre>
              </div>
            </div>
            <div class="roam-step">
              <div class="roam-step-title">3. 写入对端 peers.toml</div>
              <div class="roam-step-body">
                在对方工作区 <code>cluster/peers.toml</code> 的
                <code>[node]</code> 或 <code>[peers.X]</code> 段把该地址加进
                <code>addresses</code>；也可用 <code>nemesisbot cluster peers add --address iroh:...</code>。
              </div>
            </div>
            <div class="roam-step">
              <div class="roam-step-title">4.（可选）启用信任名单</div>
              <div class="roam-step-body">
                创建 <code>cluster/trust.toml</code> 并填入允许的节点身份，跨网通路即升级为
                「身份 + Token + 名单」三重校验：
                <pre class="roam-cmd">trusted = ["&lt;对方64位hex身份&gt;"]
revoked = []</pre>
              </div>
            </div>
          </div>
        </details>

        <div class="roam-summary" style="margin-top:var(--space-3)">
          自建中继组网（推荐）：中继地址等连接参数在【通道 → QUIC 中继】页配置；
          设备接入名册即自动互见，全程零手工配置。
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.roam-badge {
  font-size: var(--text-xs);
  font-weight: 600;
  padding: 2px 10px;
  border-radius: 999px;
}
.roam-badge.pass {
  background: color-mix(in srgb, var(--success) 12%, transparent);
  color: var(--success);
}
.roam-badge.warn {
  background: color-mix(in srgb, var(--warning, #f39c12) 12%, transparent);
  color: var(--warning, #f39c12);
}
.roam-badge.muted {
  background: var(--surface-alt);
  color: var(--text-secondary);
}

.roam-node-id {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-1) var(--space-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 320px;
  display: inline-block;
}

.roam-restart-hint {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--warning, #f39c12);
}

.roam-summary {
  font-size: var(--text-sm);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--primary) 8%, transparent);
  color: var(--text-primary);
}
.roam-summary.muted {
  background: var(--surface-alt);
  color: var(--text-secondary);
}

.roam-details {
  margin-top: var(--space-2);
  font-size: var(--text-sm);
}
.roam-details summary {
  cursor: pointer;
  color: var(--text-secondary);
  font-size: var(--text-xs);
  user-select: none;
}
.roam-details[open] summary {
  margin-bottom: var(--space-2);
}

.roam-steps {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.roam-step-title {
  font-weight: 600;
  font-size: var(--text-sm);
  margin-bottom: var(--space-1);
}
.roam-step-body {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  line-height: 1.6;
}
.roam-cmd {
  background: var(--surface-alt);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  overflow-x: auto;
  margin-top: var(--space-1);
  white-space: pre;
}
code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  background: var(--surface-alt);
  padding: 1px 5px;
  border-radius: 4px;
}
</style>

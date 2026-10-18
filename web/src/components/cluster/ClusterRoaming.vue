<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const switching = ref(false)

// cluster.roaming.status 载荷。available=false = 网关未编译 cluster-roaming
// feature（降级面板）；requires_restart = config 开关与运行态不一致。
const status = ref<any>(null)

const available = computed(() => status.value?.available === true)
const enabled = computed(() => status.value?.enabled === true)
const active = computed(() => status.value?.active === true)
const needsRestart = computed(() => status.value?.requires_restart === true)
const nodeId = computed(() => (status.value?.node_id as string) || '')
const fullAddress = computed(() => (nodeId.value ? `iroh:${nodeId.value}` : ''))

// 运行态徽标词表：active=漫游层在跑；enabled 但没跑=改了开关没重启；
// 都没有=停用。
const stateLabel = computed(() => {
  if (!available.value) return '未编译'
  if (active.value) return '运行中'
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
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-else>
    <!-- feature 未编译：诚实降级面板（TAB 恒在，能力缺席可见） -->
    <div v-if="!available" class="card">
      <div class="card-header"><h3>跨网漫游</h3></div>
      <div class="card-body">
        <div class="roam-summary muted">
          当前网关未编译跨网漫游能力（cluster-roaming feature 关闭），此面板仅作展示。
        </div>
      </div>
    </div>

    <template v-else>
      <div class="roam-two-col">
        <!-- Left: 开关 + 身份 -->
        <div class="card">
          <div class="card-header">
            <h3>跨网漫游开关</h3>
            <span class="roam-badge" :class="stateClass">{{ stateLabel }}</span>
          </div>
          <div class="card-body">
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
              <div v-if="needsRestart" class="roam-restart-hint">
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
                <div class="form-hint" style="margin-top:var(--space-1)">
                  把这个地址发给对端，或在对方 peers.toml 里声明，即可跨网找到本节点。
                </div>
              </template>
              <div v-else class="roam-summary muted">
                尚未生成漫游身份（开启开关后自动生成，或执行
                <code>nemesisbot cluster roaming keygen</code>）。
              </div>
            </div>

            <div class="form-group">
              <label class="form-label">信任名单</label>
              <template v-if="status?.trust?.enabled">
                <span>
                  已启用（放行 {{ status.trust.trusted }} 个 / 吊销 {{ status.trust.revoked }} 个）
                </span>
                <div class="form-hint" style="margin-top:var(--space-1)">
                  启用信任名单后，只有名单内的节点能通过跨网通路连入（吊销一票否决）。
                </div>
              </template>
              <span v-else style="color:var(--text-secondary)">
                未启用（认证 Token 单层语义；启用方法见右侧说明）
              </span>
            </div>
          </div>
        </div>

        <!-- Right: 对端接入说明 -->
        <div class="card">
          <div class="card-header"><h3>对端接入说明</h3></div>
          <div class="card-body">
            <div class="roam-steps">
              <div class="roam-step">
                <div class="roam-step-title">1. 双方都开启跨网漫游</div>
                <div class="roam-step-body">
                  各自打开本页开关并重启网关（身份密钥自动生成，无需端口转发、无需公网 IP）。
                </div>
              </div>
              <div class="roam-step">
                <div class="roam-step-title">2. 交换地址</div>
                <div class="roam-step-body">
                  把本节点完整地址（左栏「复制」按钮）通过任意渠道发给对方。两种写法：
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
            <div class="roam-summary" style="margin-top:var(--space-3)">
              首次跨网相遇需静态声明地址（本页或 peers.toml）；之后双方通过 announce
              自动携带漫游身份，局域网相遇过一次即可跨网互通。
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.roam-two-col {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-4);
}
@media (max-width: 900px) {
  .roam-two-col {
    grid-template-columns: 1fr;
  }
}

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

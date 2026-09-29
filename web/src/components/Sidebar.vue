<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import { computed, ref, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { useTheme } from '../composables/useTheme'
import { useWSAPI } from '../composables/useWSAPI'
import { useEditorMode } from '../composables/useEditorMode'
import { visibleNavGroups as visibleNavGroupsFiltered } from '../skins/navModel'

const router = useRouter()
const route = useRoute()
const appStore = useAppStore()
const auth = useAuthStore()
const { theme, toggleTheme } = useTheme()
// Full Access 激活徽标（只读呈现；开关本体在 useEditorMode 单例）。
const { fullAccess } = useEditorMode()

function navigate(page: string) {
  router.push(page === 'chat' ? '/' : '/' + page)
  appStore.showMobileSidebar = false
}

function handleLogout() {
  auth.logout()
}

// E-stop (kill switch): reflect + toggle global agent freeze via WSAPI.
// engage/release 操作的是 gateway 里和 agent loop 共享的同一个 EstopState。
const { request } = useWSAPI()
const estopEngaged = ref(false)
const estopBusy = ref(false)
let estopTimer: ReturnType<typeof setInterval> | undefined

async function refreshEstop() {
  try {
    const resp = await request('estop', 'status', {}, 5000)
    estopEngaged.value = !!(resp && resp.engaged)
  } catch {
    // WS 未就绪或后端不可用——保持原状态，下个轮询周期再试。
  }
}
async function toggleEstop() {
  if (estopBusy.value) return
  estopBusy.value = true
  try {
    const cmd = estopEngaged.value ? 'release' : 'trigger'
    const resp = await request('estop', cmd, {}, 5000)
    estopEngaged.value = !!(resp && resp.engaged)
  } catch (e) {
    console.error('[E-Stop] toggle failed:', e)
  } finally {
    estopBusy.value = false
  }
}
// 签名验证状态徽标（2026-09-23 接入计划 §4）：只读呈现 gateway 启动自验
// 快照（进程内不变，取到一次即止）。三色：绿=Valid（锁定版加 🔒）；
// 红=warn 态 Tampered（二进制被篡改，最危险）；黄=warn 其余失败 + off +
// 无锚降级。injected:false（测试/降级装配）不显示。
const sigLoaded = ref(false)
const sigMode = ref('')
const sigLocked = ref(false)
const sigResult = ref<string | null>(null)
const sigKeyFp = ref<string | null>(null)
const sigDetail = ref('')

const sigBadge = computed(() => {
  if (!sigLoaded.value) return null
  if (sigResult.value === 'Valid') {
    return { color: 'ok', label: sigLocked.value ? '🔒 签名已验证' : '签名已验证' }
  }
  if (sigResult.value === 'Tampered') {
    return { color: 'bad', label: '签名已被篡改' }
  }
  if (sigMode.value === 'off') return { color: 'warn', label: '签名验证 关' }
  return { color: 'warn', label: '签名验证异常' }
})

const sigTitle = computed(() => {
  const parts = [`模式: ${sigMode.value}${sigLocked.value ? '（锁定版，config 不可关）' : ''}`]
  if (sigResult.value) parts.push(`启动自验: ${sigResult.value}`)
  if (sigKeyFp.value) parts.push(`签名者: ${sigKeyFp.value}`)
  if (sigDetail.value) parts.push(sigDetail.value)
  return parts.join('\n')
})

async function refreshSigVerify() {
  if (sigLoaded.value) return
  try {
    const resp: any = await request('security', 'signature_verify_status', {}, 5000)
    if (!resp || !resp.injected) return // 测试/降级装配：不显示，下个轮询周期再试无害
    sigMode.value = resp.mode ?? ''
    sigLocked.value = !!resp.locked
    sigResult.value = resp.last_result ?? null
    sigKeyFp.value = resp.key_fp ?? null
    sigDetail.value = resp.detail ?? ''
    sigLoaded.value = true
  } catch {
    // WS 未就绪——保持未加载，下个轮询周期再试。
  }
}

onMounted(() => {
  refreshEstop()
  refreshSigVerify()
  estopTimer = setInterval(() => {
    refreshEstop()
    refreshSigVerify() // 已加载即 no-op；状态进程内不变，无需独立计时器
  }, 10000)
})
onUnmounted(() => {
  if (estopTimer) clearInterval(estopTimer)
})

// 导航分组清单 + feature 门控已上收 skins/navModel.ts（P2b 契约层单一
// 真相源）：Sidebar 与 window.NemesisSkin.nav.model() 消费同一份模型，
// 两处列表漂移 = 契约测试红。这里只保留响应式投影。
const visibleNavGroups = computed(visibleNavGroupsFiltered)
</script>

<template>
  <aside class="sidebar" data-nb-shell="sidebar" :class="{ collapsed: appStore.sidebarCollapsed, 'mobile-open': appStore.showMobileSidebar }">
    <div class="sidebar-header">
      <div class="sidebar-logo">
        <svg class="sidebar-logo-icon" width="20" height="20" viewBox="0 0 256 256" fill="none" xmlns="http://www.w3.org/2000/svg">
          <g transform="translate(8, 18)">
            <line x1="120" y1="30" x2="120" y2="5" stroke="#2C3E50" stroke-width="4" stroke-linecap="round"/>
            <circle cx="120" cy="5" r="6" fill="#FF4D4D"/>
            <circle cx="117" cy="3" r="2" fill="#FFF" opacity="0.6"/>
            <rect x="68" y="30" width="104" height="80" rx="15" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="53" y="55" width="15" height="30" rx="5" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="172" y="55" width="15" height="30" rx="5" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <circle cx="95" cy="60" r="9" fill="#2C3E50"/>
            <circle cx="145" cy="60" r="9" fill="#2C3E50"/>
            <circle cx="92" cy="57" r="3" fill="#FFF"/>
            <circle cx="142" cy="57" r="3" fill="#FFF"/>
            <circle cx="82" cy="80" r="5" fill="#FF6B6B" opacity="0.8"/>
            <circle cx="158" cy="80" r="5" fill="#FF6B6B" opacity="0.8"/>
            <path d="M 105 85 Q 120 100 135 85" stroke="#2C3E50" stroke-width="4" fill="transparent" stroke-linecap="round"/>
            <rect x="78" y="120" width="84" height="65" rx="12" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <circle cx="100" cy="145" r="5" fill="#FF8C42"/>
            <circle cx="120" cy="145" r="5" fill="#2ECC71"/>
            <circle cx="140" cy="145" r="5" fill="#FF6B6B"/>
            <rect x="45" y="135" width="33" height="16" rx="8" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="162" y="135" width="33" height="16" rx="8" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="92" y="185" width="14" height="22" rx="4" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="78" y="200" width="35" height="15" rx="7" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="134" y="185" width="14" height="22" rx="4" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="127" y="200" width="35" height="15" rx="7" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
          </g>
        </svg>
        <h1>NemesisBot</h1>
      </div>
    </div>

    <div class="sidebar-status">
      <span class="status-dot" :class="appStore.connected ? 'connected' : 'disconnected'"></span>
      <span>{{ appStore.connected ? '已连接' : '未连接' }}</span>
    </div>

    <nav class="sidebar-nav">
      <div v-for="group in visibleNavGroups" :key="group.title" class="nav-section">
        <div class="nav-section-title">{{ group.title }}</div>
        <a
          v-for="item in group.items"
          :key="item.id"
          class="nav-item"
          :class="{ active: route.path === item.route }"
          @click="navigate(item.id)"
        >
          <span class="nav-icon">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path :d="item.icon"/></svg>
          </span>
          <span class="nav-label">{{ item.label }}</span>
        </a>
      </div>
    </nav>

    <div class="sidebar-footer">
      <!-- 签名验证状态徽标（2026-09-23 接入计划 §4）：只读三色，gateway 注入状态后显示 -->
      <a
        v-if="sigBadge"
        class="nav-item sig-badge"
        :class="sigBadge.color"
        :title="sigTitle"
      >
        <span class="nav-icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
        </span>
        <span class="nav-label">{{ sigBadge.label }}</span>
      </a>
      <!-- Full Access 激活徽标（2026-09-20）：放行中必须始终可见（急停旁） -->
      <a
        v-if="fullAccess"
        class="nav-item editor-badge"
        title="Full Access 放行中（运行时开关，Agent 重启后自动关闭）— 聊天工具栏或设置页【编辑器】可关闭"
      >
        <span class="nav-icon">⚡</span>
        <span class="nav-label">Full Access 中</span>
      </a>
      <a
        class="nav-item estop-btn"
        :class="{ engaged: estopEngaged }"
        :title="estopEngaged ? '急停中——点击释放' : '触发急停（冻结全部 agent 活动）'"
        @click="toggleEstop()"
      >
        <span class="nav-icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="9" y1="9" x2="15" y2="15"/><line x1="15" y1="9" x2="9" y2="15"/></svg>
        </span>
        <span class="nav-label">{{ estopBusy ? '处理中…' : (estopEngaged ? '⛔ 急停中（点此释放）' : '急停 E-Stop') }}</span>
      </a>
      <a class="nav-item" @click="toggleTheme()">
        <span class="nav-icon">
          <svg v-if="theme === 'dark'" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
          <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        </span>
        <span class="nav-label">{{ theme === 'dark' ? '浅色模式' : '深色模式' }}</span>
      </a>
      <a class="nav-item" @click="handleLogout()">
        <span class="nav-icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
        </span>
        <span class="nav-label">退出</span>
      </a>
    </div>

    <div class="sidebar-toggle" @click="appStore.toggleSidebar()">
      <span>{{ appStore.sidebarCollapsed ? '\u00BB' : '\u00AB' }}</span>
    </div>
  </aside>
</template>

<style scoped>
.editor-badge {
  color: #e6a23c;
}
/* 签名验证徽标三色（2026-09-23 接入计划 §4）：
   ok=Valid 绿 / bad=Tampered 红（warn 态继续运行，必须刺眼）/ warn=其余+off+降级 黄 */
.sig-badge.ok {
  color: #2ecc71;
}
.sig-badge.bad {
  color: #ff4d4d;
  font-weight: 600;
  background: rgba(255, 77, 77, 0.14);
}
.sig-badge.warn {
  color: #e6a23c;
}
.estop-btn {
  cursor: pointer;
}
.estop-btn.engaged {
  color: #ff4d4d;
  background: rgba(255, 77, 77, 0.14);
  font-weight: 600;
}
.estop-btn.engaged .nav-icon {
  animation: estop-pulse 1.4s ease-in-out infinite;
}
@keyframes estop-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
</style>

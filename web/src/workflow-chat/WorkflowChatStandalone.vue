<template>
  <div class="wf-chat-standalone">
    <!-- Phase: resolving URL / loading -->
    <div v-if="phase === 'resolving'" class="wf-chat-auth">
      <div class="auth-card">
        <h1>工作流聊天</h1>
        <p class="auth-subtitle">{{ resolveError || '正在解析工作流...' }}</p>
      </div>
    </div>

    <!-- Phase: password prompt -->
    <div v-else-if="phase === 'password'" class="wf-chat-auth">
      <div class="auth-card">
        <h1>工作流聊天</h1>
        <p class="auth-subtitle" v-if="workflowName">{{ workflowName }}</p>
        <p class="auth-subtitle" v-else>请输入密码</p>
        <p class="auth-subtitle-desc" v-if="workflowDescription">{{ workflowDescription }}</p>

        <div class="auth-form">
          <input
            class="form-input"
            type="password"
            placeholder="密码"
            autocomplete="off"
            v-model="password"
            @keydown.enter="doVerify"
            :disabled="verifying"
          />
          <button
            class="btn btn-primary btn-lg"
            @click="doVerify"
            :disabled="verifying || !password"
          >
            {{ verifying ? '验证中...' : '进入' }}
          </button>
          <p class="auth-error" v-if="verifyError">{{ verifyError }}</p>
        </div>
      </div>
    </div>

    <!-- Phase: connecting WS -->
    <div v-else-if="phase === 'connecting'" class="wf-chat-auth">
      <div class="auth-card">
        <h1>工作流聊天</h1>
        <p class="auth-subtitle">{{ connectError || '正在连接...' }}</p>
        <p class="auth-subtitle" v-if="workflowName">{{ workflowName }}</p>
      </div>
    </div>

    <!-- Phase: chat panel -->
    <ChatPanel
      v-else-if="phase === 'chat'"
      standalone
      :module="'workflow_chat'"
      :moduleData="{ index }"
      :titleOverride="workflowName"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import ChatPanel from '../components/ChatPanel.vue'
import { useWebSocket } from '../composables/useWebSocket'

const props = defineProps<{
  index: string
}>()

const { connect, disconnect, status: wsStatus } = useWebSocket()

type Phase = 'resolving' | 'password' | 'connecting' | 'chat'
const phase = ref<Phase>('resolving')
const resolveError = ref('')
const workflowName = ref('')
const workflowDescription = ref('')
const needsPassword = ref(false)
const password = ref('')
const verifying = ref(false)
const verifyError = ref('')
const connectError = ref('')

function parseIndexFromPath(): string | null {
  const pathMatch = window.location.pathname.match(/\/workflow\/chat\/([0-9a-fA-F]+)/)
  if (pathMatch) return pathMatch[1]
  const hashMatch = window.location.hash.match(/workflow\/chat\/([0-9a-fA-F]+)/)
  if (hashMatch) return hashMatch[1]
  return null
}

async function fetchInfo(idx: string): Promise<void> {
  try {
    const res = await fetch('/api/workflow/chat/info?index=' + encodeURIComponent(idx))
    if (!res.ok) {
      throw new Error('HTTP ' + res.status)
    }
    const data = await res.json()
    if (!data.found) {
      resolveError.value = '未找到该工作流'
      return
    }
    if (!data.chat_eligible) {
      resolveError.value = '此工作流不支持聊天测试：' + (data.reason || '未知原因')
      return
    }
    workflowName.value = data.workflow_name || ''
    workflowDescription.value = data.description || ''
    needsPassword.value = !!data.needs_password
    if (needsPassword.value) {
      phase.value = 'password'
    } else {
      // No password needed — go straight to WS connect.
      await connectChat('')
    }
  } catch (e: any) {
    resolveError.value = '解析失败：' + (e?.message || String(e))
  }
}

async function connectChat(pwd: string): Promise<void> {
  phase.value = 'connecting'
  connectError.value = ''
  const idx = props.index || parseIndexFromPath() || ''
  // connect() with workflow_chat param: server allows no-pwd when
  // !has_password(index), otherwise verifies pwd. We pass the pwd even
  // when empty so the server's logic stays simple.
  disconnect()
  connect(null, null, { workflow_chat: idx, pwd })

  // Wait briefly for the connection attempt. wsStatus reflects the
  // outcome — 'connected' means we're in, 'disconnected' after a delay
  // means auth failed.
  const deadline = Date.now() + 4000
  while (Date.now() < deadline) {
    if (wsStatus.value === 'connected') {
      phase.value = 'chat'
      return
    }
    if (wsStatus.value === 'disconnected') {
      connectError.value = needsPassword.value
        ? '连接被拒绝 — 密码错误'
        : '连接被拒绝 — 服务端拒绝'
      // Stay in connecting phase so user sees the error; provide a back button
      // by reverting to password form if a password is needed.
      phase.value = needsPassword.value ? 'password' : 'resolving'
      verifyError.value = connectError.value
      return
    }
    await new Promise((r) => setTimeout(r, 100))
  }
  connectError.value = '连接超时'
  phase.value = needsPassword.value ? 'password' : 'resolving'
  verifyError.value = connectError.value
}

async function doVerify() {
  if (!password.value) return
  verifying.value = true
  verifyError.value = ''
  const idx = props.index || parseIndexFromPath() || ''
  try {
    const res = await fetch('/api/workflow/chat/verify', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ index: idx, password: password.value }),
    })
    if (res.status === 401) {
      verifyError.value = '密码错误'
      return
    }
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      verifyError.value = data?.error || 'HTTP ' + res.status
      return
    }
    const data = await res.json()
    if (!data.verified) {
      verifyError.value = '密码错误'
      return
    }
    // Verified — workflow_name may have been missing before verify (info
    // endpoint would have returned chat_eligible=false for a hidden workflow).
    // Update name from verify response and connect.
    workflowName.value = data.workflow_name || workflowName.value
    workflowDescription.value = data.description || workflowDescription.value
    await connectChat(password.value)
  } catch (e: any) {
    verifyError.value = e?.message || String(e)
  } finally {
    verifying.value = false
  }
}

onMounted(() => {
  const idx = props.index || parseIndexFromPath()
  if (!idx) {
    resolveError.value = 'URL 缺少工作流索引'
    return
  }
  fetchInfo(idx)
})
</script>

<style scoped>
.wf-chat-standalone {
  height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--bg);
}

.wf-chat-auth {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
}

.auth-card {
  background: var(--bg-surface, #fff);
  border-radius: 12px;
  padding: 36px 40px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.18);
  min-width: 360px;
  max-width: 440px;
}

.auth-card h1 {
  margin: 0 0 8px 0;
  font-size: 22px;
}

.auth-subtitle {
  color: var(--text-dim, #888);
  margin: 0 0 6px 0;
  font-size: 14px;
}

.auth-subtitle-desc {
  color: var(--text-dim, #aaa);
  margin: 0 0 20px 0;
  font-size: 13px;
}

.auth-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-top: 20px;
}

.form-input {
  padding: 10px 12px;
  border: 1px solid var(--border, #ddd);
  border-radius: 6px;
  font-size: 14px;
  background: var(--bg-input, #fff);
  color: var(--text, #222);
}

.btn {
  cursor: pointer;
  border: none;
  padding: 10px 16px;
  border-radius: 6px;
  font-size: 14px;
}

.btn-primary {
  background: var(--accent, #4a7cff);
  color: #fff;
}

.btn-primary:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-lg {
  padding: 12px 18px;
  font-size: 15px;
}

.auth-error {
  color: #d33;
  font-size: 13px;
  margin: 0;
}
</style>

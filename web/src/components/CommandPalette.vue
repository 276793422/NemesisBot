<script setup lang="ts">
/**
 * M6 (2026-09-07 devtool-upgrade 阶段 7): Ctrl+K 命令面板——三源合一
 * （自定义命令 / 内置命令 / 已安装技能，数据源复用 useSlashCommands，
 * 合并优先级与后端 K3 改写链一致）。
 *
 * 执行语义 = 「插入不发送」：把 `/name ` 填入聊天输入框并聚焦，参数型
 * 命令由用户补参后回车（K3 后端改写链照常生效）。非聊天视图先跳回
 * 聊天页（route '/'），命令草稿经 chat store `commandDraft` 投递，
 * ChatPanel watch 消费。Esc / 点击遮罩关闭。
 */
import { computed, nextTick, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useSlashCommands, filterSlashCommands, type SlashCommand } from '../composables/useSlashCommands'
import { useCommandPalette } from '../composables/useCommandPalette'
import { useChatStore } from '../stores/chat'

const router = useRouter()
const chatStore = useChatStore()
const slash = useSlashCommands()
const palette = useCommandPalette()

const query = ref('')
const activeIndex = ref(0)
const inputEl = ref<HTMLInputElement | null>(null)

const results = computed(() => {
  if (!query.value.trim()) return slash.commands.value
  return filterSlashCommands('/' + query.value, slash.commands.value)
})

const SOURCE_LABEL: Record<SlashCommand['source'], string> = {
  custom: '自定义',
  builtin: '内置',
  skill: '技能',
}

watch(
  () => palette.visible.value,
  async (v) => {
    if (!v) return
    query.value = ''
    activeIndex.value = 0
    // 三源加载（已加载且不强制时静默跳过——与输入框补全同款）。
    void slash.load()
    await nextTick()
    inputEl.value?.focus()
  },
)

watch(results, (list) => {
  if (activeIndex.value >= list.length) activeIndex.value = 0
})

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    palette.close()
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    if (results.value.length) activeIndex.value = (activeIndex.value + 1) % results.value.length
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    if (results.value.length)
      activeIndex.value = (activeIndex.value - 1 + results.value.length) % results.value.length
  } else if (e.key === 'Enter') {
    e.preventDefault()
    const cmd = results.value[activeIndex.value]
    if (cmd) execute(cmd)
  }
}

/** 插入不发送：命令草稿投递给 ChatPanel；非聊天视图先导航回聊天页。 */
async function execute(cmd: SlashCommand) {
  chatStore.commandDraft = `/${cmd.name} `
  palette.close()
  if (router.currentRoute.value.path !== '/') await router.push('/')
}
</script>

<template>
  <Teleport to="body">
    <div v-if="palette.visible.value" class="palette-backdrop" @mousedown.self="palette.close()">
      <div class="palette-panel" role="dialog" aria-label="命令面板">
        <input
          ref="inputEl"
          v-model="query"
          class="palette-input"
          type="text"
          placeholder="搜索命令（/ 前缀匹配，描述兜底）…"
          @keydown="onKeydown"
        />
        <div v-if="results.length === 0" class="palette-empty">无匹配命令</div>
        <ul v-else class="palette-list" role="listbox">
          <li
            v-for="(cmd, i) in results"
            :key="cmd.source + ':' + cmd.name"
            class="palette-item"
            :class="{ active: i === activeIndex }"
            role="option"
            :aria-selected="i === activeIndex"
            @mouseenter="activeIndex = i"
            @mousedown.prevent="execute(cmd)"
          >
            <div class="palette-item-main">
              <span class="palette-cmd">/{{ cmd.name }}</span>
              <span class="palette-badge" :class="'src-' + cmd.source">{{
                SOURCE_LABEL[cmd.source]
              }}</span>
              <span v-if="cmd.argument_hint" class="palette-hint">{{ cmd.argument_hint }}</span>
            </div>
            <div class="palette-desc">{{ cmd.description }}</div>
          </li>
        </ul>
        <div class="palette-foot">↑↓ 选择 · Enter 插入到输入框 · Esc 关闭</div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.palette-backdrop {
  position: fixed;
  inset: 0;
  z-index: 3000;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 12vh;
}
.palette-panel {
  width: min(560px, 92vw);
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-panel, #1e1e2e);
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
  border-radius: 10px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.45);
  overflow: hidden;
}
.palette-input {
  margin: 10px;
  padding: 10px 12px;
  font-size: 14px;
  color: var(--text-primary, #eaeaf0);
  background: var(--bg-input, rgba(255, 255, 255, 0.06));
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
  border-radius: 6px;
  outline: none;
}
.palette-input:focus {
  border-color: var(--accent, #3b82f6);
}
.palette-empty {
  padding: 18px;
  text-align: center;
  color: var(--text-secondary, #8b8b9e);
  font-size: 13px;
}
.palette-list {
  list-style: none;
  margin: 0;
  padding: 0 6px 6px;
  overflow-y: auto;
  flex: 1;
}
.palette-item {
  padding: 8px 10px;
  border-radius: 6px;
  cursor: pointer;
}
.palette-item.active {
  background: var(--bg-hover, rgba(59, 130, 246, 0.16));
}
.palette-item-main {
  display: flex;
  align-items: center;
  gap: 8px;
}
.palette-cmd {
  font-family: var(--font-mono, monospace);
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary, #eaeaf0);
}
.palette-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 999px;
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.16));
  color: var(--text-secondary, #8b8b9e);
}
.palette-badge.src-custom {
  color: #34d399;
}
.palette-badge.src-builtin {
  color: #60a5fa;
}
.palette-badge.src-skill {
  color: #fbbf24;
}
.palette-hint {
  font-size: 12px;
  color: var(--text-secondary, #8b8b9e);
}
.palette-desc {
  margin-top: 2px;
  font-size: 12px;
  color: var(--text-secondary, #8b8b9e);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.palette-foot {
  padding: 8px 12px;
  font-size: 11px;
  color: var(--text-secondary, #8b8b9e);
  border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
}
</style>

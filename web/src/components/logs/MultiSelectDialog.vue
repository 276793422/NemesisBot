<script lang="ts">
export interface SelectableItem {
  value: string
  label: string
  icon?: string
  color?: string
  count?: number
}
</script>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'

const props = withDefaults(defineProps<{
  visible: boolean
  title: string
  items: SelectableItem[]
  selected: Set<string>
  searchEnabled?: boolean
  showCounts?: boolean
}>(), {
  searchEnabled: true,
  showCounts: false,
})

const emit = defineEmits<{
  confirm: [selected: Set<string>]
  cancel: []
}>()

const search = ref('')
const pending = ref<Set<string>>(new Set())

watch(() => props.visible, (v) => {
  if (v) {
    pending.value = new Set(props.selected)
    search.value = ''
  }
})

const filteredItems = computed(() => {
  if (!search.value) return props.items
  const k = search.value.toLowerCase()
  return props.items.filter(i =>
    i.label.toLowerCase().includes(k) || i.value.toLowerCase().includes(k)
  )
})

const allSelected = computed(() => {
  return props.items.length > 0 && pending.value.size === props.items.length
})

const noneSelected = computed(() => pending.value.size === 0)

function toggleItem(value: string) {
  if (pending.value.has(value)) pending.value.delete(value)
  else pending.value.add(value)
  pending.value = new Set(pending.value)
}

function selectAll() {
  pending.value = new Set(props.items.map(i => i.value))
}

function clearAll() {
  pending.value = new Set()
}

function handleConfirm() {
  emit('confirm', new Set(pending.value))
}

function handleCancel() {
  emit('cancel')
}
</script>

<template>
  <Teleport to="body">
    <Transition name="modal">
      <div v-if="visible" class="modal-overlay" @click.self="handleCancel">
        <div class="select-dialog" @click.stop>
          <div class="dialog-header">
            <h3>{{ title }}</h3>
            <button class="btn btn-sm btn-ghost" @click="handleCancel">✕</button>
          </div>

          <div class="dialog-toolbar">
            <input
              v-if="searchEnabled"
              class="form-input dialog-search"
              type="text"
              placeholder="搜索..."
              v-model="search"
            >
            <button class="btn btn-sm btn-ghost" @click="selectAll" :disabled="allSelected">
              ☑ 全选
            </button>
            <button class="btn btn-sm btn-ghost" @click="clearAll" :disabled="noneSelected">
              ☐ 清空
            </button>
            <span class="dialog-count">{{ pending.size }} / {{ items.length }}</span>
          </div>

          <div class="dialog-list scrollable">
            <label
              v-for="item in filteredItems"
              :key="item.value"
              class="dialog-option"
              :class="{ active: pending.has(item.value) }"
              :style="item.color ? { '--item-color': item.color } : {}"
            >
              <input
                type="checkbox"
                :checked="pending.has(item.value)"
                @change="toggleItem(item.value)"
              >
              <span v-if="item.icon" class="option-icon">{{ item.icon }}</span>
              <span class="option-name">{{ item.label }}</span>
              <span v-if="showCounts && item.count !== undefined" class="option-count">
                {{ item.count }} 条
              </span>
            </label>
            <div v-if="filteredItems.length === 0" class="dialog-empty">
              <p>无匹配项</p>
            </div>
          </div>

          <div class="dialog-footer">
            <button class="btn btn-ghost" @click="handleCancel">取消</button>
            <button class="btn btn-primary" @click="handleConfirm">
              确认（{{ pending.size }}）
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.select-dialog {
  width: 480px;
  max-width: 90vw;
  max-height: 70vh;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  border: 1px solid var(--border-light);
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 32px rgba(0,0,0,0.3);
}

.dialog-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border-light);
}

.dialog-header h3 {
  margin: 0;
  font-size: var(--text-md);
  font-weight: 600;
}

.dialog-toolbar {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border-light);
  align-items: center;
  flex-wrap: wrap;
}

.dialog-search {
  flex: 1;
  min-width: 160px;
}

.dialog-count {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-left: auto;
  white-space: nowrap;
}

.dialog-list {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-2) 0;
  min-height: 200px;
  max-height: 50vh;
}

.dialog-option {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-4);
  cursor: pointer;
  transition: background 0.1s;
  border-left: 3px solid transparent;
}

.dialog-option:hover {
  background: var(--bg-hover);
}

.dialog-option.active {
  background: var(--accent-muted);
  border-left-color: var(--item-color, var(--accent));
}

.dialog-option input[type="checkbox"] {
  margin: 0;
  cursor: pointer;
  accent-color: var(--item-color, var(--accent));
}

.option-icon {
  font-size: 14px;
}

.option-name {
  flex: 1;
  font-size: var(--text-sm);
  color: var(--text-primary);
  word-break: break-all;
}

.dialog-option.active .option-name {
  color: var(--item-color, var(--accent));
  font-weight: 500;
}

.option-count {
  font-size: var(--text-xs);
  color: var(--text-muted);
  background: var(--bg-tertiary);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

.dialog-empty {
  padding: var(--space-4);
  text-align: center;
  color: var(--text-muted);
  font-size: var(--text-sm);
}

.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  border-top: 1px solid var(--border-light);
}

.modal-enter-active, .modal-leave-active {
  transition: opacity 0.15s;
}

.modal-enter-from, .modal-leave-to {
  opacity: 0;
}

.modal-enter-active .select-dialog,
.modal-leave-active .select-dialog {
  transition: transform 0.15s;
}

.modal-enter-from .select-dialog,
.modal-leave-to .select-dialog {
  transform: scale(0.95);
}
</style>

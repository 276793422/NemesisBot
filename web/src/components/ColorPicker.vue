<script setup lang="ts">
import { ref, computed } from 'vue'
import { useTheme } from '../composables/useTheme'

const { colorScheme, setColorScheme, createCustomScheme, presets } = useTheme()
const showPicker = ref(false)
const customColor = ref('#E8705A')
const customName = ref('自定义')

const currentId = computed(() => colorScheme.value.id)

function selectPreset(scheme: typeof presets[0]) {
  setColorScheme(scheme)
}

function applyCustom() {
  const scheme = createCustomScheme(customName.value, customColor.value)
  setColorScheme(scheme)
}

function togglePicker() {
  showPicker.value = !showPicker.value
}
</script>

<template>
  <div class="color-picker">
    <button
      type="button"
      class="color-trigger"
      :style="{ background: colorScheme.accent }"
      :title="`当前配色: ${colorScheme.name}`"
      @click="togglePicker"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <path d="M12 2.69l5.66 5.66a8 8 0 1 1-11.31 0z"/>
      </svg>
    </button>

    <div v-if="showPicker" class="color-picker-dropdown" @click.stop>
      <div class="picker-header">
        <span class="picker-title">配色方案</span>
        <button type="button" class="picker-close" @click="showPicker = false">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      </div>

      <div class="preset-grid">
        <button
          v-for="preset in presets"
          :key="preset.id"
          type="button"
          class="preset-item"
          :class="{ active: currentId === preset.id }"
          :title="preset.name"
          @click="selectPreset(preset)"
        >
          <span class="preset-swatch" :style="{ background: preset.accent }"></span>
          <span class="preset-name">{{ preset.name }}</span>
        </button>
      </div>

      <div class="custom-section">
        <div class="custom-label">自定义</div>
        <div class="custom-row">
          <input
            v-model="customColor"
            type="color"
            class="color-input"
            title="选择颜色"
          />
          <input
            v-model="customName"
            type="text"
            class="name-input"
            placeholder="命名..."
            maxlength="8"
          />
          <button type="button" class="apply-btn" @click="applyCustom">
            应用
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.color-picker {
  position: relative;
}

.color-trigger {
  width: 32px;
  height: 32px;
  border-radius: var(--radius-full);
  border: 2px solid var(--border);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  color: white;
  transition: all var(--duration-fast);
  flex-shrink: 0;
  padding: 0;
  background: none;
}

.color-trigger:hover {
  transform: scale(1.1);
  box-shadow: 0 0 0 3px var(--accent-muted);
}

.color-picker-dropdown {
  position: absolute;
  bottom: calc(100% + 8px);
  right: 0;
  width: 280px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-xl);
  padding: var(--space-4);
  z-index: calc(var(--z-sidebar) + 1);
  animation: scaleIn var(--duration-fast) var(--ease-spring);
}

.picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-3);
}

.picker-title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
}

.picker-close {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-sm);
  transition: color var(--duration-fast);
}

.picker-close:hover {
  color: var(--text);
}

.preset-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}

.preset-item {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: var(--space-2);
  border: 2px solid transparent;
  border-radius: var(--radius-md);
  cursor: pointer;
  background: none;
  transition: all var(--duration-fast);
}

.preset-item:hover {
  background: var(--surface-hover);
}

.preset-item.active {
  border-color: var(--accent);
  background: var(--accent-muted);
}

.preset-swatch {
  width: 28px;
  height: 28px;
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-xs);
  transition: transform var(--duration-fast);
}

.preset-item:hover .preset-swatch {
  transform: scale(1.15);
}

.preset-name {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  white-space: nowrap;
}

.custom-section {
  border-top: 1px solid var(--border);
  padding-top: var(--space-3);
}

.custom-label {
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: var(--space-2);
}

.custom-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.color-input {
  width: 36px;
  height: 36px;
  border: none;
  border-radius: var(--radius-md);
  cursor: pointer;
  padding: 0;
  background: none;
  flex-shrink: 0;
}

.color-input::-webkit-color-swatch-wrapper {
  padding: 0;
}

.color-input::-webkit-color-swatch {
  border: 2px solid var(--border);
  border-radius: var(--radius-md);
}

.name-input {
  flex: 1;
  padding: var(--space-1) var(--space-2);
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  color: var(--text);
  font-size: var(--text-sm);
  font-family: var(--font-sans);
  min-width: 0;
}

.name-input:focus {
  outline: none;
  border-color: var(--accent);
}

.apply-btn {
  padding: var(--space-1) var(--space-3);
  background: var(--accent);
  color: white;
  border: none;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 600;
  cursor: pointer;
  transition: all var(--duration-fast);
  flex-shrink: 0;
}

.apply-btn:hover {
  background: var(--accent-hover);
  transform: translateY(-1px);
}
</style>

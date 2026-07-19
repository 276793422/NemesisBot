<script setup lang="ts">
/**
 * Smart form component with friendly controls:
 * - toggle: click to switch
 * - slider: drag for numeric ranges
 * - select: dropdown for presets
 * - number: spin buttons
 * - text/password: input fields
 */
import { computed, ref, watch } from 'vue'
import { fieldMetaFor, type FriendlyField } from '../lib/friendlyFields'

const props = defineProps<{
  modelValue: Record<string, any>
  metaTable?: Record<string, FriendlyField>
  /** Keys to hide entirely */
  hideKeys?: string[]
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: Record<string, any>): void
}>()

const rows = computed(() => {
  const hide = new Set(props.hideKeys || [])
  const table = props.metaTable || {}
  return Object.entries(props.modelValue || {})
    .filter(([k, v]) => !hide.has(k) && (v === null || ['string', 'number', 'boolean'].includes(typeof v)))
    .map(([k, v]) => ({ meta: fieldMetaFor(k, v, table), value: v }))
})

function setField(key: string, value: any) {
  emit('update:modelValue', { ...props.modelValue, [key]: value })
}

function formatSliderValue(meta: FriendlyField, value: number): string {
  if (meta.unit) return `${Math.round(value)}${meta.unit}`
  return String(Math.round(value))
}

function getSliderDisplay(meta: FriendlyField, value: any): string {
  const num = Number(value) || meta.min || 0
  return formatSliderValue(meta, num)
}
</script>

<template>
  <div class="smart-field-form">
    <div v-if="rows.length === 0" class="empty-hint">暂无需要填写的选项</div>
    <div v-for="{ meta, value } in rows" :key="meta.key" class="field-row" :class="`kind-${meta.kind}`">
      <div class="field-label">
        <span class="field-name">{{ meta.label }}</span>
        <span v-if="meta.hint" class="field-hint">{{ meta.hint }}</span>
      </div>
      <div class="field-control">
        <!-- Toggle Switch -->
        <div
          v-if="meta.kind === 'toggle'"
          class="toggle-control"
          :class="{ active: !!value }"
          role="switch"
          :aria-checked="!!value"
          @click="setField(meta.key, !value)"
        >
          <span class="toggle-track">
            <span class="toggle-thumb"></span>
          </span>
          <span class="toggle-label">{{ value ? '已启用' : '已禁用' }}</span>
        </div>

        <!-- Slider -->
        <div v-else-if="meta.kind === 'slider'" class="slider-control">
          <div class="slider-header">
            <span class="slider-value">{{ getSliderDisplay(meta, value) }}</span>
          </div>
          <input
            type="range"
            class="slider-input"
            :min="meta.min ?? 0"
            :max="meta.max ?? 100"
            :step="meta.step ?? 1"
            :value="Number(value) || meta.min || 0"
            @input="setField(meta.key, Number(($event.target as HTMLInputElement).value))"
          />
          <div class="slider-ruler">
            <span>{{ meta.min ?? 0 }}{{ meta.unit }}</span>
            <span>{{ meta.max ?? 100 }}{{ meta.unit }}</span>
          </div>
        </div>

        <!-- Select Dropdown -->
        <div v-else-if="meta.kind === 'select'" class="select-control">
          <div class="select-options">
            <button
              v-for="opt in meta.options"
              :key="String(opt.value)"
              type="button"
              class="select-chip"
              :class="{ active: value === opt.value || String(value) === String(opt.value) }"
              @click="setField(meta.key, opt.value)"
            >
              {{ opt.label }}
            </button>
          </div>
        </div>

        <!-- Number Input -->
        <div v-else-if="meta.kind === 'number'" class="number-control">
          <input
            class="form-input number-input"
            type="number"
            :value="value ?? ''"
            @input="setField(meta.key, Number(($event.target as HTMLInputElement).value))"
          />
          <div v-if="meta.unit" class="number-unit">{{ meta.unit }}</div>
        </div>

        <!-- Password Input -->
        <input
          v-else-if="meta.kind === 'password'"
          class="form-input"
          type="password"
          :value="value ?? ''"
          autocomplete="off"
          placeholder="已保存则保持不变，或粘贴新密钥"
          @input="setField(meta.key, ($event.target as HTMLInputElement).value)"
        />

        <!-- Text Input -->
        <input
          v-else
          class="form-input"
          type="text"
          :value="value ?? ''"
          @input="setField(meta.key, ($event.target as HTMLInputElement).value)"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.smart-field-form {
  display: flex;
  flex-direction: column;
  gap: var(--space-5);
}

.field-row {
  display: grid;
  grid-template-columns: minmax(140px, 220px) 1fr;
  gap: var(--space-4);
  align-items: start;
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--border);
}

.field-row:last-child {
  border-bottom: none;
}

.field-label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding-top: var(--space-1);
}

.field-name {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
}

.field-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-weight: 400;
  line-height: 1.4;
}

.field-control {
  display: flex;
  align-items: center;
  min-height: 36px;
}

.empty-hint {
  color: var(--text-muted);
  font-size: var(--text-sm);
  padding: var(--space-6);
  text-align: center;
}

/* ===== Toggle Control ===== */
.toggle-control {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  cursor: pointer;
  user-select: none;
}

.toggle-track {
  position: relative;
  width: 44px;
  height: 24px;
  background: var(--border);
  border-radius: var(--radius-full);
  transition: background var(--duration-fast);
  flex-shrink: 0;
}

.toggle-control.active .toggle-track {
  background: var(--accent);
}

.toggle-thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 20px;
  height: 20px;
  background: white;
  border-radius: 50%;
  transition: transform var(--duration-fast);
  box-shadow: var(--shadow-xs);
}

.toggle-control.active .toggle-thumb {
  transform: translateX(20px);
}

.toggle-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  font-weight: 500;
}

/* ===== Slider Control ===== */
.slider-control {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  width: 100%;
  max-width: 400px;
}

.slider-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.slider-value {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--accent);
  background: var(--accent-muted);
  padding: 2px 10px;
  border-radius: var(--radius-sm);
}

.slider-input {
  -webkit-appearance: none;
  appearance: none;
  width: 100%;
  height: 6px;
  background: var(--border);
  border-radius: var(--radius-full);
  outline: none;
  cursor: pointer;
}

.slider-input::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  box-shadow: var(--shadow-sm);
  transition: transform var(--duration-fast), box-shadow var(--duration-fast);
}

.slider-input::-webkit-slider-thumb:hover {
  transform: scale(1.15);
  box-shadow: 0 0 0 4px var(--accent-muted);
}

.slider-input::-moz-range-thumb {
  width: 20px;
  height: 20px;
  background: var(--accent);
  border-radius: 50%;
  cursor: pointer;
  border: none;
  box-shadow: var(--shadow-sm);
}

.slider-ruler {
  display: flex;
  justify-content: space-between;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

/* ===== Select Control ===== */
.select-control {
  width: 100%;
}

.select-options {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.select-chip {
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text-secondary);
  font-size: var(--text-sm);
  font-weight: 500;
  cursor: pointer;
  transition: all var(--duration-fast);
  font-family: var(--font-sans);
}

.select-chip:hover {
  border-color: var(--text-muted);
  background: var(--surface-hover);
}

.select-chip.active {
  border-color: var(--accent);
  background: var(--accent-muted);
  color: var(--accent);
  box-shadow: 0 0 0 1px rgba(232, 112, 90, 0.15);
}

/* ===== Number Control ===== */
.number-control {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.number-input {
  max-width: 120px;
  text-align: center;
}

.number-unit {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

/* ===== Responsive ===== */
@media (max-width: 640px) {
  .field-row {
    grid-template-columns: 1fr;
    gap: var(--space-2);
  }

  .slider-control {
    max-width: 100%;
  }
}
</style>

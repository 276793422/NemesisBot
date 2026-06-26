<script setup lang="ts">
/**
 * Form field wrapper: renders a label, slot for the input control, and an
 * optional hint / error message below. Used by every node form to keep
 * spacing and typography consistent.
 */
defineProps<{
  label: string
  /** Hint text shown in muted color below the input. */
  hint?: string
  /** Error text shown in danger color below the input. Mutually exclusive with hint. */
  error?: string
  /** Mark the label with a red asterisk. Cosmetic only — does not block save. */
  required?: boolean
}>()
</script>

<template>
  <div class="form-field" :class="{ 'has-error': !!error }">
    <label class="form-field-label">
      {{ label }}
      <span v-if="required" class="req">*</span>
    </label>
    <div class="form-field-control">
      <slot />
    </div>
    <div v-if="error" class="form-field-msg form-field-error">⚠ {{ error }}</div>
    <div v-else-if="hint" class="form-field-msg form-field-hint">{{ hint }}</div>
  </div>
</template>

<style scoped>
.form-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin-bottom: var(--space-2);
}

.form-field-label {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  font-weight: 500;
}

.form-field-label .req {
  color: var(--danger, #e74c3c);
  margin-left: 2px;
}

.form-field-control {
  display: flex;
  flex-direction: column;
}

.form-field-msg {
  font-size: var(--text-xs);
  margin-top: 2px;
}

.form-field-hint {
  color: var(--text-muted);
}

.form-field-error {
  color: var(--danger, #e74c3c);
}

.form-field.has-error .form-field-control :deep(.form-input),
.form-field.has-error .form-field-control :deep(.form-textarea),
.form-field.has-error .form-field-control :deep(.form-select) {
  border-color: var(--danger, #e74c3c);
}
</style>

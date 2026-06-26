/**
 * Variable picker — `@`-triggered dropdown for inserting `{{var}}` references
 * into text inputs/textareas.
 *
 * Usage:
 *   const { onInput, onKeyDown } = useVariablePicker(inputRef, availableVars)
 *
 * The composable watches the bound element for `@` typed at the cursor
 * position, exposes the active picker state (open, query, matches,
 * selectedIndex), and provides `insert(variable)` which replaces the
 * `@query` with `{{variable}}` at cursor and closes the picker.
 *
 * Caller is responsible for rendering the dropdown UI and forwarding
 * `onInput` / `onKeyDown` to the underlying <input> / <textarea>.
 */
import { ref, computed } from 'vue'

export interface VariableOption {
  /** Variable name as it should appear in {{...}} — e.g. "user_input" or "fetch.output.text". */
  value: string
  /** Human-readable label shown in the dropdown. */
  label: string
  /** Optional grouping hint (e.g. "Workflow variables", "Node outputs"). */
  group?: string
}

export interface VariablePickerState {
  open: boolean
  query: string
  startIndex: number  // cursor index where `@` was typed
  selectedIndex: number
  matches: VariableOption[]
  anchor: { top: number; left: number } | null
}

export function useVariablePicker(
  inputEl: () => HTMLInputElement | HTMLTextAreaElement | null,
  availableVars: () => VariableOption[],
) {
  const state = ref<VariablePickerState>({
    open: false,
    query: '',
    startIndex: -1,
    selectedIndex: 0,
    matches: [],
    anchor: null,
  })

  function computeMatches(): VariableOption[] {
    const q = state.value.query.toLowerCase()
    const all = availableVars()
    if (!q) return all.slice(0, 50)
    return all
      .filter((v) => v.value.toLowerCase().includes(q) || v.label.toLowerCase().includes(q))
      .slice(0, 50)
  }

  function openPicker(at: number, anchor: { top: number; left: number }) {
    state.value = {
      open: true,
      query: '',
      startIndex: at,
      selectedIndex: 0,
      matches: computeMatches(),
      anchor,
    }
  }

  function closePicker() {
    state.value = { ...state.value, open: false, anchor: null }
  }

  function updateQuery(q: string) {
    state.value.query = q
    state.value.matches = computeMatches()
    state.value.selectedIndex = 0
  }

  /**
   * Wire to <input> / <textarea> `@input`. Detects `@` typed at cursor and
   * opens the picker; updates query as user continues typing.
   */
  function onInput(_e: Event) {
    const el = inputEl()
    if (!el) return
    const pos = el.selectionStart ?? el.value.length
    const text = el.value

    if (state.value.open) {
      // Picker is open — extract query from `startIndex` to current cursor.
      const start = state.value.startIndex
      if (pos < start) {
        closePicker()
        return
      }
      const q = text.slice(start + 1, pos)
      // Close if user typed whitespace or moved past a token boundary.
      if (/\s/.test(q)) {
        closePicker()
        return
      }
      updateQuery(q)
      return
    }

    // Detect `@` just typed at cursor (the char before pos is `@` and the
    // char before that is whitespace or start-of-string — avoids triggering
    // inside emails).
    if (pos > 0 && text[pos - 1] === '@') {
      const prev = pos >= 2 ? text[pos - 2] : ' '
      if (/\s/.test(prev) || pos === 1) {
        const rect = caretCoords(el, pos)
        openPicker(pos - 1, rect)
      }
    }
  }

  /**
   * Wire to `@keydown`. Handles ArrowUp/Down/Enter/Escape when the picker
   * is open.
   */
  function onKeyDown(e: KeyboardEvent) {
    if (!state.value.open) return
    const s = state.value
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      s.selectedIndex = Math.min(s.selectedIndex + 1, s.matches.length - 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      s.selectedIndex = Math.max(s.selectedIndex - 1, 0)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      const opt = s.matches[s.selectedIndex]
      if (opt) insert(opt.value)
      else closePicker()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      closePicker()
    }
  }

  /**
   * Replace `@query` at cursor with `{{variable}}` and close the picker.
   */
  function insert(variable: string) {
    const el = inputEl()
    if (!el) {
      closePicker()
      return
    }
    const start = state.value.startIndex
    const pos = el.selectionStart ?? el.value.length
    const before = el.value.slice(0, start)
    const after = el.value.slice(pos)
    const insertText = '{{' + variable + '}}'
    const newValue = before + insertText + after
    const newPos = start + insertText.length
    // Use native setter so v-model picks up the change.
    setNativeValue(el, newValue)
    el.selectionStart = el.selectionEnd = newPos
    el.dispatchEvent(new Event('input', { bubbles: true }))
    el.focus()
    closePicker()
  }

  return {
    state,
    onInput,
    onKeyDown,
    insert,
    closePicker,
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Set value on input/textarea in a way that triggers v-model. */
function setNativeValue(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  if (setter) {
    setter.call(el, value)
  } else {
    ;(el as any)._value = value
  }
}

/** Compute approximate (top, left) caret coordinates for dropdown anchoring. */
function caretCoords(
  el: HTMLInputElement | HTMLTextAreaElement,
  pos: number,
): { top: number; left: number } {
  // Lazy: use element bounding rect + a small offset. A pixel-perfect
  // mirror-<div> implementation is overkill for v1.
  const rect = el.getBoundingClientRect()
  return { top: rect.top + 24, left: rect.left + 12 }
}

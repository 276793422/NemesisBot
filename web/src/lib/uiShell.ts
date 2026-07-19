/**
 * UI shell preference — pure helpers so vitest can cover persistence without
 * mounting Vue. Default is the user-friendly shell; classic keeps the old IA.
 */

export type UiShellMode = 'friendly' | 'classic'

export const UI_SHELL_STORAGE_KEY = 'nemesisbot_ui_shell'
export const UI_SHELL_DEFAULT: UiShellMode = 'friendly'

export function isUiShellMode(value: unknown): value is UiShellMode {
  return value === 'friendly' || value === 'classic'
}

/** Parse a stored value; unknown/missing → default friendly. */
export function parseUiShellMode(raw: string | null | undefined): UiShellMode {
  if (raw == null || raw === '') return UI_SHELL_DEFAULT
  const v = raw.trim().toLowerCase()
  return isUiShellMode(v) ? v : UI_SHELL_DEFAULT
}

export function readUiShellMode(
  storage: Pick<Storage, 'getItem'> | null | undefined = typeof localStorage !== 'undefined' ? localStorage : null,
): UiShellMode {
  try {
    return parseUiShellMode(storage?.getItem(UI_SHELL_STORAGE_KEY) ?? null)
  } catch {
    return UI_SHELL_DEFAULT
  }
}

export function writeUiShellMode(
  mode: UiShellMode,
  storage: Pick<Storage, 'setItem'> | null | undefined = typeof localStorage !== 'undefined' ? localStorage : null,
): void {
  if (!isUiShellMode(mode)) return
  try {
    storage?.setItem(UI_SHELL_STORAGE_KEY, mode)
  } catch {
    // ignore quota / private mode
  }
}

export function toggleUiShellMode(current: UiShellMode): UiShellMode {
  return current === 'friendly' ? 'classic' : 'friendly'
}

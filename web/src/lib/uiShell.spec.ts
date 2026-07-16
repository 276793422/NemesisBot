import { describe, it, expect, beforeEach } from 'vitest'
import {
  UI_SHELL_DEFAULT,
  UI_SHELL_STORAGE_KEY,
  parseUiShellMode,
  readUiShellMode,
  writeUiShellMode,
  toggleUiShellMode,
  isUiShellMode,
} from './uiShell'

/** In-memory Storage stub — exercises real read/write helpers. */
function memoryStorage(initial: Record<string, string> = {}): Storage {
  const map = new Map(Object.entries(initial))
  return {
    get length() {
      return map.size
    },
    clear() {
      map.clear()
    },
    getItem(key: string) {
      return map.has(key) ? map.get(key)! : null
    },
    key(index: number) {
      return [...map.keys()][index] ?? null
    },
    removeItem(key: string) {
      map.delete(key)
    },
    setItem(key: string, value: string) {
      map.set(key, String(value))
    },
  }
}

describe('uiShell preference', () => {
  beforeEach(() => {
    // no-op; each test uses its own memory storage
  })

  it('defaults to friendly', () => {
    expect(UI_SHELL_DEFAULT).toBe('friendly')
    expect(parseUiShellMode(null)).toBe('friendly')
    expect(parseUiShellMode(undefined)).toBe('friendly')
    expect(parseUiShellMode('')).toBe('friendly')
    expect(parseUiShellMode('garbage')).toBe('friendly')
  })

  it('accepts only friendly | classic', () => {
    expect(isUiShellMode('friendly')).toBe(true)
    expect(isUiShellMode('classic')).toBe(true)
    expect(isUiShellMode('modern')).toBe(false)
  })

  it('persists and reloads shell mode via storage', () => {
    const storage = memoryStorage()
    expect(readUiShellMode(storage)).toBe('friendly')

    writeUiShellMode('classic', storage)
    expect(storage.getItem(UI_SHELL_STORAGE_KEY)).toBe('classic')
    expect(readUiShellMode(storage)).toBe('classic')

    writeUiShellMode('friendly', storage)
    expect(readUiShellMode(storage)).toBe('friendly')
  })

  it('toggle switches between shells', () => {
    expect(toggleUiShellMode('friendly')).toBe('classic')
    expect(toggleUiShellMode('classic')).toBe('friendly')
  })

  it('round-trips both shells as selectable preferences', () => {
    const storage = memoryStorage()
    const modes = ['friendly', 'classic'] as const
    for (const m of modes) {
      writeUiShellMode(m, storage)
      expect(readUiShellMode(storage)).toBe(m)
      expect(isUiShellMode(readUiShellMode(storage))).toBe(true)
    }
  })
})

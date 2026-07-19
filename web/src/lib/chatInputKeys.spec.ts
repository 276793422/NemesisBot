import { describe, it, expect } from 'vitest'
import { decideChatInputKey } from './chatInputKeys'

function key(
  partial: Partial<Pick<KeyboardEvent, 'key' | 'shiftKey' | 'ctrlKey' | 'metaKey' | 'isComposing'>>,
) {
  return {
    key: 'Enter',
    shiftKey: false,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    ...partial,
  }
}

describe('decideChatInputKey (shipped chat policy)', () => {
  it('plain Enter → send', () => {
    const d = decideChatInputKey(key({ key: 'Enter' }))
    expect(d.action).toBe('send')
    expect(d.preventDefault).toBe(true)
  })

  it('Shift+Enter → newline (does not send)', () => {
    const d = decideChatInputKey(key({ key: 'Enter', shiftKey: true }))
    expect(d.action).toBe('newline')
    expect(d.preventDefault).toBe(false)
  })

  it('Ctrl+Enter still sends (legacy)', () => {
    const d = decideChatInputKey(key({ key: 'Enter', ctrlKey: true }))
    expect(d.action).toBe('send')
    expect(d.preventDefault).toBe(true)
  })

  it('Cmd+Enter sends on mac-style meta', () => {
    const d = decideChatInputKey(key({ key: 'Enter', metaKey: true }))
    expect(d.action).toBe('send')
  })

  it('non-Enter keys are ignored', () => {
    const d = decideChatInputKey(key({ key: 'a' }))
    expect(d.action).toBe('none')
    expect(d.preventDefault).toBe(false)
  })

  it('IME composition does not steal Enter', () => {
    const d = decideChatInputKey(key({ key: 'Enter', isComposing: true }))
    expect(d.action).toBe('none')
  })
})

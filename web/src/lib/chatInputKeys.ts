/**
 * Chat composer keyboard policy — shared by ChatPanel and unit tests.
 *
 * Friendly / modern chat apps: Enter sends, Shift+Enter newline.
 * Ctrl/Cmd+Enter still sends (legacy power-user habit).
 */

export type ChatKeyAction = 'send' | 'newline' | 'none'

export interface ChatKeyDecision {
  action: ChatKeyAction
  /** When true, caller should preventDefault (and usually stopPropagation). */
  preventDefault: boolean
}

/**
 * Decide what a keydown on the chat textarea should do.
 * Uses the real browser KeyboardEvent fields so tests drive the shipped function.
 */
export function decideChatInputKey(e: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'ctrlKey' | 'metaKey' | 'isComposing'>): ChatKeyDecision {
  // IME composition: never steal Enter while composing CJK input.
  if (e.isComposing) {
    return { action: 'none', preventDefault: false }
  }

  if (e.key !== 'Enter') {
    return { action: 'none', preventDefault: false }
  }

  // Shift+Enter → let the browser insert a newline
  if (e.shiftKey && !e.ctrlKey && !e.metaKey) {
    return { action: 'newline', preventDefault: false }
  }

  // Enter alone, or Ctrl/Cmd+Enter → send
  return { action: 'send', preventDefault: true }
}

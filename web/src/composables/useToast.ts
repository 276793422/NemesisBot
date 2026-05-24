import { reactive } from 'vue'

export interface Toast {
  id: number
  message: string
  type: 'info' | 'success' | 'warn' | 'error'
  removing?: boolean
}

const toasts = reactive<Toast[]>([])
let nextId = 0

function addToast(message: string, type: Toast['type'] = 'info', duration = 4000) {
  const id = nextId++
  toasts.push({ id, message, type })
  if (duration > 0) {
    setTimeout(() => removeToast(id), duration)
  }
}

function removeToast(id: number) {
  const toast = toasts.find(t => t.id === id)
  if (!toast || toast.removing) return
  toast.removing = true
  setTimeout(() => {
    const idx = toasts.findIndex(t => t.id === id)
    if (idx !== -1) toasts.splice(idx, 1)
  }, 200)
}

export function useToast() {
  return {
    toasts,
    info: (msg: string) => addToast(msg, 'info'),
    success: (msg: string) => addToast(msg, 'success'),
    warn: (msg: string) => addToast(msg, 'warn'),
    error: (msg: string) => addToast(msg, 'error', 6000),
    remove: removeToast,
  }
}

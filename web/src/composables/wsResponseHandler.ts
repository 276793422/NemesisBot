/**
 * WS API response handler — shared between useWebSocket and useWSAPI.
 *
 * Placed in a separate file to avoid circular dependencies:
 *   useWebSocket.ts → wsResponseHandler.ts ← useWSAPI.ts
 */

const pendingRequests = new Map<string, {
  resolve: (data: any) => void
  reject: (error: string) => void
  timer: ReturnType<typeof setTimeout> | null
}>()

export const REQUEST_TIMEOUT = 30000

/**
 * Register a pending request. Called by useWSAPI.request().
 */
export function registerPendingRequest(
  reqId: string,
  resolve: (data: any) => void,
  reject: (error: string) => void,
  timer: ReturnType<typeof setTimeout> | null,
) {
  pendingRequests.set(reqId, { resolve, reject, timer })
}

/**
 * Remove a pending request (e.g., on timeout). Called by useWSAPI.request().
 */
export function removePendingRequest(reqId: string) {
  pendingRequests.delete(reqId)
}

/**
 * Called by useWebSocket's onmessage when a message arrives.
 * If it's a response matching a pending request, resolves/rejects the Promise.
 * Returns true if the message was consumed (should not be dispatched further).
 */
export function handleWSResponse(data: any): boolean {
  if (data.type !== 'response' || !data.reqId) return false

  const pending = pendingRequests.get(data.reqId)
  if (!pending) return false

  if (pending.timer) clearTimeout(pending.timer)
  pendingRequests.delete(data.reqId)

  if (data.error) {
    pending.reject(data.error)
  } else {
    pending.resolve(data.data)
  }
  return true
}

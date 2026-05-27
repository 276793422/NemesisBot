import { registerPendingRequest, removePendingRequest, REQUEST_TIMEOUT } from './wsResponseHandler'

/**
 * Promise-based WS API composable.
 *
 * Sends `type: "request"` messages and resolves/rejects Promises when the
 * matching `type: "response"` arrives (correlated by `reqId`).
 */

// Lazy reference to sendRaw — set by useWebSocket during initialization
// to avoid circular dependency.
let _sendRaw: ((msg: object) => void) | null = null

/**
 * Called by useWebSocket to provide the sendRaw function.
 * Breaks the circular dependency: useWSAPI doesn't import useWebSocket.
 */
export function initWSAPI(sendRaw: (msg: object) => void) {
  _sendRaw = sendRaw
}

export function useWSAPI() {
  /**
   * Send a WS API request and return a Promise that resolves with the
   * response data or rejects with an error string.
   * @param timeoutMs Timeout in ms. 0 = no timeout. Undefined = default (30s).
   */
  function request(module: string, cmd: string, data?: any, timeoutMs?: number): Promise<any> {
    return new Promise((resolve, reject) => {
      if (!_sendRaw) {
        reject('WS not initialized')
        return
      }

      const reqId = crypto.randomUUID()
      const effectiveTimeout = timeoutMs !== undefined ? timeoutMs : REQUEST_TIMEOUT
      const timer = effectiveTimeout > 0
        ? setTimeout(() => {
            removePendingRequest(reqId)
            reject(`timeout: ${module}.${cmd}`)
          }, effectiveTimeout)
        : null

      registerPendingRequest(reqId, resolve, reject, timer)

      _sendRaw({
        type: 'request',
        module,
        cmd,
        reqId,
        data: data ?? {},
      })
    })
  }

  return { request }
}

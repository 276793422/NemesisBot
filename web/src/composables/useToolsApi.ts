/**
 * Tools API client — typed wrapper around the WSAPI `tools.*` commands.
 *
 * `tools.list` enumerates the host bot's registered tools (name + description
 * + JSON Schema parameters). The workflow canvas tool-node form uses it to
 * render a tool picker and a schema-driven parameter form. The list mirrors
 * exactly what the workflow tool node can execute (the agent's tools, bridged
 * into the workflow registry via `AgentToolAdapter`), so the picker and the
 * runtime never drift.
 *
 * Mirrors the shape of `useWorkflowApi.ts`.
 */

import { useWSAPI } from './useWSAPI'

/** One registered tool: name + description + OpenAI-style JSON Schema. */
export interface ToolSchema {
  name: string
  description: string
  /** OpenAI-compatible JSON Schema object: { type:'object', properties, required }. */
  parameters: Record<string, unknown>
}

export interface ToolsListResponse {
  tools: ToolSchema[]
  count: number
}

// Module-level cache: the tool set is stable for the lifetime of the gateway
// session, so multiple tool-node forms share one fetch. `refresh()` busts it.
let _cache: ToolSchema[] | null = null
let _inflight: Promise<ToolSchema[]> | null = null

export function useToolsApi() {
  const { request } = useWSAPI()

  /**
   * Fetch the tool list (cached). Returns an empty array on error so the form
   * can fall back to free-text input instead of blocking.
   */
  async function list(force = false): Promise<ToolSchema[]> {
    if (_cache && !force) return _cache
    if (_inflight) return _inflight
    _inflight = (async () => {
      try {
        const resp = (await request('tools', 'list')) as ToolsListResponse
        _cache = Array.isArray(resp?.tools) ? resp.tools : []
      } catch {
        // Agent not running / WS not ready — leave empty so callers can degrade.
        _cache = []
      } finally {
        _inflight = null
      }
      return _cache
    })()
    return _inflight
  }

  function refresh(): Promise<ToolSchema[]> {
    _cache = null
    return list(true)
  }

  return { list, refresh }
}

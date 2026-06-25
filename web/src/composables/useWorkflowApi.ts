/**
 * Workflow API client — typed wrapper around the WSAPI `workflow.*` commands.
 *
 * Every function corresponds 1:1 to a backend WSAPI command in
 * `crates/nemesis-web/src/handlers/workflow.rs`. Return types mirror the
 * shapes declared in `web/src/types/workflow.ts`.
 *
 * UI MUST treat `trigger_driver_status` / `triggers[].driven` as the source
 * of truth for what's wired up. Never hardcode "event/message is undriven"
 * on the client — read it from the API response.
 */

import { useWSAPI } from './useWSAPI'
import type {
  NodeListResponse,
  WorkflowGetResponse,
  RunStartResponse,
  ValidateResponse,
  ExecutionListResponse,
  ExecutionDetail,
  CheckpointListResponse,
  Checkpoint,
  WorkflowDef,
} from '../types/workflow'

export function useWorkflowApi() {
  const { request } = useWSAPI()

  return {
    list: async (): Promise<NodeListResponse> =>
      await request('workflow', 'list'),

    get: async (name: string): Promise<WorkflowGetResponse> =>
      await request('workflow', 'get', { name }),

    create: async (workflow: WorkflowDef): Promise<{ name: string; created: boolean }> =>
      await request('workflow', 'create', { workflow }),

    update: async (
      name: string,
      workflow: WorkflowDef,
    ): Promise<{ name: string; updated: boolean }> =>
      await request('workflow', 'update', { name, workflow }),

    delete: async (name: string): Promise<{ name: string; deleted: boolean }> =>
      await request('workflow', 'delete', { name }),

    validate: async (workflow: WorkflowDef): Promise<ValidateResponse> =>
      await request('workflow', 'validate', { workflow }),

    /** Manually trigger a workflow to run right now (WebUI trigger source). */
    runNow: async (
      name: string,
      input: Record<string, unknown>,
    ): Promise<RunStartResponse> =>
      await request('workflow', 'run_now', { name, input }),

    start: async (
      name: string,
      input: Record<string, unknown>,
    ): Promise<RunStartResponse> =>
      await request('workflow', 'start', { name, input }),

    /**
     * Publish a trigger-event into the engine's EventDispatcher. The canvas
     * page uses this for the "⚡ 模拟事件" button so users can test `event`
     * triggers without needing the real producer (workflow.completed, etc.).
     */
    fireEvent: async (
      eventType: string,
      data: Record<string, unknown>,
    ): Promise<{
      event_type: string
      data: Record<string, unknown>
      matched_workflows: string[]
      published: boolean
    }> =>
      await request('workflow', 'fire_event', {
        event_type: eventType,
        data,
      }),

    /**
     * Resolve an opaque chat index (the 8-hex-char hash in the URL) to its
     * workflow metadata + chat eligibility. Used by the standalone
     * workflow-chat page (`/workflow/chat/<index>`) before rendering the
     * chat UI.
     */
    resolveChatTarget: async (
      index: string,
    ): Promise<{
      found: boolean
      workflow_name?: string
      description?: string
      chat_eligible: boolean
      reason?: string
    }> => await request('workflow', 'resolve_chat_target', { index }),

    status: async (executionId: string): Promise<ExecutionDetail> =>
      await request('workflow', 'status', { execution_id: executionId }),

    cancel: async (executionId: string): Promise<ExecutionDetail> =>
      await request('workflow', 'cancel', { execution_id: executionId }),

    resume: async (
      executionId: string,
      review: Record<string, unknown>,
    ): Promise<ExecutionDetail> =>
      await request('workflow', 'resume', { execution_id: executionId, review }),

    listExecutions: async (params: {
      workflow_name?: string
      state?: string
      limit?: number
    }): Promise<ExecutionListResponse> =>
      await request('workflow', 'list_executions', params),

    listCheckpoints: async (executionId: string): Promise<CheckpointListResponse> =>
      await request('workflow', 'list_checkpoints', { execution_id: executionId }),

    getCheckpoint: async (
      executionId: string,
      checkpointId: string,
    ): Promise<{ checkpoint: Checkpoint }> =>
      await request('workflow', 'get_checkpoint', {
        execution_id: executionId,
        checkpoint_id: checkpointId,
      }),
  }
}

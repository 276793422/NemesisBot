# LLM Forward to Peer Chat Migration Guide

## Overview

This document describes the migration from `llm_forward` to `peer_chat` action for node-to-node communication in the NemesisBot cluster.

## Why This Change?

The original name `llm_forward` was semantically incorrect:
- **Implied**: Node A → [third-party forwarder] → Node B
- **Actual**: Node A ⟷ Node B (direct peer-to-peer communication)

The new name `peer_chat` better represents:
- Nodes are equal partners (peers)
- Direct intelligent agent collaboration
- No intermediate forwarding through third parties

## API Changes

### Old Action: `llm_forward`

**Request Format:**
```json
{
  "channel": "rpc",
  "chat_id": "user-123",
  "content": "Hello from Node A",
  "sender_id": "node-a",
  "session_key": "session-abc",
  "metadata": {
    "source": "rpc"
  }
}
```

**Response Format:**
```json
{
  "success": true,
  "content": "LLM response",
  "metadata": {...}
}
```

### New Action: `peer_chat`

**Request Format:**
```json
{
  "type": "request",
  "content": "Hello from Node A",
  "context": {
    "chat_id": "user-123",
    "sender_id": "node-a",
    "session_key": "session-abc"
  }
}
```

**Response Format:**
```json
{
  "status": "success",
  "response": "LLM response"
}
```

## Key Differences

| Aspect | llm_forward | peer_chat |
|--------|-------------|-----------|
| **Parameters** | 6 top-level fields | 3 top-level fields |
| **Required Fields** | chat_id, content | content only |
| **Optional Fields** | sender_id, session_key, metadata, channel | type, context (all fields) |
| **Conversation Types** | Single type | 4 types: chat, request, task, query |
| **Default Type** | N/A | request |
| **Response** | success + content | status + response |
| **Error Handling** | success: false | status: error |

## Supported Conversation Types

The new `peer_chat` action supports 4 types of conversations:

1. **chat**: Casual conversation between peers
2. **request**: Requesting help or assistance (default)
3. **task**: Collaborative task execution
4. **query**: Information querying

Example:
```json
{
  "type": "task",
  "content": "Please analyze this data and provide insights",
  "context": {
    "chat_id": "user-456"
  }
}
```

## Migration Steps

### 1. Update Action Name

Change the action name in RPC calls:
```go
// Old
result := peer.Call(ctx, "llm_forward", payload)

// New
result := peer.Call(ctx, "peer_chat", payload)
```

### 2. Restructure Request Payload

**Before:**
```go
payload := map[string]interface{}{
    "channel": "rpc",
    "chat_id": "user-123",
    "content": "Help me with this task",
    "sender_id": "node-a",
    "session_key": "session-abc",
}
```

**After:**
```go
payload := map[string]interface{}{
    "type": "request",  // Optional, defaults to "request"
    "content": "Help me with this task",
    "context": map[string]interface{}{
        "chat_id": "user-123",
        "sender_id": "node-a",
        "session_key": "session-abc",
    },
}
```

### 3. Update Response Handling

**Before:**
```go
if success, ok := result["success"].(bool); ok && success {
    content := result["content"].(string)
    // Process content
} else {
    errMsg := result["error"].(string)
    // Handle error
}
```

**After:**
```go
if status, ok := result["status"].(string); ok && status == "success" {
    response := result["response"].(string)
    // Process response
} else {
    errMsg := result["response"].(string)  // Error in response field
    // Handle error
}
```

### 4. Handler Registration

**Before:**
```go
handlers.RegisterLLMHandlers(logger, rpcChannel, handlerFactory, registrar)
```

**After:**
```go
handlers.RegisterPeerChatHandlers(logger, rpcChannel, handlerFactory, registrar)
```

Note: `RegisterLLMHandlers` is still available as a deprecated alias for backward compatibility.

## Backward Compatibility

**Important:** This migration does NOT support backward compatibility with the old `llm_forward` format. All nodes must be updated to use `peer_chat` simultaneously.

### Cluster Upgrade Strategy

1. **Stop all nodes** in the cluster
2. **Deploy the new version** to all nodes
3. **Start all nodes** together
4. **Verify communication** between nodes

## Testing

### Unit Tests

Run unit tests for the peer_chat handler:
```bash
go test ./test/unit/cluster/rpc/ -v -run TestPeerChat
go test ./test/unit/cluster/handlers/ -v -run TestPeerChat
```

### Integration Tests

Test peer-to-peer communication:
```bash
go test ./test/integration/ -v -run TestPeerChat
```

### Manual Testing

Test communication between two nodes:

1. Start Node A:
```go
clusterA := cluster.NewCluster(configA)
clusterA.Start(ctx)
```

2. Start Node B:
```go
clusterB := cluster.NewCluster(configB)
clusterB.Start(ctx)
```

3. Node A calls Node B:
```go
payload := map[string]interface{}{
    "type": "chat",
    "content": "Hello from Node A",
    "context": map[string]interface{}{
        "chat_id": "test-user",
    },
}
result := clusterA.CallPeer(ctx, nodeBID, "peer_chat", payload)
```

## Rollback Plan

If issues occur after migration:

1. **Stop all nodes**
2. **Revert to previous version** that uses `llm_forward`
3. **Investigate issues** in the new version
4. **Fix issues** and retry migration

## Timeline

- **2026-03-05**: Migration completed
- **Old action**: `llm_forward` (deprecated)
- **New action**: `peer_chat` (current)

## Support

For questions or issues related to this migration:
- Check the implementation plan: `docs/PEER_CHAT_IMPLEMENTATION_PLAN.md`
- Review the handler code: `module/cluster/rpc/peer_chat_handler.go`
- Run tests to verify: `go test ./test/unit/cluster/rpc/ -v`

## Summary

| Feature | Old (llm_forward) | New (peer_chat) |
|---------|-------------------|-----------------|
| Semantic Accuracy | ❌ Implies forwarding | ✅ P2P communication |
| Parameter Count | 6 fields | 3 fields |
| Required Fields | chat_id, content | content only |
| Conversation Types | 1 type | 4 types |
| Simplicity | Complex | Simple |
| Default Behavior | N/A | request type |

The `peer_chat` action provides a cleaner, more semantic API for node-to-node intelligent agent collaboration.

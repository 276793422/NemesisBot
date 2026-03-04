// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

// Logger represents logging functions for RPC operations
type Logger interface {
	RPCInfo(format string, args ...interface{})
	RPCError(format string, args ...interface{})
	RPCDebug(format string, args ...interface{})
}

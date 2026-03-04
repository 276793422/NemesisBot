// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import (
	"fmt"
	"time"
)

// RegisterCustomHandlers registers custom business logic RPC handlers.
// These handlers provide application-specific functionality beyond the system defaults.
//
// Parameters:
//   - logger: The logger for logging messages
//   - getNodeID: Function to get the current node ID
//   - registrar: Function to register handlers with the RPC server
func RegisterCustomHandlers(logger Logger, getNodeID func() string, registrar Registrar) {
	// Register hello handler
	registrar("hello", func(payload map[string]interface{}) (map[string]interface{}, error) {
		// Extract request information
		from := ""
		if fromVal, ok := payload["from"].(string); ok {
			from = fromVal
		}

		timestamp := ""
		if tsVal, ok := payload["timestamp"].(string); ok {
			timestamp = tsVal
		}

		logger.LogRPCInfo("Hello handler: Received hello from %s at %s", from, timestamp)

		// Build response
		response := map[string]interface{}{
			"greeting":  fmt.Sprintf("Hello! Received your greeting from %s", from),
			"timestamp": time.Now().Format(time.RFC3339),
			"node_id":   getNodeID(),
			"status":    "ok",
		}

		logger.LogRPCInfo("Hello handler: Sending response to %s", from)

		return response, nil
	})

	logger.LogRPCInfo("Registered custom handlers: hello")
}

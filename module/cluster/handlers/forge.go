// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import "time"

// ForgeDataProvider provides the interface for forge data operations.
// This decouples the handler from the forge package.
type ForgeDataProvider interface {
	ReceiveReflection(payload map[string]interface{}) error
	GetReflectionsListPayload() map[string]interface{}
	ReadReflectionContent(filename string) (string, error)
	SanitizeContent(content string) string
}

// RegisterForgeHandlers registers forge-related RPC handlers for cluster learning.
//
// Parameters:
//   - logger: The logger for logging messages
//   - provider: The forge data provider for receiving and listing reflections
//   - getNodeID: Function to get the current node ID
//   - registrar: Function to register handlers with the RPC server
func RegisterForgeHandlers(logger Logger, provider ForgeDataProvider, getNodeID func() string, registrar Registrar) {
	// forge_share: receive a remote reflection report
	registrar("forge_share", func(payload map[string]interface{}) (map[string]interface{}, error) {
		from := ""
		if fromVal, ok := payload["from"].(string); ok {
			from = fromVal
		}

		logger.LogRPCInfo("Forge handler: Receiving reflection from %s", from)

		if err := provider.ReceiveReflection(payload); err != nil {
			logger.LogRPCError("Forge handler: Failed to receive reflection: %v", err)
			return map[string]interface{}{
				"status": "error",
				"error":  err.Error(),
			}, err
		}

		logger.LogRPCInfo("Forge handler: Reflection received successfully from %s", from)

		return map[string]interface{}{
			"status":    "ok",
			"message":   "Reflection received",
			"node_id":   getNodeID(),
			"timestamp": time.Now().Format(time.RFC3339),
		}, nil
	})

	// forge_get_reflections: list available local reflections (or get specific one)
	registrar("forge_get_reflections", func(payload map[string]interface{}) (map[string]interface{}, error) {
		from := ""
		if fromVal, ok := payload["from"].(string); ok {
			from = fromVal
		}

		logger.LogRPCInfo("Forge handler: Reflections list requested by %s", from)

		result := provider.GetReflectionsListPayload()

		// If a specific reflection is requested, include its content (sanitized)
		if filename, ok := payload["filename"].(string); ok && filename != "" {
			content, err := provider.ReadReflectionContent(filename)
			if err != nil {
				logger.LogRPCError("Forge handler: Failed to read reflection %s: %v", filename, err)
				return map[string]interface{}{
					"status": "error",
					"error":  err.Error(),
				}, nil
			}
			result["content"] = provider.SanitizeContent(content)
			result["filename"] = filename
		}

		result["node_id"] = getNodeID()

		return result, nil
	})

	logger.LogRPCInfo("Registered forge handlers: forge_share, forge_get_reflections")
}

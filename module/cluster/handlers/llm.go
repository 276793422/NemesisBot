// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import (
	"github.com/276793422/NemesisBot/module/channels"
)

// RegisterLLMHandlers registers LLM-related RPC handlers.
// This function should be called after the RPCChannel is set up.
//
// Parameters:
//   - logger: The logger for logging messages
//   - rpcChannel: The RPC channel for sending requests to the LLM
//   - handlerFactory: Factory function to create the LLM handler
//   - registrar: Function to register handlers with the RPC server
func RegisterLLMHandlers(
	logger Logger,
	rpcChannel *channels.RPCChannel,
	handlerFactory func(*channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error),
	registrar Registrar,
) {
	// Create and register the LLM forward handler using the factory
	llmForwardHandler := handlerFactory(rpcChannel)
	registrar("llm_forward", llmForwardHandler)

	logger.LogRPCInfo("Registered LLM handlers: llm_forward")
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import (
	"github.com/276793422/NemesisBot/module/channels"
)

// RegisterPeerChatHandlers registers peer chat RPC handlers.
// This function should be called after the RPCChannel is set up.
//
// Parameters:
//   - logger: The logger for logging messages
//   - rpcChannel: The RPC channel for sending requests to the LLM
//   - handlerFactory: Factory function to create the peer chat handler
//   - registrar: Function to register handlers with the RPC server
func RegisterPeerChatHandlers(
	logger Logger,
	rpcChannel *channels.RPCChannel,
	handlerFactory func(*channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error),
	registrar Registrar,
) {
	// Create and register the peer chat handler using the factory
	peerChatHandler := handlerFactory(rpcChannel)
	registrar("peer_chat", peerChatHandler)

	logger.LogRPCInfo("Registered peer chat handler: peer_chat")
}

// RegisterLLMHandlers is an alias for RegisterPeerChatHandlers for backward compatibility.
// Deprecated: Use RegisterPeerChatHandlers instead.
func RegisterLLMHandlers(
	logger Logger,
	rpcChannel *channels.RPCChannel,
	handlerFactory func(*channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error),
	registrar Registrar,
) {
	RegisterPeerChatHandlers(logger, rpcChannel, handlerFactory, registrar)
}

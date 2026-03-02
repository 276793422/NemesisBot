# Cluster Configuration Guide

## Overview

The cluster module configuration is stored in `workspace/config/config.cluster.json`. This file is separate from the main `config.json` to allow each workspace to have its own cluster settings.

## Default Configuration

The default cluster configuration is embedded in the binary at `module/cluster/config.cluster.default.json`:

```json
{
  "enabled": false,
  "port": 49100,
  "rpc_port": 49200,
  "broadcast_interval": 30
}
```

When you start a bot, if `workspace/config/config.cluster.json` doesn't exist, it will use this default configuration (cluster disabled).

## Configuration File Location

```
workspace/
└── config/
    ├── config.cluster.json    ← Your cluster configuration (optional)
    └── config.json            ← Main bot configuration
```

## Configuration Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable/disable cluster functionality |
| `port` | integer | `49100` | UDP port for discovery broadcast |
| `rpc_port` | integer | `49200` | WebSocket port for RPC |
| `broadcast_interval` | integer | `30` | Broadcast interval in seconds |

## Environment Variables

You can override configuration with environment variables:

| Environment Variable | Description |
|---------------------|-------------|
| `CLUSTER_ENABLED` | Enable cluster (true/false) |
| `CLUSTER_UDP_PORT` | UDP port for discovery |
| `CLUSTER_RPC_PORT` | RPC port for WebSocket |
| `CLUSTER_BROADCAST_INTERVAL` | Broadcast interval in seconds |

## How It Works

When cluster is enabled:

1. **Discovery (UDP 49100)**
   - Bot listens for UDP broadcasts from other bots
   - Bot broadcasts its presence every 30 seconds
   - Discovered bots are added to the registry

2. **RPC (WebSocket 49200)**
   - Bot starts WebSocket RPC server
   - Other bots can make RPC calls
   - LLM can use `cluster_rpc` tool to communicate with other bots

3. **State Persistence**
   - Runtime state saved to `workspace/cluster/peers.toml`
   - Contains discovered nodes and their status
   - Updated periodically and on discovery events

4. **Logging**
   - Logs written to `.nemesisbot/workspace/logs/cluster/`
   - `discovery.log` - UDP discovery logs
   - `rpc.log` - WebSocket RPC logs

## Usage

### Enable Cluster

1. Create or edit `workspace/config/config.cluster.json`:
```json
{
  "enabled": true,
  "port": 49100,
  "broadcast_interval": 30
}
```

2. Start your bot:
```bash
./nemesisbot gateway
```

3. The bot will:
   - Generate a unique node ID
   - Start listening on UDP 49100
   - Start broadcasting every 30 seconds
   - Start WebSocket RPC server on port 49200

### Use with LLM

Once cluster is enabled, the LLM can use the `cluster_rpc` tool:

```
User: Call bot-2 and get its capabilities

LLM: [Uses cluster_rpc tool]
{
  "peer_id": "bot-2",
  "action": "get_capabilities"
}
```

### Multiple Bots

Run multiple bot instances on the same network:

```bash
# Terminal 1
cd /path/to/bot1
./nemesisbot gateway

# Terminal 2
cd /path/to/bot2
./nemesisbot gateway

# Terminal 3
cd /path/to/bot3
./nemesisbot gateway
```

Each bot will automatically discover the others and can communicate via RPC.

## Architecture

```
┌─────────────────────────────────────────────┐
│  Bot 1 (192.168.1.100)                      │
│  - UDP Discovery: 49100                      │
│  - WebSocket RPC: 49200                      │
│  - config.cluster.json                       │
└─────────────────────────────────────────────┘
                    ↕ UDP Broadcast
┌─────────────────────────────────────────────┐
│  Bot 2 (192.168.1.101)                      │
│  - UDP Discovery: 49100                      │
│  - WebSocket RPC: 49200                      │
└─────────────────────────────────────────────┘
                    ↕ WebSocket RPC
┌─────────────────────────────────────────────┐
│  Bot 3 (192.168.1.102)                      │
└─────────────────────────────────────────────┘
```

## Troubleshooting

### Bots not discovering each other

1. Check firewall settings (UDP 49100 must be open)
2. Ensure bots are on the same network
3. Check logs: `.nemesisbot/workspace/logs/cluster/discovery.log`

### RPC calls failing

1. Ensure both bots have cluster enabled
2. Check peer status in `workspace/cluster/peers.toml`
3. Check logs: `.nemesisbot/workspace/logs/cluster/rpc.log`

### Port conflicts

Change the port in `config.cluster.json`:
```json
{
  "enabled": true,
  "port": 49101
}
```

Make sure all bots use the same port.

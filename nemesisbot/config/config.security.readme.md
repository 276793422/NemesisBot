# Security Configuration Guide

## ⚠️ IMPORTANT (2026-02-24)

The `ask` action is **currently mapped to `deny`** for security.

### What This Means

If you have rules with `"action": "ask"` in this config file, they will **BLOCK** operations.

### Example

```json
{
  "pattern": "*.log",
  "action": "ask"  // Currently behaves as "deny" - blocks the operation
}
```

### Rules with `ask` Action

The following rules in `config.security.default.json` use `ask`:

| Category | Pattern | Current Behavior |
|----------|---------|------------------|
| file_read | `*.log` | ❌ Blocked |
| process_spawn | `*` | ❌ Blocked |
| network_download | `*` | ❌ Blocked |
| network_upload | `*` | ❌ Blocked |
| registry_write | `HKEY_LOCAL_MACHINE/**` | ❌ Blocked |
| registry_write | `HKEY_CURRENT_USER/**` | ❌ Blocked |

### Workaround

To allow these operations temporarily, change `"action": "ask"` to `"action": "allow"`:

```json
{
  "pattern": "*.log",
  "action": "allow"  // Explicitly allow instead of ask
}
```

### Future Plans

When the interactive approval workflow is implemented:
- `ask` will prompt user for permission
- Commands like `nemesisbot security approve <id>` will be functional
- No configuration changes will be needed

### More Information

See `module/security/README.md` for complete documentation.

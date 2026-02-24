# Security Audit Framework - Configuration and Usage Guide

## Overview

> **⚠️ IMPORTANT NOTICE (2026-02-24)**
>
> The `ask` action is currently **mapped to `deny`** for security reasons.
> - Rules with `"action": "ask"` will **block** operations
> - This is temporary until the interactive approval workflow is implemented
> - See [Important Notes](#-important-notes) section for details
>
> **Workaround:** Change `"action": "ask"` to `"action": "allow"` in config if needed

The Security Audit Framework provides centralized security controls for all dangerous operations in NemesisBot, similar to how antivirus software monitors system operations.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Security Auditor                         │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────┐     │
│  │  Policies   │  │ Permissions  │  │  Audit Log      │     │
│  └─────────────┘  └──────────────┘  └─────────────────┘     │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                  Security Middleware                        │
│  ┌──────────┐ ┌──────────┐ ┌─────────┐ ┌──────────────┐     │
│  │   File   │ │ Process  │ │ Network │ │   Hardware   │     │
│  │ Wrapper  │ │ Wrapper  │ │ Wrapper │ │   Wrapper    │     │
│  └──────────┘ └──────────┘ └─────────┘ └──────────────┘     │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                       Tools Layer                           │
│  exec_tool, filesystem, web_fetch, i2c, spi, etc.           │
└─────────────────────────────────────────────────────────────┘
```

## Operation Types and Danger Levels

| Category | Operation Type | Danger Level | Description |
|----------|----------------|--------------|-------------|
| **File** | `file_read` | LOW | Reading file contents |
| **File** | `file_write` | HIGH | Creating/overwriting files |
| **File** | `file_delete` | HIGH | Deleting files |
| **File** | `file_edit` | MEDIUM | Editing existing files |
| **File** | `file_append` | MEDIUM | Appending to files |
| **File** | `dir_create` | HIGH | Creating directories |
| **File** | `dir_delete` | HIGH | Deleting directories |
| **File** | `dir_list` | LOW | Listing directory contents |
| **Process** | `process_exec` | CRITICAL | Executing shell commands |
| **Process** | `process_spawn` | HIGH | Spawning new processes |
| **Process** | `process_kill` | CRITICAL | Terminating processes |
| **Registry** | `registry_read` | MEDIUM | Reading registry (Windows) |
| **Registry** | `registry_write` | CRITICAL | Writing registry |
| **Registry** | `registry_delete` | CRITICAL | Deleting registry entries |
| **Network** | `network_download` | MEDIUM | Downloading files |
| **Network** | `network_upload` | MEDIUM | Uploading files |
| **Network** | `network_request` | LOW | Making HTTP requests |
| **Hardware** | `hardware_i2c` | LOW | I2C bus operations |
| **Hardware** | `hardware_spi` | LOW | SPI bus operations |
| **Hardware** | `hardware_gpio` | LOW | GPIO operations |
| **System** | `system_shutdown` | CRITICAL | Shutting down system |
| **System** | `system_reboot` | CRITICAL | Rebooting system |
| **System** | `system_config` | HIGH | Modifying system config |
| **System** | `system_service` | HIGH | Managing services |
| **System** | `system_install` | CRITICAL | Installing software |

## Configuration Example

### 1. Basic Setup

```go
package main

import (
    "github.com/276793422/NemesisBot/module/security"
)

func main() {
    // Create security auditor
    config := &security.AuditorConfig{
        Enabled:               true,
        LogAllOperations:      true,
        LogDenialsOnly:        false,
        ApprovalTimeout:       5 * time.Minute,
        MaxPendingRequests:    100,
        AuditLogRetentionDays: 90,
        AuditLogPath:          "/var/log/nemesisbot/audit.csv",
        SynchronousMode:       false,
        PolicyEngine:          "abac",
    }

    auditor := security.NewSecurityAuditor(config)

    // Create default policies
    security.CreateDefaultPolicies(auditor)

    // Set permissions for different users
    auditor.SetPermission("cli", security.CreateCLIPermission())
    auditor.SetPermission("web", security.CreateWebPermission())
    auditor.SetPermission("agent:default", security.CreateAgentPermission("default"))

    // Use the security middleware
    middleware := security.NewSecurityMiddleware(auditor, "user_id", "cli", "/workspace")
}
```

### 2. Custom Policy Example

```go
// Create a policy to protect sensitive files
protectConfigPolicy := &security.Policy{
    Name:        "protect_config",
    Description: "Protect configuration files",
    Enabled:     true,
    Rules: []security.PolicyRule{
        {
            Name:        "block_config_write",
            MatchOpType: security.OpFileWrite,
            MatchTarget: "\\.json$|\\.yaml$|\\.toml$",
            MatchSource: "web|telegram",
            Action:      "deny",
            Reason:      "Config file writes from web/telegram are blocked",
        },
        {
            Name:        "require_approval",
            MatchOpType: security.OpFileDelete,
            MatchTarget: "\\.md$",
            Action:      "require_approval",
            Reason:      "Deleting markdown files requires approval",
        },
    },
    DefaultAction: "allow",
}

auditor.RegisterPolicy(protectConfigPolicy)
```

### 3. Permission Profiles

```go
// Restrictive profile for web users
webProfile := &security.Permission{
    AllowedTypes: map[security.OperationType]bool{
        security.OpFileRead:   true,
        security.OpFileWrite:  true,
        security.OpDirList:    true,
    },
    AllowedTargets: []string{
        "^/workspace/",  // Only workspace directory
    },
    DeniedTargets: []string{
        "\\.env$",           // Environment files
        "\\.key$",           // Private keys
        "\\.pem$",           // Certificates
    },
    RequireApproval: map[security.OperationType]bool{
        security.OpFileDelete:   true,
        security.OpNetworkDownload: true,
    },
    MaxDangerLevel: security.DangerMedium,
}

auditor.SetPermission("web", webProfile)
```

## Integration with Existing Tools

### Example: Wrapping the Exec Tool

```go
// module/tools/shell.go - Modified

import "github.com/276793422/NemesisBot/module/security"

type ExecTool struct {
    workingDir     string
    timeout        time.Duration
    denyPatterns   []*regexp.Regexp
    allowPatterns  []*regexp.Regexp
    securityMW     *security.SecurityMiddleware  // NEW
}

func NewExecToolWithSecurity(
    workingDir string,
    restrict bool,
    config *config.Config,
    securityMW *security.SecurityMiddleware,  // NEW
) *ExecTool {
    return &ExecTool{
        workingDir:     workingDir,
        timeout:        60 * time.Second,
        denyPatterns:   defaultDenyPatterns,
        allowPatterns:  nil,
        securityMW:     securityMW,  // NEW
    }
}

func (t *ExecTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    command, _ := args["command"].(string)

    // NEW: Use security wrapper
    processWrapper := t.securityMW.Process()
    output, err := processWrapper.ExecuteCommand(command)
    if err != nil {
        return ErrorResult(err.Error())
    }

    return NewToolResult(output)
}
```

### Example: Wrapping File Operations

```go
// module/tools/filesystem.go - Modified

import "github.com/276793422/NemesisBot/module/security"

type WriteFileTool struct {
    workspace    string
    restrict     bool
    securityMW   *security.SecurityMiddleware  // NEW
}

func (t *WriteFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    path, _ := args["path"].(string)
    content, _ := args["content"].(string)

    // NEW: Use security wrapper
    fileWrapper := t.securityMW.File()
    err := fileWrapper.WriteFile(path, []byte(content))
    if err != nil {
        return ErrorResult(err.Error())
    }

    return SilentResult(fmt.Sprintf("File written: %s", path))
}
```

## Audit and Monitoring

### Viewing Audit Logs

```go
// Get all audit events
log := middleware.GetAuditLog(security.AuditFilter{})

// Filter by user
userLog := middleware.GetAuditLog(security.AuditFilter{
    User: "cli",
})

// Filter by operation type
fileOpsLog := middleware.GetAuditLog(security.AuditFilter{
    OperationType: security.OpFileWrite,
})

// Filter by decision
deniedLog := middleware.GetAuditLog(security.AuditFilter{
    Decision: "denied",
})

// Filter by time range
start := time.Now().Add(-24 * time.Hour)
recentLog := middleware.GetAuditLog(security.AuditFilter{
    StartTime: &start,
})

// Export to CSV
middleware.ExportAuditLog("/var/log/nemesisbot/audit.csv")
```

### Monitoring Pending Approvals

```go
// Get all pending approval requests
pending := middleware.GetSecuritySummary()["pending"].([]map[string]interface{})

for _, req := range pending {
    fmt.Printf("Request ID: %s\n", req["id"])
    fmt.Printf("  Type: %s\n", req["type"])
    fmt.Printf("  Target: %s\n", req["target"])
    fmt.Printf("  Danger: %s\n", req["danger"])

    // Approve or deny
    middleware.ApprovePendingRequest(req["id"].(string))
    // OR
    middleware.DenyPendingRequest(req["id"].(string), "Reason for denial")
}
```

### Real-time Monitoring

```go
// Start security monitoring in background
ctx, cancel := context.WithCancel(context.Background())
defer cancel()

go security.MonitorSecurityStatus(ctx, auditor, 5*time.Minute)
```

## Decision Flow

```
┌─────────────────────────────────────────────────────────────┐
│  1. Operation Requested                                     │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  2. Check Default Deny Patterns                             │
│     - Does target match dangerous pattern?                  │
│     - Block if yes                                          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  3. Check User Permissions                                  │
│     - Is operation type allowed?                            │
│     - Is target in whitelist/blacklist?                     │
│     - Is danger level within limit?                         │
│     - Does operation require approval?                      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  4. Evaluate Policies                                       │
│     - For each enabled policy                               │
│     - Check if rules match                                  │
│     - Apply matching rule action:                           │
│       • allow → Execute                                     │
│       • deny → Block                                        │
│       • ask → Block (temporarily mapped to deny)            │
│         ⚠️  See "Important Notes" section below              │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  5. Decision                                                │
│     - ALLOW: Execute operation                              │
│     - DENY: Block with reason                               │
│     - ASK: Block with reason (currently, see notes)         │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  6. Log to Audit Trail                                      │
└─────────────────────────────────────────────────────────────┘
```

## ⚠️ Important Notes

### Current Status of "ask" Action

**As of 2026-02-24, the `ask` action is mapped to `deny` for security reasons.**

| Action | Current Behavior | Future Behavior |
|--------|-----------------|-----------------|
| `allow` | ✅ Allows operation | ✅ Allows operation |
| `deny` | ❌ Blocks operation | ❌ Blocks operation |
| `ask` | ❌ **Blocks operation** (temporarily) | 🔜 **Will prompt for approval** |

#### Why This Mapping?

The approval workflow (prompting user interactively) is not yet implemented. To maintain security:
- Rules with `ask` action **block the operation** (treated as `deny`)
- This prevents operations from executing without proper approval
- Once approval UI is implemented, `ask` will be restored to its intended behavior

#### Impact on Current Configuration

If your `config.security.json` contains rules with `"action": "ask"`, they will currently **deny** the operation:

```json
{
  "file_read": [
    {
      "pattern": "*.log",
      "action": "ask"  // Currently treated as "deny"
    }
  ]
}
```

**Workaround:** To allow these operations temporarily, change `"action": "ask"` to `"action": "allow"`.

#### Future Implementation

When the approval workflow is completed:
1. Interactive prompts will ask user for permission
2. `ask` will be mapped back to `require_approval`
3. Commands like `nemesisbot security approve <id>` will be functional

#### Code Reference

See `module/security/auditor.go:normalizeDecision()`:
```go
case "ask":
    // TEMPORARY: Map ask to denied until approval UI is implemented
    // This prevents operations that require approval from executing
    return "denied"
```

---

## Security Best Practices

### 1. Principle of Least Privilege

```go
// Different permission profiles for different contexts
permissions := map[string]*security.Permission{
    "cli":    security.CreateCLIPermission(),        // Full access
    "web":    security.CreateWebPermission(),        // Restricted
    "agent":  security.CreateAgentPermission("bot"), // Context-aware
    "cron":   createCronPermission(),                // Minimal
}
```

### 2. Workspace Isolation

```go
// Always use workspace restrictions
middleware := security.NewSecurityMiddleware(
    auditor,
    userID,
    source,
    "/home/user/bot-workspace",  // Restrict to this directory
)
```

### 3. Require Approval for Critical Operations

```go
criticalPolicy := &security.Policy{
    Name:    "critical_ops",
    Enabled: true,
    Rules: []security.PolicyRule{
        {
            MatchOpType: security.OpProcessExec,
            MinDanger:   security.DangerHigh,
            Action:      "require_approval",
            Reason:      "High-danger commands need approval",
        },
    },
}
```

### 4. Regular Audit Review

```go
// Schedule regular audit log reviews
func scheduleAuditReview(auditor *security.Auditor) {
    ticker := time.NewTicker(24 * time.Hour)
    for range ticker.C {
        log := auditor.GetAuditLog(security.AuditFilter{
            StartTime: time.Now().Add(-24 * time.Hour),
        })

        // Review denied operations
        for _, event := range log {
            if event.Decision == "denied" {
                alertSecurityTeam(event)
            }
        }

        // Export logs
        auditor.ExportAuditLog(fmt.Sprintf(
            "/var/log/nemesisbot/audit-%s.csv",
            time.Now().Format("2006-01-02"),
        ))
    }
}
```

### 5. Enable All Logging

```go
config := &security.AuditorConfig{
    Enabled:          true,
    LogAllOperations: true,  // Log everything
    LogDenialsOnly:   false,  // Not just denials
}
```

## Migration Guide

### Step 1: Initialize Security Auditor

In your main.go or initialization code:

```go
import "github.com/276793422/NemesisBot/module/security"

func initSecurity(cfg *config.Config) *security.SecurityAuditor {
    config := &security.AuditorConfig{
        Enabled:               true,
        LogAllOperations:      true,
        ApprovalTimeout:       5 * time.Minute,
        AuditLogRetentionDays: 90,
    }

    auditor := security.NewSecurityAuditor(config)
    security.CreateDefaultPolicies(auditor)

    // Set up permissions
    auditor.SetPermission("cli", security.CreateCLIPermission())
    auditor.SetPermission("web", security.CreateWebPermission())

    return auditor
}
```

### Step 2: Pass Security Middleware to Tools

```go
// When creating tools
fileWrapper := security.NewSecureFileWrapper(
    auditor,
    userID,
    source,
    workspace,
)

// Modify tools to use wrapper
writeTool := &WriteFileTool{
    workspace:  workspace,
    restrict:   true,
    fileWrapper: fileWrapper,  // NEW
}
```

### Step 3: Update Tool Execute Methods

```go
// Before (old code)
func (t *WriteFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    path, _ := args["path"].(string)
    content, _ := args["content"].(string)
    return os.WriteFile(path, []byte(content), 0644)
}

// After (with security)
func (t *WriteFileTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    path, _ := args["path"].(string)
    content, _ := args["content"].(string)
    return t.fileWrapper.WriteFile(path, []byte(content))
}
```

## Testing

Run the security tests:

```bash
cd /path/to/NemesisBot
go test ./module/security/... -v
```

Expected output:
```
=== RUN   TestSecurityAuditor_BasicOperation
--- PASS: TestSecurityAuditor_BasicOperation (0.00s)
=== RUN   TestSecurityAuditor_PolicyRules
--- PASS: TestSecurityAuditor_PolicyRules (0.00s)
...
PASS
ok      github.com/276793422/NemesisBot/module/security    0.123s
```

## Troubleshooting

### Issue: Operations are being blocked unexpectedly

**Solution**: Check the audit log to see why:
```go
log := middleware.GetAuditLog(AuditFilter{Decision: "denied"})
for _, event := range log {
    fmt.Printf("Denied: %s - %s\n", event.Request.Target, event.Reason)
}
```

### Issue: Want to temporarily disable security

**Warning**: Only do this in development!
```go
auditor.Disable()
// ... perform operations ...
auditor.Enable()
```

### Issue: Too many pending approvals

**Solution**: Adjust permissions to auto-allow more operations:
```go
perm := security.CreateCLIPermission()
// Remove from require approval
delete(perm.RequireApproval, security.OpFileDelete)
auditor.SetPermission("cli", perm)
```

## Advanced Features

### Custom Danger Level Calculation

```go
func customDangerLevel(op OperationType, target string) DangerLevel {
    baseLevel := GetDangerLevel(op)

    // Increase danger level for sensitive paths
    if strings.Contains(target, "/etc/") ||
       strings.Contains(target, "C:\\Windows") {
        if baseLevel < DangerHigh {
            return DangerHigh
        }
    }

    return baseLevel
}
```

### Time-Based Policies

```go
// Only allow dangerous operations during business hours
func businessHoursPolicy(req *OperationRequest) bool {
    hour := time.Now().Hour()
    isBusinessHours := hour >= 9 && hour < 17

    if req.DangerLevel >= DangerHigh && !isBusinessHours {
        return false
    }
    return true
}
```

### Multi-User Approval

```go
// Require multiple approvers for critical operations
criticalPolicy := &security.Policy{
    Name:    "multi_approver",
    Enabled: true,
    Rules: []security.PolicyRule{
        {
            MatchOpType: security.OpSystemShutdown,
            Action:      "require_approval",
            Reason:      "Requires 2 approvers",
        },
    },
}

// In your approval handler:
func approveWithMultiple(requestID string, approvers []string) error {
    approvals := 0
    for _, approver := range approvers {
        if err := auditor.ApproveRequest(requestID, approver); err == nil {
            approvals++
        }
    }

    if approvals < 2 {
        return fmt.Errorf("need at least 2 approvals, got %d", approvals)
    }
    return nil
}
```

## File Structure

```
module/security/
├── auditor.go              # Main security auditor implementation
├── middleware.go           # Wrappers and middleware
├── auditor_test.go         # Test suite
├── policies.go             # Predefined policies (optional)
└── README.md              # This file
```

## Performance Considerations

- **Synchronous Mode**: Set `SynchronousMode=false` for non-blocking operation
- **Log Filtering**: Use `LogDenialsOnly=true` in production to reduce log size
- **Audit Cleanup**: Enable automatic cleanup with `AuditLogRetentionDays`
- **In-Memory Logging**: Audit logs are kept in memory; export regularly

## Security Recommendations

1. **Always enable security in production**
2. **Use workspace restrictions**
3. **Require approval for critical operations**
4. **Review audit logs regularly**
5. **Export audit logs for long-term storage**
6. **Set up alerts for denied operations**
7. **Use different permission profiles for different contexts**
8. **Keep security framework updated**

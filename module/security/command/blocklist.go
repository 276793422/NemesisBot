// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package command provides dangerous command detection and blocking
package command

// DefaultBlocklist returns the built-in list of dangerous command patterns
// organised by category.  Every entry has a regex Pattern, a Category,
// a Severity, the Platform it applies to, and a human-readable Reason.
//
// Totals: 15 destructive + 10 network + 10 privilege + 10 recon = 45 entries.
func DefaultBlocklist() []BlockEntry {
	entries := make([]BlockEntry, 0, 50)

	// =================================================================
	// Category: destructive (15)
	// Commands that cause irreversible data or system destruction.
	// =================================================================
	entries = append(entries,
		BlockEntry{
			Pattern:  `\brm\s+(-[a-zA-Z]*[rf][a-zA-Z]*\s+|-[a-zA-Z]*f[a-zA-Z]*r[a-zA-Z]*\s+)(/\S*|\s*[/\\])`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "recursive force-delete from root or critical paths",
		},
		BlockEntry{
			Pattern:  `\brm\s+--no-preserve-root`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "bypass root preservation in rm",
		},
		BlockEntry{
			Pattern:  `\bformat\s+[a-zA-Z]:`,
			Category: "destructive",
			Severity: "critical",
			Platform: "windows",
			Reason:   "format a Windows drive letter",
		},
		BlockEntry{
			Pattern:  `\bdel\s+/[fF]\s+/[sS]\s+/[qQ]`,
			Category: "destructive",
			Severity: "critical",
			Platform: "windows",
			Reason:   "force-delete files recursively and quietly on Windows",
		},
		BlockEntry{
			Pattern:  `\brd\s+/[sS]\s+/[qQ]`,
			Category: "destructive",
			Severity: "critical",
			Platform: "windows",
			Reason:   "force-remove directory tree quietly on Windows",
		},
		BlockEntry{
			Pattern:  `\bshred\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "secure file deletion (overwrite before remove)",
		},
		BlockEntry{
			Pattern:  `\bdd\s+if=/dev/zero`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "zero-fill a device with dd",
		},
		BlockEntry{
			Pattern:  `\bdd\s+if=/dev/urandom`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "random-fill a device with dd",
		},
		BlockEntry{
			Pattern:  `\bmkfs\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "build a filesystem (destroys existing data)",
		},
		BlockEntry{
			Pattern:  `\bfdisk\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "manipulate disk partition table",
		},
		BlockEntry{
			Pattern:  `\bwipefs\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "wipe filesystem signatures from device",
		},
		BlockEntry{
			Pattern:  `\bparted\b.*\bmklabel\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "create new partition table (destroys existing)",
		},
		BlockEntry{
			Pattern:  `\bdiskpart\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "windows",
			Reason:   "Windows disk partition management tool",
		},
		BlockEntry{
			Pattern:  `\b(shutdown|poweroff|halt)\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "linux",
			Reason:   "shutdown or power off the system",
		},
		BlockEntry{
			Pattern:  `\bshutdown\b`,
			Category: "destructive",
			Severity: "critical",
			Platform: "windows",
			Reason:   "shutdown the Windows system",
		},
	)

	// =================================================================
	// Category: network (10)
	// Commands that establish unauthorized network services, tunnels,
	// or download-and-execute payloads.
	// =================================================================
	entries = append(entries,
		BlockEntry{
			Pattern:  `\bnmap\b.*(-s[STUV]*A|-A\b|--script\b)`,
			Category: "network",
			Severity: "high",
			Platform: "any",
			Reason:   "aggressive nmap scanning with script engine or OS detection",
		},
		BlockEntry{
			Pattern:  `\bnc\b.*(-[a-zA-Z]*l[a-zA-Z]*\s|-l\b)`,
			Category: "network",
			Severity: "critical",
			Platform: "any",
			Reason:   "netcat listen mode (potential reverse shell listener)",
		},
		BlockEntry{
			Pattern:  `\bncat\b.*(-[a-zA-Z]*l[a-zA-Z]*\s|-l\b)`,
			Category: "network",
			Severity: "critical",
			Platform: "any",
			Reason:   "ncat listen mode (potential reverse shell listener)",
		},
		BlockEntry{
			Pattern:  `\bcurl\b.*\|\s*(ba)?sh\b`,
			Category: "network",
			Severity: "critical",
			Platform: "any",
			Reason:   "download and execute script via curl piped to shell",
		},
		BlockEntry{
			Pattern:  `\bwget\b.*\|\s*(ba)?sh\b`,
			Category: "network",
			Severity: "critical",
			Platform: "any",
			Reason:   "download and execute script via wget piped to shell",
		},
		BlockEntry{
			Pattern:  `\bsocat\b.*(exec|system|fork)`,
			Category: "network",
			Severity: "critical",
			Platform: "linux",
			Reason:   "socat with exec/system (potential remote shell)",
		},
		BlockEntry{
			Pattern:  `\bpython[0-9.]*\s+-c\s+.*socket.*exec\b`,
			Category: "network",
			Severity: "critical",
			Platform: "any",
			Reason:   "Python reverse shell pattern",
		},
		BlockEntry{
			Pattern:  `\bpython[0-9.]*\s+-c\s+.*subprocess\b`,
			Category: "network",
			Severity: "high",
			Platform: "any",
			Reason:   "Python one-liner spawning subprocess",
		},
		BlockEntry{
			Pattern:  `\b(bash|sh|zsh)\s+-i\s*>&\s*/dev/tcp/`,
			Category: "network",
			Severity: "critical",
			Platform: "linux",
			Reason:   "bash reverse shell via /dev/tcp",
		},
		BlockEntry{
			Pattern:  `\bssh\s+-[a-zA-Z]*R\b`,
			Category: "network",
			Severity: "high",
			Platform: "any",
			Reason:   "SSH remote port forwarding (potential tunnel)",
		},
	)

	// =================================================================
	// Category: privilege (10)
	// Commands that escalate privileges or modify access controls.
	// =================================================================
	entries = append(entries,
		BlockEntry{
			Pattern:  `\bsudo\s+(su|-i)\b`,
			Category: "privilege",
			Severity: "critical",
			Platform: "linux",
			Reason:   "escalate to root shell via sudo su or sudo -i",
		},
		BlockEntry{
			Pattern:  `\bchmod\s+(777|000|[0-7]{3,4})\b`,
			Category: "privilege",
			Severity: "high",
			Platform: "linux",
			Reason:   "modify file permissions to wide-open or zero",
		},
		BlockEntry{
			Pattern:  `\bchown\s+\S+\s+\S+`,
			Category: "privilege",
			Severity: "high",
			Platform: "linux",
			Reason:   "change file ownership",
		},
		BlockEntry{
			Pattern:  `\bpasswd\b`,
			Category: "privilege",
			Severity: "critical",
			Platform: "linux",
			Reason:   "change user password",
		},
		BlockEntry{
			Pattern:  `\buseradd\b`,
			Category: "privilege",
			Severity: "high",
			Platform: "linux",
			Reason:   "create a new system user",
		},
		BlockEntry{
			Pattern:  `\busermod\b`,
			Category: "privilege",
			Severity: "high",
			Platform: "linux",
			Reason:   "modify a system user account",
		},
		BlockEntry{
			Pattern:  `\bvisudo\b`,
			Category: "privilege",
			Severity: "critical",
			Platform: "linux",
			Reason:   "edit sudoers configuration",
		},
		BlockEntry{
			Pattern:  `\bnet\s+(localgroup\s+)?administrators\b`,
			Category: "privilege",
			Severity: "critical",
			Platform: "windows",
			Reason:   "modify Windows administrators group",
		},
		BlockEntry{
			Pattern:  `\bicacls\b.*/grant\b.*:(F|O)\b`,
			Category: "privilege",
			Severity: "high",
			Platform: "windows",
			Reason:   "grant full/ownership permissions via icacls",
		},
		BlockEntry{
			Pattern:  `\brunas\b.*/user:(.*\\)?(admin|system)\b`,
			Category: "privilege",
			Severity: "high",
			Platform: "windows",
			Reason:   "run command as admin or system user",
		},
	)

	// =================================================================
	// Category: recon (10)
	// Commands used for system reconnaissance and credential harvesting.
	// =================================================================
	entries = append(entries,
		BlockEntry{
			Pattern:  `\bwhoami\s+(/priv|/all)\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "windows",
			Reason:   "enumerate Windows privileges of current user",
		},
		BlockEntry{
			Pattern:  `\bnet\s+user\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "windows",
			Reason:   "enumerate Windows user accounts",
		},
		BlockEntry{
			Pattern:  `\bnet\s+(localgroup|group)\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "windows",
			Reason:   "enumerate Windows groups and memberships",
		},
		BlockEntry{
			Pattern:  `\bcat\s+/etc/shadow\b`,
			Category: "recon",
			Severity: "critical",
			Platform: "linux",
			Reason:   "read shadow password file (password hashes)",
		},
		BlockEntry{
			Pattern:  `\b(cat|head|tail|less|more)\s+/etc/passwd\b.*\|`,
			Category: "recon",
			Severity: "high",
			Platform: "linux",
			Reason:   "read passwd file piped to another command",
		},
		BlockEntry{
			Pattern:  `\breg\s+query\s+(HKLM\\SAM|HKLM\\SECURITY|HKLM\\SYSTEM)\b`,
			Category: "recon",
			Severity: "critical",
			Platform: "windows",
			Reason:   "query Windows registry hive for SAM/SECURITY/SYSTEM",
		},
		BlockEntry{
			Pattern:  `\bsamtools\b.*\bdump\b`,
			Category: "recon",
			Severity: "high",
			Platform: "windows",
			Reason:   "dump Windows SAM database",
		},
		BlockEntry{
			Pattern:  `\bwmic\b.*(useraccount|group)\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "windows",
			Reason:   "enumerate user accounts or groups via WMI",
		},
		BlockEntry{
			Pattern:  `\b(get-acl|Get-Acl)\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "windows",
			Reason:   "PowerShell ACL enumeration",
		},
		BlockEntry{
			Pattern:  `\b(lsof|-i)\b.*\b(listen|LISTEN)\b`,
			Category: "recon",
			Severity: "medium",
			Platform: "linux",
			Reason:   "enumerate listening ports / open files",
		},
	)

	return entries
}

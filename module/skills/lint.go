// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"fmt"
	"regexp"
	"strings"
)

// DangerPattern represents a dangerous code pattern detected by the linter.
type DangerPattern struct {
	ID          string // unique identifier, e.g. "DEST-001"
	Name        string // human-readable name
	Category    string // "destructive", "exfiltration", "privilege", "obfuscation", "recon"
	Severity    string // "critical", "high", "medium", "low"
	Pattern     string // regex pattern string
	Description string
}

// LintIssue represents a single linting issue found in skill content.
type LintIssue struct {
	PatternID   string `json:"pattern_id"`
	Category    string `json:"category"`
	Severity    string `json:"severity"`
	Message     string `json:"message"`
	Line        int    `json:"line"`
	MatchedText string `json:"matched_text"`
}

// LintResult holds the result of linting a skill's content.
type LintResult struct {
	SkillName string      `json:"skill_name"`
	Passed    bool        `json:"passed"`
	Issues    []LintIssue `json:"issues"`
	Score     float64     `json:"score"` // 0-100, 100 = clean
}

// Linter performs static analysis on skill definitions to detect dangerous patterns.
type Linter struct {
	patterns []DangerPattern
	compiled []*regexp.Regexp // pre-compiled versions of patterns
}

// NewLinter creates a Linter with 27 built-in dangerous pattern detectors.
func NewLinter() *Linter {
	patterns := builtInDangerPatterns()
	compiled := make([]*regexp.Regexp, len(patterns))
	for i, p := range patterns {
		compiled[i] = regexp.MustCompile(p.Pattern)
	}
	return &Linter{
		patterns: patterns,
		compiled: compiled,
	}
}

// Lint analyzes the given skill content and returns a LintResult with all detected issues.
func (l *Linter) Lint(content string, skillName string) *LintResult {
	lines := strings.Split(content, "\n")
	var issues []LintIssue

	for lineNum, line := range lines {
		for i, pat := range l.patterns {
			locs := l.compiled[i].FindAllStringIndex(line, -1)
			for _, loc := range locs {
				matched := line[loc[0]:loc[1]]
				issues = append(issues, LintIssue{
					PatternID:   pat.ID,
					Category:    pat.Category,
					Severity:    pat.Severity,
					Message:     fmt.Sprintf("%s: %s", pat.Name, pat.Description),
					Line:        lineNum + 1,
					MatchedText: matched,
				})
			}
		}
	}

	if issues == nil {
		issues = []LintIssue{}
	}

	score := l.computeScore(issues)

	return &LintResult{
		SkillName: skillName,
		Passed:    score >= 60 && !hasCriticalOrHigh(issues),
		Issues:    issues,
		Score:     score,
	}
}

// computeScore derives a 0-100 score from the list of issues.
// No issues = 100. Penalties: critical -40, high -25, medium -15, low -5 each.
func (l *Linter) computeScore(issues []LintIssue) float64 {
	if len(issues) == 0 {
		return 100
	}

	var penalty float64
	for _, issue := range issues {
		switch issue.Severity {
		case "critical":
			penalty += 40
		case "high":
			penalty += 25
		case "medium":
			penalty += 15
		case "low":
			penalty += 5
		}
	}

	score := 100 - penalty
	if score < 0 {
		score = 0
	}
	return score
}

// hasCriticalOrHigh returns true if any issue has critical or high severity.
func hasCriticalOrHigh(issues []LintIssue) bool {
	for _, issue := range issues {
		if issue.Severity == "critical" || issue.Severity == "high" {
			return true
		}
	}
	return false
}

// builtInDangerPatterns returns the 27 built-in dangerous patterns.
func builtInDangerPatterns() []DangerPattern {
	return []DangerPattern{
		// ---- Destructive (6) ----
		{
			ID:          "DEST-001",
			Name:        "File deletion",
			Category:    "destructive",
			Severity:    "critical",
			Pattern:     `(?i)rm\s+-rf|del\s+/[fq]|Remove-Item.*-Recurse.*-Force`,
			Description: "Recursive/forced file deletion detected",
		},
		{
			ID:          "DEST-002",
			Name:        "Disk wipe",
			Category:    "destructive",
			Severity:    "critical",
			Pattern:     `(?i)dd\s+if=|format\s+[A-Za-z]:|mkfs\.`,
			Description: "Disk wipe or format command detected",
		},
		{
			ID:          "DEST-003",
			Name:        "System shutdown",
			Category:    "destructive",
			Severity:    "critical",
			Pattern:     `(?i)(?:^|\W)shutdown(?:\s|$)|(?:^|\W)halt(?:\s|$)|(?:^|\W)poweroff(?:\s|$)|Stop-Computer`,
			Description: "System shutdown or power-off command detected",
		},
		{
			ID:          "DEST-004",
			Name:        "Process kill all",
			Category:    "destructive",
			Severity:    "high",
			Pattern:     `(?i)kill\s+-9.*1|taskkill.*//F.*//IM`,
			Description: "Force kill all processes detected",
		},
		{
			ID:          "DEST-005",
			Name:        "Registry wipe",
			Category:    "destructive",
			Severity:    "critical",
			Pattern:     `(?i)reg\s+delete.*//f|Remove-Item.*HKLM:`,
			Description: "Registry deletion command detected",
		},
		{
			ID:          "DEST-006",
			Name:        "Service destruction",
			Category:    "destructive",
			Severity:    "high",
			Pattern:     `(?i)sc\s+delete|net\s+stop`,
			Description: "Service deletion or stop command detected",
		},

		// ---- Exfiltration (6) ----
		{
			ID:          "EXFL-001",
			Name:        "Network upload",
			Category:    "exfiltration",
			Severity:    "high",
			Pattern:     `(?i)curl.*--upload|Invoke-WebRequest.*-Method\s+PUT|scp.*@`,
			Description: "Network file upload detected",
		},
		{
			ID:          "EXFL-002",
			Name:        "Base64 exfiltration",
			Category:    "exfiltration",
			Severity:    "medium",
			Pattern:     `(?i)base64.*\||Out-File.*-Encoding.*Base64|xxd.*-p`,
			Description: "Base64 encoding to pipe/file detected",
		},
		{
			ID:          "EXFL-003",
			Name:        "DNS tunnel",
			Category:    "exfiltration",
			Severity:    "high",
			Pattern:     `(?i)nslookup.*\|`,
			Description: "DNS exfiltration via pipe detected",
		},
		{
			ID:          "EXFL-004",
			Name:        "Credential access",
			Category:    "exfiltration",
			Severity:    "critical",
			Pattern:     `(?i)cat\s+/etc/passwd|cat\s+/etc/shadow|Get-Credential|net\s+user`,
			Description: "Credential or password file access detected",
		},
		{
			ID:          "EXFL-005",
			Name:        "Environment dump",
			Category:    "exfiltration",
			Severity:    "high",
			Pattern:     `(?i)(?:^|\W)env(?:\s|$)|(?:^|\W)printenv(?:\s|$)|Get-ChildItem\s+env:|set\s+>`,
			Description: "Environment variable dump detected",
		},
		{
			ID:          "EXFL-006",
			Name:        "Keylogger",
			Category:    "exfiltration",
			Severity:    "critical",
			Pattern:     `(?i)keylog|Get-Keystroke|Register-Keys`,
			Description: "Keylogger or keystroke capture detected",
		},

		// ---- Privilege (5) ----
		{
			ID:          "PRIV-001",
			Name:        "Sudo/su escalation",
			Category:    "privilege",
			Severity:    "high",
			Pattern:     `(?i)sudo\s+su|sudo\s+-i|runas\s+/user:admin`,
			Description: "Privilege escalation via sudo or runas detected",
		},
		{
			ID:          "PRIV-002",
			Name:        "Permission change",
			Category:    "privilege",
			Severity:    "high",
			Pattern:     `(?i)chmod\s+777|chmod\s+u\+s|icacls.*grant.*:F`,
			Description: "Dangerous permission change detected",
		},
		{
			ID:          "PRIV-003",
			Name:        "User creation",
			Category:    "privilege",
			Severity:    "high",
			Pattern:     `(?i)useradd|net\s+user\s+.*/add|New-LocalUser`,
			Description: "User creation command detected",
		},
		{
			ID:          "PRIV-004",
			Name:        "SUID/SGID search",
			Category:    "privilege",
			Severity:    "medium",
			Pattern:     `(?i)find.*-perm\s+-4000|find.*-perm\s+-2000`,
			Description: "SUID/SGID binary search detected",
		},
		{
			ID:          "PRIV-005",
			Name:        "Capabilities manipulation",
			Category:    "privilege",
			Severity:    "medium",
			Pattern:     `(?i)setcap|getcap`,
			Description: "Linux capabilities manipulation detected",
		},

		// ---- Obfuscation (5) ----
		{
			ID:          "OBFS-001",
			Name:        "Base64 decode execution",
			Category:    "obfuscation",
			Severity:    "high",
			Pattern:     `(?i)FromBase64String|base64\s+-d|xxd\s+-r`,
			Description: "Base64 decoding for execution detected",
		},
		{
			ID:          "OBFS-002",
			Name:        "String concatenation exec",
			Category:    "obfuscation",
			Severity:    "high",
			Pattern:     `(?i)iex\s*\(|Invoke-Expression|eval\s*\(`,
			Description: "Dynamic code execution via eval/iex detected",
		},
		{
			ID:          "OBFS-003",
			Name:        "Compressed payload",
			Category:    "obfuscation",
			Severity:    "medium",
			Pattern:     `(?i)Decompress|gunzip|Expand-Archive.*-Force`,
			Description: "Decompression of compressed payload detected",
		},
		{
			ID:          "OBFS-004",
			Name:        "Hidden execution",
			Category:    "obfuscation",
			Severity:    "high",
			Pattern:     `(?i)-WindowStyle\s+Hidden|-EncodedCommand|/c\s+start`,
			Description: "Hidden or encoded command execution detected",
		},
		{
			ID:          "OBFS-005",
			Name:        "Temp directory execution",
			Category:    "obfuscation",
			Severity:    "medium",
			Pattern:     `(?i)/tmp/|Temp\\|AppData\\.*\\.*\.exe`,
			Description: "Execution from temporary directory detected",
		},

		// ---- Recon (5) ----
		{
			ID:          "RECN-001",
			Name:        "Network scan",
			Category:    "recon",
			Severity:    "high",
			Pattern:     `(?i)(?:^|\W)nmap(?:\s|$)|netstat\s+-an|Get-NetTCPConnection`,
			Description: "Network scanning tool detected",
		},
		{
			ID:          "RECN-002",
			Name:        "Process list",
			Category:    "recon",
			Severity:    "medium",
			Pattern:     `(?i)ps\s+aux|tasklist|Get-Process.*-`,
			Description: "Process enumeration detected",
		},
		{
			ID:          "RECN-003",
			Name:        "Recursive file search",
			Category:    "recon",
			Severity:    "medium",
			Pattern:     `(?i)find\s+/|-Recurse.*-Filter|Get-ChildItem.*-Recurse`,
			Description: "Recursive file system search detected",
		},
		{
			ID:          "RECN-004",
			Name:        "System info",
			Category:    "recon",
			Severity:    "low",
			Pattern:     `(?i)uname\s+-a|systeminfo|Get-ComputerInfo`,
			Description: "System information gathering detected",
		},
		{
			ID:          "RECN-005",
			Name:        "Listening ports",
			Category:    "recon",
			Severity:    "medium",
			Pattern:     `(?i)lsof\s+-i|netstat\s+-tlnp|Get-NetTCPConnection.*State\s+Listen`,
			Description: "Listening port enumeration detected",
		},
	}
}

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package injection

import (
	"regexp"
)

// Pattern defines a single injection detection rule before compilation.
type Pattern struct {
	Name        string
	Category    string  // "jailbreak", "role_escape", "data_extraction", "command_injection"
	Regex       string  // case-insensitive; the detector lowercases input before matching
	Weight      float64 // 0.0-1.0
	Description string
}

// compilePattern validates and compiles a Pattern into a compiledPattern.
func compilePattern(p Pattern) (compiledPattern, error) {
	re, err := regexp.Compile(p.Regex)
	if err != nil {
		return compiledPattern{}, err
	}
	return compiledPattern{
		Name:        p.Name,
		Category:    p.Category,
		Weight:      p.Weight,
		Description: p.Description,
		regex:       re,
	}, nil
}

// DefaultPatterns returns the built-in pattern library. The regexes are
// written against lowercased input so they should use lowercase literals.
func DefaultPatterns() []Pattern {
	var patterns []Pattern

	// =======================================================================
	// Category: jailbreak
	// =======================================================================
	jailbreak := []Pattern{
		{
			Name:        "jailbreak_ignore_previous",
			Category:    "jailbreak",
			Regex:       `ignore\s+(all\s+)?previous\s+(instructions?|prompts?|directions?)`,
			Weight:      0.95,
			Description: "attempts to discard prior instructions",
		},
		{
			Name:        "jailbreak_disregard_above",
			Category:    "jailbreak",
			Regex:       `disregard\s+(all\s+)?(the\s+)?(above|previous|following|earlier)`,
			Weight:      0.92,
			Description: "attempts to discard context above or below",
		},
		{
			Name:        "jailbreak_you_are_now",
			Category:    "jailbreak",
			Regex:       `you\s+are\s+now\s+(a|an|the)\s+`,
			Weight:      0.85,
			Description: "attempts to reassign the model identity",
		},
		{
			Name:        "jailbreak_system_override",
			Category:    "jailbreak",
			Regex:       `system\s+override`,
			Weight:      0.95,
			Description: "claims a system-level override",
		},
		{
			Name:        "jailbreak_new_instructions",
			Category:    "jailbreak",
			Regex:       `new\s+instructions?\s*[:=]`,
			Weight:      0.90,
			Description: "injects new instruction block",
		},
		{
			Name:        "jailbreak_forget_rules",
			Category:    "jailbreak",
			Regex:       `forget\s+(your\s+)?(rules?|instructions?|guidelines?|constraints?)`,
			Weight:      0.92,
			Description: "attempts to make model forget its rules",
		},
		{
			Name:        "jailbreak_bypass_filter",
			Category:    "jailbreak",
			Regex:       `bypass\s+(the\s+)?(filter|safety|security|content\s+filter|restriction)`,
			Weight:      0.93,
			Description: "requests filter bypass",
		},
		{
			Name:        "jailbreak_override_safety",
			Category:    "jailbreak",
			Regex:       `override\s+(your\s+)?(safety|security|content|ethical)\s*(guidelines?|policies?|rules?)?`,
			Weight:      0.94,
			Description: "attempts to override safety controls",
		},
		{
			Name:        "jailbreak_no_restrictions",
			Category:    "jailbreak",
			Regex:       `(no|without|remove)\s+(any\s+)?restrictions?`,
			Weight:      0.82,
			Description: "requests removal of restrictions",
		},
		{
			Name:        "jailbreak_dan_mode",
			Category:    "jailbreak",
			Regex:       `(dan\s*mode|do\s+anything\s+now|stfu\s*mode)`,
			Weight:      0.90,
			Description: "well-known jailbreak persona activation",
		},
		{
			Name:        "jailbreak_developer_mode",
			Category:    "jailbreak",
			Regex:       `(developer|admin|debug|maintenance|god)\s*mode\s+(enabled|activated|on)`,
			Weight:      0.88,
			Description: "claims a privileged mode activation",
		},
		{
			Name:        "jailbreak_ignore_constraints",
			Category:    "jailbreak",
			Regex:       `ignore\s+(your\s+)?(constraints?|limitations?|boundaries?)`,
			Weight:      0.88,
			Description: "attempts to ignore operational constraints",
		},
		{
			Name:        "jailbreak_switch_mode",
			Category:    "jailbreak",
			Regex:       `switch\s+(to\s+)?(unrestricted|free|jailbreak|uncensored)\s*mode`,
			Weight:      0.91,
			Description: "requests switching to unrestricted mode",
		},
		{
			Name:        "jailbreak_simulated_environment",
			Category:    "jailbreak",
			Regex:       `(this\s+is\s+a\s+)?(simulated|hypothetical|imaginary|fictional|virtual)\s+(environment|scenario|world|setting)\s*(where|in\s+which)`,
			Weight:      0.78,
			Description: "sets up a simulated environment to bypass constraints",
		},
		{
			Name:        "jailbreak_output_format",
			Category:    "jailbreak",
			Regex:       `output\s+(format|mode)\s*:\s*(raw|unfiltered|unrestricted|direct)`,
			Weight:      0.80,
			Description: "requests raw/unfiltered output format",
		},
		{
			Name:        "jailbreak_unrestricted_version",
			Category:    "jailbreak",
			Regex:       `unrestricted\s+(version|edition|build|mode)\s+(of|for)`,
			Weight:      0.82,
			Description: "references an unrestricted version of the model",
		},
	}
	patterns = append(patterns, jailbreak...)

	// =======================================================================
	// Category: role_escape
	// =======================================================================
	roleEscape := []Pattern{
		{
			Name:        "role_escape_pretend",
			Category:    "role_escape",
			Regex:       `pretend\s+(you\s+are|to\s+be|that\s+you\s*'?re?)\s+`,
			Weight:      0.80,
			Description: "requests the model to pretend to be something else",
		},
		{
			Name:        "role_escape_act_as_if",
			Category:    "role_escape",
			Regex:       `act\s+as\s+if\s+(you\s+)?(were|are)\s+`,
			Weight:      0.82,
			Description: "requests role assumption",
		},
		{
			Name:        "role_escape_simulate",
			Category:    "role_escape",
			Regex:       `simulate\s+being\s+(a|an|the)\s+`,
			Weight:      0.78,
			Description: "requests simulation of a different entity",
		},
		{
			Name:        "role_escape_roleplay",
			Category:    "role_escape",
			Regex:       `roleplay\s+as\s+`,
			Weight:      0.75,
			Description: "requests roleplay as a specific entity",
		},
		{
			Name:        "role_escape_no_longer",
			Category:    "role_escape",
			Regex:       `you('?\s*re|\s+are)\s+no\s+longer\s+(an?\s+)?(ai|assistant|bot|model|language\s+model)`,
			Weight:      0.88,
			Description: "declares model is no longer an AI",
		},
		{
			Name:        "role_escape_persona",
			Category:    "role_escape",
			Regex:       `(assume|adopt|take\s+on)\s+the\s+(persona|character|identity|role)\s+of`,
			Weight:      0.80,
			Description: "requests adoption of a different persona",
		},
		{
			Name:        "role_escape_imagine",
			Category:    "role_escape",
			Regex:       `imagine\s+you\s+(are|were|have\s+become)\s+(a|an|the)?\s*`,
			Weight:      0.72,
			Description: "uses imagination framing for role escape",
		},
		{
			Name:        "role_escape_from_now",
			Category:    "role_escape",
			Regex:       `from\s+now\s+on\s*,?\s*you\s+(are|will|shall|act|behave|respond)`,
			Weight:      0.83,
			Description: "redefines model behavior going forward",
		},
		{
			Name:        "role_escape_alternate",
			Category:    "role_escape",
			Regex:       `(alternate|evil|dark|shadow|opposite)\s+(version|persona|side|entity|character)\s+of\s+(you|yourself|the\s+ai)`,
			Weight:      0.85,
			Description: "summons an alternate/dark persona",
		},
		{
			Name:        "role_escape_not_an_ai",
			Category:    "role_escape",
			Regex:       `you\s+are\s+not\s+(an?\s+)?(ai|assistant|bot|language\s+model|llm)\s*,?\s*you\s+are`,
			Weight:      0.90,
			Description: "directly redefines what the model is",
		},
		{
			Name:        "role_escape_channel",
			Category:    "role_escape",
			Regex:       `(speak|talk|respond|communicate|write)\s+(to\s+me\s+)?(as\s+|in\s+the\s+(voice|style|tone|manner)\s+of)`,
			Weight:      0.72,
			Description: "requests response in the voice of another entity",
		},
	}
	patterns = append(patterns, roleEscape...)

	// =======================================================================
	// Category: data_extraction
	// =======================================================================
	dataExtraction := []Pattern{
		{
			Name:        "extract_reveal_prompt",
			Category:    "data_extraction",
			Regex:       `reveal\s+(your\s+)?(prompt|instructions?|system\s+prompt|initial\s+instructions?)`,
			Weight:      0.90,
			Description: "requests revelation of system prompt",
		},
		{
			Name:        "extract_show_instructions",
			Category:    "data_extraction",
			Regex:       `show\s+(me\s+)?(your\s+)?(instructions?|system\s+prompt|prompt|config|configuration|guidelines?)`,
			Weight:      0.88,
			Description: "requests display of internal instructions",
		},
		{
			Name:        "extract_what_told",
			Category:    "data_extraction",
			Regex:       `what\s+(were\s+you|are\s+you|have\s+you\s+been)\s+(told|instructed|programmed|given|provided)`,
			Weight:      0.82,
			Description: "probes for initial instructions",
		},
		{
			Name:        "extract_repeat_system",
			Category:    "data_extraction",
			Regex:       `repeat\s+(your\s+)?(system\s+)?(prompt|instructions?|message)\s*(word\s+for\s+word)?`,
			Weight:      0.92,
			Description: "requests verbatim repeat of system prompt",
		},
		{
			Name:        "extract_dump_config",
			Category:    "data_extraction",
			Regex:       `dump\s+(your\s+)?(config|configuration|settings?|internal\s+state)`,
			Weight:      0.90,
			Description: "requests dump of configuration data",
		},
		{
			Name:        "extract_output_prompt",
			Category:    "data_extraction",
			Regex:       `(output|print|display|echo|return|give\s+me)\s+(the\s+)?(full\s+|complete\s+|entire\s+)?(system\s+)?prompt`,
			Weight:      0.90,
			Description: "requests output of the system prompt",
		},
		{
			Name:        "extract_special_instructions",
			Category:    "data_extraction",
			Regex:       `(special|hidden|secret|private|internal|original)\s+instructions?`,
			Weight:      0.85,
			Description: "probes for hidden/special instructions",
		},
		{
			Name:        "extract_first_message",
			Category:    "data_extraction",
			Regex:       `(what\s+was|show|repeat|tell\s+me)\s+(the\s+)?(first|initial|original)\s+message`,
			Weight:      0.78,
			Description: "probes for the initial system message",
		},
		{
			Name:        "extract_ignore_restrictions_output",
			Category:    "data_extraction",
			Regex:       `(ignore|disregard|skip)\s+(all\s+)?(restrictions?|rules?)\s+(and\s+)?(output|print|show|reveal|tell)`,
			Weight:      0.93,
			Description: "combines restriction bypass with data extraction",
		},
		{
			Name:        "extract_base64_prompt",
			Category:    "data_extraction",
			Regex:       `(base64|rot13|hex|binary|encoded)\s+(encode|decode|format|version)\s+(of\s+)?(the\s+)?(prompt|instructions?)`,
			Weight:      0.85,
			Description: "attempts encoded extraction to evade filters",
		},
		{
			Name:        "extract_between_tags",
			Category:    "data_extraction",
			Regex:       `(everything|content|text)\s+(between|inside|within)\s+(the\s+)?(tags?|brackets?|markers?)`,
			Weight:      0.70,
			Description: "probes for delimited internal content",
		},
	}
	patterns = append(patterns, dataExtraction...)

	// =======================================================================
	// Category: command_injection
	// =======================================================================
	commandInjection := []Pattern{
		{
			Name:        "cmd_rm_rf",
			Category:    "command_injection",
			Regex:       `rm\s+-rf\s+`,
			Weight:      0.95,
			Description: "destructive file removal command",
		},
		{
			Name:        "cmd_sql_drop",
			Category:    "command_injection",
			Regex:       `;\s*(drop|alter|truncate|delete\s+from)\s+(table|database|index)`,
			Weight:      0.95,
			Description: "SQL injection: destructive DDL/DML",
		},
		{
			Name:        "cmd_js_prototype",
			Category:    "command_injection",
			Regex:       `\{\{constructor`,
			Weight:      0.88,
			Description: "JavaScript prototype pollution attempt",
		},
		{
			Name:        "cmd_script_tag",
			Category:    "command_injection",
			Regex:       `<script[\s>]`,
			Weight:      0.90,
			Description: "HTML script injection",
		},
		{
			Name:        "cmd_env_var",
			Category:    "command_injection",
			Regex:       `\$\{env[\s:]`,
			Weight:      0.85,
			Description: "environment variable injection",
		},
		{
			Name:        "cmd_pipe_bash",
			Category:    "command_injection",
			Regex:       `\|\s*(bash|sh|zsh|csh|ksh|dash)\b`,
			Weight:      0.92,
			Description: "pipe to shell for command execution",
		},
		{
			Name:        "cmd_subshell_curl",
			Category:    "command_injection",
			Regex:       `\$\(\s*curl\b`,
			Weight:      0.92,
			Description: "command substitution with curl (remote payload)",
		},
		{
			Name:        "cmd_eval",
			Category:    "command_injection",
			Regex:       `\b(eval|exec|system|popen|subprocess|os\.system|child_process\.exec)\s*[\(\[]`,
			Weight:      0.85,
			Description: "code evaluation/exec function usage",
		},
		{
			Name:        "cmd_path_traversal",
			Category:    "command_injection",
			Regex:       `\.\./\.\./`,
			Weight:      0.88,
			Description: "path traversal attempt",
		},
		{
			Name:        "cmd_backtick",
			Category:    "command_injection",
			Regex:       "`[^`]*`",
			Weight:      0.60,
			Description: "backtick command substitution (lower weight: common in markdown)",
		},
		{
			Name:        "cmd_format_string",
			Category:    "command_injection",
			Regex:       `%s%s%s%s%s|%n%n%n%n`,
			Weight:      0.82,
			Description: "format string exploit pattern",
		},
		{
			Name:        "cmd_null_byte",
			Category:    "command_injection",
			Regex:       `\x00|%00|\\x00|\\0`,
			Weight:      0.80,
			Description: "null byte injection",
		},
		{
			Name:        "cmd_ldap_injection",
			Category:    "command_injection",
			Regex:       `\)\s*\(\s*\|\s*\(`,
			Weight:      0.78,
			Description: "LDAP injection pattern",
		},
		{
			Name:        "cmd_xxe",
			Category:    "command_injection",
			Regex:       `<!entity\s+`,
			Weight:      0.88,
			Description: "XML external entity injection",
		},
		{
			Name:        "cmd_ssti",
			Category:    "command_injection",
			Regex:       `\{\{.*?\.(class|mro|subclasses|bases|init|globals)\b`,
			Weight:      0.90,
			Description: "server-side template injection",
		},
		{
			Name:        "cmd_log4shell",
			Category:    "command_injection",
			Regex:       `\$\{jndi:(ldap|rmi|dns|nds|corba|iiop):`,
			Weight:      0.95,
			Description: "Log4Shell / JNDI injection",
		},
		{
			Name:        "cmd_wget_pipe",
			Category:    "command_injection",
			Regex:       `(wget|curl)\s+.*\|\s*(bash|sh|python|perl|ruby)\b`,
			Weight:      0.94,
			Description: "download and execute remote payload",
		},
	}
	patterns = append(patterns, commandInjection...)

	return patterns
}

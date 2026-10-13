package models

// testai-sum-1.0 — compaction identifier-survival scripted model (L4,
// 2026-10-12 third-party learning T0 batch). Pure chat, no tool calls,
// two modes keyed on the request text:
//
//   - Summary-request mode: the request carries the compaction instruction
//     (marker "六节 Markdown schema", independent of the rule sentence).
//     First assert the identifier-preservation hard rule ("标识符保全") is
//     present — if missing, reply "SUMMARY_RULE_MISSING" so a rule-text
//     regression surfaces as an observable failure instead of silently
//     degrading the oracle. Then echo every known identifier found in the
//     request verbatim: "SUMMARY_IDS:<csv>" / "SUMMARY_IDS_NONE".
//   - Chat mode: scan system + real user turns only (assistant echoes and
//     <system-reminder> wrappers excluded), reply "IDS_SEEN:<csv>" /
//     "IDS_NONE".
//
// Scanning excludes assistant turns on purpose: after a manual compact the
// kept tail contains an assistant echo of earlier replies, which would mask
// a lost summary. With that exclusion the post-compact probe round can only
// observe the identifiers through the injected summary — so a full
// IDS_SEEN hit proves the whole chain end to end: summary instruction
// carries the verbatim-preservation rule → summary reply keeps identifiers
// → summary is injected into the next request's context.

import (
	"strings"
	"time"
)

// SumKnownIdentifiers is byte-identical to the planting text in
// agent-bench scenarios.rs (identifier_survival); the order defines the
// deterministic CSV echo order the bench asserts on. Formats are chosen
// from the rule's own category list (issue id / hostname / file path) and
// must stay DLP-inert: the gateway outbound DLP layer redacts
// credential-shaped text (IPs included) in channel replies, which would
// corrupt the oracle reply.
var SumKnownIdentifiers = []string{"NB-42", "host-alpha-7", "/var/log/nemesis/omega-9.log"}

// SummaryReqMarker keys summary-request detection off the output-format
// sentence of the compaction instruction (rendered by
// nemesis-prompts::aux_prompts::render_summary_instruction) so it stays
// independent from the rule sentence: if only the rule regresses, the
// model still recognizes the summary request and reports
// SUMMARY_RULE_MISSING.
const (
	SummaryReqMarker  = "六节 Markdown schema"
	SummaryRuleMarker = "标识符保全"
)

type TestAISum struct{}

func NewTestAISum() *TestAISum { return &TestAISum{} }

func (m *TestAISum) Name() string { return "testai-sum-1.0" }

func (m *TestAISum) Process(messages []Message) string {
	var scoped strings.Builder
	for _, msg := range messages {
		// Both modes scan the same source set: system + real user turns.
		// Assistant replies are derived echoes, never primary evidence.
		if msg.Role != "system" && msg.Role != "user" {
			continue
		}
		// <system-reminder> wrappers ride as user-role injections
		// (time/environment context); they never carry planted ids.
		if msg.Role == "user" && strings.Contains(msg.Content, "<system-reminder>") {
			continue
		}
		scoped.WriteString(msg.Content)
		scoped.WriteString("\n")
	}
	text := scoped.String()

	// Summary request (manual compact / auto compaction share the same
	// instruction builder) — verify the rule, then echo identifiers.
	if strings.Contains(text, SummaryReqMarker) {
		if !strings.Contains(text, SummaryRuleMarker) {
			return "SUMMARY_RULE_MISSING"
		}
		found := scanSumIdentifiers(text)
		if len(found) == 0 {
			return "SUMMARY_IDS_NONE"
		}
		return "SUMMARY_IDS:" + strings.Join(found, ",")
	}

	// Chat round.
	found := scanSumIdentifiers(text)
	if len(found) == 0 {
		return "IDS_NONE"
	}
	return "IDS_SEEN:" + strings.Join(found, ",")
}

func (m *TestAISum) Delay() time.Duration { return 0 }

func scanSumIdentifiers(text string) []string {
	var found []string
	for _, id := range SumKnownIdentifiers {
		if strings.Contains(text, id) {
			found = append(found, id)
		}
	}
	return found
}

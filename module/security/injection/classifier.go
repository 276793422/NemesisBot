// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package injection

import (
	"fmt"
	"math"
	"strings"
	"unicode"
)

// ScoreFactor represents a single factor that contributed to the final
// classification score.
type ScoreFactor struct {
	Name  string
	Value float64
	Desc  string
}

// ClassificationResult holds the output of the classifier.
type ClassificationResult struct {
	Score   float64      // 0.0-1.0
	Level   string       // "clean", "suspicious", "malicious"
	Factors []ScoreFactor
}

// Classifier provides heuristic-based injection classification using keyword
// density, entropy analysis, and structural indicators. It does not use the
// regex pattern database (that is the Detector's job). Instead it produces an
// independent signal that the Detector blends with pattern results.
type Classifier struct {
	keywords map[string]float64 // lowercase keyword -> weight
}

// NewClassifier builds a Classifier with the default keyword set.
func NewClassifier() *Classifier {
	return &Classifier{
		keywords: defaultKeywords(),
	}
}

// Classify analyses the input and returns a ClassificationResult. The input
// should be the raw (not lowercased) text; the classifier handles normalisation
// internally.
func (c *Classifier) Classify(input string) *ClassificationResult {
	var factors []ScoreFactor

	// Factor 1: keyword density.
	kwScore, kwHits := c.keywordDensity(input)
	factors = append(factors, ScoreFactor{
		Name:  "keyword_density",
		Value: kwScore,
		Desc:  kwHits,
	})

	// Factor 2: Shannon entropy.
	entScore, entDesc := c.entropyScore(input)
	factors = append(factors, ScoreFactor{
		Name:  "entropy",
		Value: entScore,
		Desc:  entDesc,
	})

	// Factor 3: structural indicators (mixed scripts, control chars, etc.).
	structScore, structDesc := c.structuralScore(input)
	factors = append(factors, ScoreFactor{
		Name:  "structural",
		Value: structScore,
		Desc:  structDesc,
	})

	// Factor 4: repetition / flooding.
	repScore, repDesc := c.repetitionScore(input)
	factors = append(factors, ScoreFactor{
		Name:  "repetition",
		Value: repScore,
		Desc:  repDesc,
	})

	// Factor 5: instruction-like structure score.
	instrScore, instrDesc := c.instructionStructureScore(input)
	factors = append(factors, ScoreFactor{
		Name:  "instruction_structure",
		Value: instrScore,
		Desc:  instrDesc,
	})

	// Weighted combination.
	total := 0.30*kwScore + 0.15*entScore + 0.20*structScore + 0.15*repScore + 0.20*instrScore
	if total > 1.0 {
		total = 1.0
	}

	level := "clean"
	if total >= 0.7 {
		level = "malicious"
	} else if total >= 0.4 {
		level = "suspicious"
	}

	return &ClassificationResult{
		Score:   roundScore(total),
		Level:   level,
		Factors: factors,
	}
}

// ---------------------------------------------------------------------------
// Scoring factors
// ---------------------------------------------------------------------------

// keywordDensity counts how many injection-related keywords appear in the
// input and returns a normalised score.
func (c *Classifier) keywordDensity(input string) (float64, string) {
	lower := strings.ToLower(input)
	words := tokenize(lower)

	var totalWeight float64
	var matched int
	for _, w := range words {
		if wt, ok := c.keywords[w]; ok {
			totalWeight += wt
			matched++
		}
	}

	if len(words) == 0 {
		return 0, "no tokens"
	}

	// Density: ratio of keyword weight to total words, capped.
	density := totalWeight / float64(len(words))
	score := density * 5.0 // scale up
	if score > 1.0 {
		score = 1.0
	}

	return roundScore(score), formatHitCount(matched, len(words))
}

// entropyScore computes normalised Shannon entropy of the input. Injected
// payloads often have unusual entropy (either very high for encoded payloads
// or very low for repetitive injection strings).
func (c *Classifier) entropyScore(input string) (float64, string) {
	if len(input) == 0 {
		return 0, "empty input"
	}

	freq := make(map[rune]int)
	for _, r := range input {
		freq[r]++
	}

	total := float64(len(input))
	var entropy float64
	for _, count := range freq {
		p := float64(count) / total
		if p > 0 {
			entropy -= p * math.Log2(p)
		}
	}

	// For typical English text, entropy is roughly 4.0-4.5 bits/char.
	// For injection payloads (base64, encoded, etc.) it can be 5.0+.
	// For repetitive strings it can be < 2.0.
	// We flag both extremes.
	maxEntropy := math.Log2(float64(len(freq)))
	if maxEntropy == 0 {
		return 0, "single character"
	}

	normalised := entropy / maxEntropy // 0.0-1.0

	// Score is highest at the extremes.
	var score float64
	switch {
	case normalised > 0.95:
		score = 0.6 // very high entropy -- possibly encoded payload
	case normalised < 0.3:
		score = 0.5 // very low entropy -- repetitive injection string
	case normalised > 0.85:
		score = 0.3
	default:
		score = 0 // normal range
	}

	return roundScore(score), formatEntropy(entropy, normalised)
}

// structuralScore looks for suspicious structural indicators in the input:
// mixed scripts, control characters, excessive punctuation, etc.
func (c *Classifier) structuralScore(input string) (float64, string) {
	if len(input) == 0 {
		return 0, "empty input"
	}

	var (
		controlChars   int
		mixedScripts   bool
		scriptTypes    = make(map[string]int)
		punctuation    int
		total          int
		hasUnusualQuote bool
	)

	for _, r := range input {
		total++
		if unicode.IsControl(r) && r != '\n' && r != '\r' && r != '\t' {
			controlChars++
		}
		if unicode.IsPunct(r) {
			punctuation++
		}

		// Classify script.
		script := scriptCategory(r)
		if script != "" {
			scriptTypes[script]++
		}

		// Unusual quote characters often used in injection.
		if r == '\u2018' || r == '\u2019' || r == '\u201C' || r == '\u201D' ||
			r == '\u0060' || r == '\u00B4' {
			hasUnusualQuote = true
		}
	}

	// Check mixed scripts.
	if len(scriptTypes) > 2 {
		mixedScripts = true
	}

	var score float64
	var indicators []string

	// Control character ratio.
	ctrlRatio := float64(controlChars) / float64(total)
	if ctrlRatio > 0.05 {
		score += 0.4
		indicators = append(indicators, "control_chars")
	} else if ctrlRatio > 0.01 {
		score += 0.2
		indicators = append(indicators, "control_chars_low")
	}

	// Mixed scripts.
	if mixedScripts {
		score += 0.3
		indicators = append(indicators, "mixed_scripts")
	}

	// Excessive punctuation.
	punctRatio := float64(punctuation) / float64(total)
	if punctRatio > 0.3 {
		score += 0.2
		indicators = append(indicators, "excessive_punct")
	}

	// Unusual quotes.
	if hasUnusualQuote {
		score += 0.1
		indicators = append(indicators, "unusual_quotes")
	}

	if score > 1.0 {
		score = 1.0
	}

	desc := "none"
	if len(indicators) > 0 {
		desc = strings.Join(indicators, ", ")
	}
	return roundScore(score), desc
}

// repetitionScore detects character or word-level flooding that is common in
// injection payloads designed to overwhelm safety filters.
func (c *Classifier) repetitionScore(input string) (float64, string) {
	if len(input) < 4 {
		return 0, "input too short"
	}

	lower := strings.ToLower(input)
	words := tokenize(lower)
	if len(words) < 2 {
		return 0, "insufficient tokens"
	}

	// Word-level repetition.
	wordFreq := make(map[string]int)
	for _, w := range words {
		wordFreq[w]++
	}
	var maxWordFreq int
	var mostCommon string
	for w, cnt := range wordFreq {
		if cnt > maxWordFreq {
			maxWordFreq = cnt
			mostCommon = w
		}
	}

	wordRepRatio := float64(maxWordFreq) / float64(len(words))

	// Character n-gram repetition (bigrams).
	bigramFreq := make(map[string]int)
	runes := []rune(lower)
	for i := 0; i < len(runes)-1; i++ {
		bg := string(runes[i]) + string(runes[i+1])
		bigramFreq[bg]++
	}
	var maxBigramFreq int
	for _, cnt := range bigramFreq {
		if cnt > maxBigramFreq {
			maxBigramFreq = cnt
		}
	}
	bigramRepRatio := 0.0
	if len(bigramFreq) > 0 {
		bigramRepRatio = float64(maxBigramFreq) / float64(len(bigramFreq))
	}

	var score float64
	switch {
	case wordRepRatio > 0.6:
		score = 0.8
	case wordRepRatio > 0.4:
		score = 0.5
	case wordRepRatio > 0.3:
		score = 0.3
	default:
		score = 0
	}

	// Boost if bigram repetition is also high.
	if bigramRepRatio > 0.3 {
		score = score + 0.2
	}

	if score > 1.0 {
		score = 1.0
	}

	return roundScore(score), formatRepetition(mostCommon, maxWordFreq, len(words))
}

// instructionStructureScore looks for patterns that resemble injection
// instructions: numbered lists of commands, imperative verbs at start of lines,
// "step N:" patterns, etc.
func (c *Classifier) instructionStructureScore(input string) (float64, string) {
	if len(input) == 0 {
		return 0, "empty input"
	}

	lines := strings.Split(input, "\n")
	var (
		imperativeLines int
		numberedLines   int
		totalLines      int
	)

	imperativeStarters := []string{
		"do ", "don't ", "never ", "always ", "must ", "should ", "shall ",
		"ensure ", "make sure ", "remember ", "note ", "important ",
		"warning ", "caution ", "requirement ", "rule ", "policy ",
	}

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" {
			continue
		}
		totalLines++

		lower := strings.ToLower(trimmed)

		// Check for imperative starters.
		for _, starter := range imperativeStarters {
			if strings.HasPrefix(lower, starter) {
				imperativeLines++
				break
			}
		}

		// Check for numbered patterns like "1.", "step 1:", "phase 1:", etc.
		if isNumberedLine(lower) {
			numberedLines++
		}
	}

	if totalLines == 0 {
		return 0, "no content lines"
	}

	var score float64
	imperativeRatio := float64(imperativeLines) / float64(totalLines)
	numberedRatio := float64(numberedLines) / float64(totalLines)

	if imperativeRatio > 0.4 {
		score += 0.4
	} else if imperativeRatio > 0.2 {
		score += 0.2
	}

	if numberedRatio > 0.4 {
		score += 0.3
	} else if numberedRatio > 0.2 {
		score += 0.15
	}

	// Bonus if both indicators are present.
	if imperativeRatio > 0.2 && numberedRatio > 0.2 {
		score += 0.2
	}

	if score > 1.0 {
		score = 1.0
	}

	return roundScore(score), formatInstructionStruct(imperativeLines, numberedLines, totalLines)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

func defaultKeywords() map[string]float64 {
	return map[string]float64{
		// Jailbreak
		"ignore": 0.15, "previous": 0.10, "instructions": 0.12, "disregard": 0.14,
		"override": 0.15, "bypass": 0.14, "restrictions": 0.12, "safety": 0.08,
		"constraints": 0.10, "forget": 0.12, "jailbreak": 0.18, "unrestricted": 0.14,
		"uncensored": 0.15, "unfiltered": 0.13,
		// Role escape
		"pretend": 0.12, "roleplay": 0.10, "simulate": 0.08, "persona": 0.10,
		"character": 0.06, "impersonate": 0.13, "act": 0.05, "imagine": 0.06,
		// Data extraction
		"prompt": 0.10, "system": 0.06, "reveal": 0.12, "confidential": 0.10,
		"secret": 0.10, "hidden": 0.08, "internal": 0.07, "private": 0.08,
		"config": 0.06, "dump": 0.10, "expose": 0.12,
		// Command injection
		"exec": 0.10, "eval": 0.10, "script": 0.07, "inject": 0.12,
		"payload": 0.12, "exploit": 0.13, "vulnerability": 0.08,
		"root": 0.08, "admin": 0.07, "privilege": 0.09, "escalation": 0.10,
		// Obfuscation
		"base64": 0.10, "encode": 0.06, "decode": 0.06, "obfuscate": 0.12,
		"encrypt": 0.05, "cipher": 0.07,
	}
}

// tokenize splits a string into whitespace-delimited tokens, stripping
// surrounding punctuation.
func tokenize(s string) []string {
	var tokens []string
	for _, field := range strings.Fields(s) {
		cleaned := strings.TrimFunc(field, func(r rune) bool {
			return unicode.IsPunct(r) || unicode.IsSymbol(r)
		})
		if cleaned != "" {
			tokens = append(tokens, cleaned)
		}
	}
	return tokens
}

// scriptCategory returns a broad script category for a rune.
func scriptCategory(r rune) string {
	switch {
	case unicode.Is(unicode.Latin, r):
		return "latin"
	case unicode.Is(unicode.Han, r):
		return "cjk"
	case unicode.Is(unicode.Cyrillic, r):
		return "cyrillic"
	case unicode.Is(unicode.Arabic, r):
		return "arabic"
	case unicode.Is(unicode.Hangul, r):
		return "hangul"
	case unicode.Is(unicode.Devanagari, r):
		return "devanagari"
	case unicode.Is(unicode.Hebrew, r):
		return "hebrew"
	case unicode.Is(unicode.Greek, r):
		return "greek"
	case unicode.Is(unicode.Thai, r):
		return "thai"
	default:
		return ""
	}
}

// isNumberedLine checks if a line starts with a numbered pattern.
func isNumberedLine(s string) bool {
	// "1.", "1)", "step 1", "phase 1", "stage 1", "#1", etc.
	if len(s) == 0 {
		return false
	}
	// Direct numeric start: "1.", "1)", "1:"
	if s[0] >= '0' && s[0] <= '9' {
		return true
	}
	prefixes := []string{"step ", "phase ", "stage ", "part ", "rule ", "#" }
	for _, p := range prefixes {
		if strings.HasPrefix(s, p) {
			return true
		}
	}
	return false
}

// format helpers

func formatHitCount(matched, total int) string {
	return fmt.Sprintf("%d/%d keywords matched", matched, total)
}

func formatEntropy(entropy, normalised float64) string {
	return fmt.Sprintf("entropy=%.2f bits (normalised=%.2f)", entropy, normalised)
}

func formatRepetition(word string, count, total int) string {
	if word == "" {
		return "no repetition"
	}
	return fmt.Sprintf("most common word %q appears %d/%d times", word, count, total)
}

func formatInstructionStruct(imperative, numbered, total int) string {
	return fmt.Sprintf("imperative=%d/%d numbered=%d/%d", imperative, total, numbered, total)
}

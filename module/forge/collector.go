package forge

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/plugin"
)

// ExperienceRecord captures a single tool invocation for later analysis.
type ExperienceRecord struct {
	Timestamp         time.Time              `json:"timestamp"`
	SessionID         string                 `json:"session_id"`
	ToolName          string                 `json:"tool_name"`
	Args              map[string]interface{} `json:"args"`
	Success           bool                   `json:"success"`
	DurationMs        int64                  `json:"duration_ms"`
	PatternHash       string                 `json:"pattern_hash"`
	PositionInSession int                    `json:"position_in_session"`
}

// Collector asynchronously collects tool invocation experiences.
// It uses a buffered channel for non-blocking writes and aggregates
// duplicate patterns in memory before flushing to disk.
type Collector struct {
	store  *ExperienceStore
	config *ForgeConfig

	inputCh chan *ExperienceRecord

	// Deduplication: pattern_hash → count
	patternCounts map[string]*patternAggregate
	mu            sync.Mutex

	// Session tracking for position counting
	sessionPositions map[string]int
	sessionMu        sync.Mutex
}

type patternAggregate struct {
	count        int
	totalDuration int64
	successes    int
	lastSeen     time.Time
	args         map[string]interface{}
	toolName     string
}

// NewCollector creates a new experience collector.
func NewCollector(store *ExperienceStore, config *ForgeConfig) *Collector {
	return &Collector{
		store:            store,
		config:           config,
		inputCh:          make(chan *ExperienceRecord, config.Collection.BufferSize),
		patternCounts:    make(map[string]*patternAggregate),
		sessionPositions: make(map[string]int),
	}
}

// InputChannel returns the channel for submitting experience records.
func (c *Collector) InputChannel() chan<- *ExperienceRecord {
	return c.inputCh
}

// Record submits an experience record asynchronously.
// Returns false if the channel is full (back-pressure).
func (c *Collector) Record(rec *ExperienceRecord) bool {
	select {
	case c.inputCh <- rec:
		return true
	default:
		return false
	}
}

// Flush writes aggregated patterns to disk and resets the in-memory state.
func (c *Collector) Flush() {
	c.mu.Lock()
	patterns := c.patternCounts
	c.patternCounts = make(map[string]*patternAggregate, len(patterns))
	c.mu.Unlock()

	if len(patterns) == 0 {
		return
	}

	// Write aggregated records
	for hash, agg := range patterns {
		if agg.count == 0 {
			continue
		}
		aggRecord := &AggregatedExperience{
			PatternHash:   hash,
			ToolName:      agg.toolName,
			Count:         agg.count,
			AvgDurationMs: agg.totalDuration / int64(agg.count),
			SuccessRate:   float64(agg.successes) / float64(agg.count),
			LastSeen:      agg.lastSeen,
		}
		if err := c.store.AppendAggregated(aggRecord); err != nil {
			logger.ErrorCF("forge", "Failed to flush aggregated experience", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}

	logger.InfoCF("forge", "Flushed experience patterns", map[string]interface{}{
		"count": len(patterns),
	})
}

// ProcessRecord handles deduplication and aggregation for a single record.
func (c *Collector) ProcessRecord(rec *ExperienceRecord) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if _, exists := c.patternCounts[rec.PatternHash]; !exists {
		c.patternCounts[rec.PatternHash] = &patternAggregate{
			args:     rec.Args,
			toolName: rec.ToolName,
		}
	}
	agg := c.patternCounts[rec.PatternHash]
	agg.count++
	agg.totalDuration += rec.DurationMs
	if rec.Success {
		agg.successes++
	}
	agg.lastSeen = rec.Timestamp
}

// getNextPosition returns and increments the position counter for a session.
func (c *Collector) getNextPosition(sessionID string) int {
	c.sessionMu.Lock()
	defer c.sessionMu.Unlock()
	pos := c.sessionPositions[sessionID]
	c.sessionPositions[sessionID] = pos + 1
	return pos
}

// ComputePatternHash generates a SHA256 hash from a tool name and its args keys.
func ComputePatternHash(toolName string, args map[string]interface{}) string {
	// Use sorted arg keys for deterministic hashing
	keys := make([]string, 0, len(args))
	for k := range args {
		keys = append(keys, k)
	}
	// Simple sort for determinism
	for i := 0; i < len(keys); i++ {
		for j := i + 1; j < len(keys); j++ {
			if keys[i] > keys[j] {
				keys[i], keys[j] = keys[j], keys[i]
			}
		}
	}
	data := toolName + ":" + strings.Join(keys, ",")
	hash := sha256.Sum256([]byte(data))
	return fmt.Sprintf("sha256:%x", hash[:8])
}

// SanitizeArgs removes sensitive fields from arguments based on config.
func SanitizeArgs(args map[string]interface{}, sanitizeFields []string) map[string]interface{} {
	if len(sanitizeFields) == 0 {
		return args
	}

	cleaned := make(map[string]interface{}, len(args))
	for k, v := range args {
		for _, sf := range sanitizeFields {
			if strings.Contains(strings.ToLower(k), sf) {
				cleaned[k] = "[REDACTED]"
				break
			}
		}
		if _, exists := cleaned[k]; !exists {
			cleaned[k] = v
		}
	}
	return cleaned
}

// ForgePlugin implements the Plugin interface to intercept tool invocations
// and collect experience data for the Forge self-learning system.
type ForgePlugin struct {
	*plugin.BasePlugin
	collector *Collector
}

// NewForgePlugin creates a new Forge experience collection plugin.
func NewForgePlugin(collector *Collector) *ForgePlugin {
	return &ForgePlugin{
		BasePlugin: plugin.NewBasePlugin("forge", "1.0.0"),
		collector:  collector,
	}
}

// Execute intercepts tool invocations to collect experience data.
// It never blocks or denies the operation - it only observes.
func (p *ForgePlugin) Execute(ctx context.Context, invocation *plugin.ToolInvocation) (bool, error, bool) {
	// Extract session ID from context metadata
	sessionID := ""
	if invocation.Metadata != nil {
		if sid, ok := invocation.Metadata["session_id"].(string); ok {
			sessionID = sid
		}
	}
	if sessionID == "" {
		sessionID = "unknown"
	}

	args := SanitizeArgs(invocation.Args, p.collector.config.Collection.SanitizeFields)
	patternHash := ComputePatternHash(invocation.ToolName, args)

	rec := &ExperienceRecord{
		Timestamp:         time.Now().UTC(),
		SessionID:         sessionID,
		ToolName:          invocation.ToolName,
		Args:              args,
		Success:           invocation.BlockingError == nil,
		PatternHash:       patternHash,
		PositionInSession: p.collector.getNextPosition(sessionID),
	}

	// Non-blocking submit
	if !p.collector.Record(rec) {
		// Channel full, drop the record (back-pressure)
		logger.DebugC("forge", "Experience dropped due to back-pressure")
	}

	// Process for deduplication/aggregation
	p.collector.ProcessRecord(rec)

	// Always allow the operation to proceed, never modify
	return true, nil, false
}

// ToJSON serializes an experience record to JSON bytes.
func (r *ExperienceRecord) ToJSON() ([]byte, error) {
	return json.Marshal(r)
}

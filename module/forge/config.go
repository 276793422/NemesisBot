package forge

import (
	"encoding/json"
	"os"
	"time"
)

// ForgeConfig holds all configuration for the Forge self-learning system.
// It is loaded from workspace/forge/forge.json.
type ForgeConfig struct {
	Collection CollectionConfig `json:"collection"`
	Storage    StorageConfig    `json:"storage"`
	Reflection ReflectionConfig `json:"reflection"`
	Artifacts  ArtifactsConfig  `json:"artifacts"`
	Validation ValidationConfig `json:"validation"`
	Trace      TraceConfig      `json:"trace"`
	Learning   LearningConfig   `json:"learning"` // Phase 6: closed-loop learning
}

// CollectionConfig controls experience collection behavior.
type CollectionConfig struct {
	Enabled             bool     `json:"enabled"`
	BufferSize          int      `json:"buffer_size"`
	FlushInterval       Duration `json:"flush_interval"`
	MaxExperiencesPerDay int     `json:"max_experiences_per_day"`
	SanitizeFields      []string `json:"sanitize_fields"`
}

// StorageConfig controls data retention.
type StorageConfig struct {
	MaxExperienceAgeDays int      `json:"max_experience_age_days"`
	MaxReportAgeDays     int      `json:"max_report_age_days"`
	CleanupInterval      Duration `json:"cleanup_interval"`
}

// ReflectionConfig controls the reflection engine.
type ReflectionConfig struct {
	Interval       Duration `json:"interval"`
	MinExperiences int      `json:"min_experiences"`
	UseLLM         bool     `json:"use_llm"`
	LLMBudgetTokens int     `json:"llm_budget_tokens"`
	MaxReportAgeDays int    `json:"max_report_age_days"`
}

// ArtifactsConfig controls artifact generation behavior.
type ArtifactsConfig struct {
	AutoSkill     bool   `json:"auto_skill"`
	MaxSkills     int    `json:"max_skills"`
	MaxScripts    int    `json:"max_scripts"`
	DefaultStatus string `json:"default_status"`
}

// ValidationConfig controls the three-stage validation pipeline.
type ValidationConfig struct {
	AutoValidate    bool     `json:"auto_validate"`
	MinQualityScore int      `json:"min_quality_score"`
	LLMMaxTokens    int      `json:"llm_max_tokens"`
	Timeout         Duration `json:"timeout"`
}

// TraceConfig controls conversation-level trace collection (Phase 5).
type TraceConfig struct {
	Enabled              bool `json:"enabled"`
	MaxTraceAgeDays      int  `json:"max_trace_age_days"`
	MinTracesForAnalysis int  `json:"min_traces_for_analysis"`
}

// LearningConfig controls closed-loop self-learning (Phase 6).
type LearningConfig struct {
	Enabled             bool    `json:"enabled"`
	MinPatternFrequency int     `json:"min_pattern_frequency"`      // minimum occurrences to qualify as pattern (default 5)
	HighConfThreshold   float64 `json:"high_confidence_threshold"`  // auto-generate threshold (default 0.8)
	MaxAutoCreates      int     `json:"max_auto_creates_per_cycle"` // max auto-creates per cycle (default 3)
	MaxRefineRounds     int     `json:"max_refine_rounds"`          // max refine iterations (default 3)
	MinOutcomeSamples   int     `json:"min_outcome_samples"`        // min samples for evaluation (default 5)
	MonitorWindowDays   int     `json:"monitor_window_days"`        // observation window in days (default 7)
	DegradeThreshold    float64 `json:"deprecation_threshold"`      // deprecation threshold (default -0.2)
	DegradeCooldownDays int     `json:"deprecate_cooldown_days"`    // cooldown before re-deprecating (default 7)
	LLMBudgetTokens     int     `json:"llm_budget_tokens"`          // token budget for Skill draft generation (default 2000)
}

// Duration wraps time.Duration for JSON serialization.
type Duration struct {
	time.Duration
}

func (d Duration) MarshalJSON() ([]byte, error) {
	return json.Marshal(d.String())
}

func (d *Duration) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return err
	}
	parsed, err := time.ParseDuration(s)
	if err != nil {
		return err
	}
	d.Duration = parsed
	return nil
}

// DefaultForgeConfig returns sensible defaults.
func DefaultForgeConfig() *ForgeConfig {
	return &ForgeConfig{
		Collection: CollectionConfig{
			Enabled:             true,
			BufferSize:          256,
			FlushInterval:       Duration{30 * time.Second},
			MaxExperiencesPerDay: 500,
			SanitizeFields:      []string{"api_key", "token", "password", "secret", "credential", "key"},
		},
		Storage: StorageConfig{
			MaxExperienceAgeDays: 90,
			MaxReportAgeDays:     30,
			CleanupInterval:      Duration{24 * time.Hour},
		},
		Reflection: ReflectionConfig{
			Interval:        Duration{6 * time.Hour},
			MinExperiences:  10,
			UseLLM:          true,
			LLMBudgetTokens: 4000,
			MaxReportAgeDays: 30,
		},
		Artifacts: ArtifactsConfig{
			AutoSkill:     false,
			MaxSkills:     50,
			MaxScripts:    100,
			DefaultStatus: "draft",
		},
		Validation: ValidationConfig{
			AutoValidate:    true,
			MinQualityScore: 60,
			LLMMaxTokens:    2000,
			Timeout:         Duration{60 * time.Second},
		},
		Trace: TraceConfig{
			Enabled:              true,
			MaxTraceAgeDays:      30,
			MinTracesForAnalysis: 5,
		},
		Learning: LearningConfig{
			Enabled:             false,
			MinPatternFrequency: 5,
			HighConfThreshold:   0.8,
			MaxAutoCreates:      3,
			MaxRefineRounds:     3,
			MinOutcomeSamples:   5,
			MonitorWindowDays:   7,
			DegradeThreshold:    -0.2,
			DegradeCooldownDays: 7,
			LLMBudgetTokens:     2000,
		},
	}
}

// LoadForgeConfig loads forge configuration from the given path.
func LoadForgeConfig(path string) (*ForgeConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	cfg := DefaultForgeConfig()
	if err := json.Unmarshal(data, cfg); err != nil {
		return nil, err
	}

	return cfg, nil
}

// SaveForgeConfig saves forge configuration to the given path.
func SaveForgeConfig(path string, cfg *ForgeConfig) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0644)
}

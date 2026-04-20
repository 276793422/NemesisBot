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

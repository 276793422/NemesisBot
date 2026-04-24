// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package router

// PolicyConfig describes a named routing policy with optional weight configuration.
type PolicyConfig struct {
	Name        string        `json:"name"`
	Description string        `json:"description"`
	Policy      Policy        `json:"policy"`
	Weights     PolicyWeights `json:"weights,omitempty"`
}

// PolicyWeights defines the relative importance of each factor for balanced policies.
// Weights are normalized internally; only the relative ratios matter.
type PolicyWeights struct {
	Cost    float64 `json:"cost"`
	Quality float64 `json:"quality"`
	Latency float64 `json:"latency"`
}

// predefinedPolicies contains the built-in policy configurations.
var predefinedPolicies = map[string]PolicyConfig{
	"fast": {
		Name:        "fast",
		Description: "Optimize for lowest latency. Selects the provider with the fastest response time based on recorded metrics.",
		Policy:      PolicyLatency,
		Weights:     PolicyWeights{Latency: 1.0},
	},
	"balanced": {
		Name:        "balanced",
		Description: "Balance cost, quality, and latency equally. Uses weighted scoring across all three factors.",
		Policy:      PolicyQuality,
		Weights: PolicyWeights{
			Cost:     0.33,
			Quality:  0.34,
			Latency:  0.33,
		},
	},
	"cheap": {
		Name:        "cheap",
		Description: "Optimize for lowest cost. Always selects the cheapest provider per 1K tokens.",
		Policy:      PolicyCost,
		Weights:     PolicyWeights{Cost: 1.0},
	},
	"best": {
		Name:        "best",
		Description: "Optimize for highest quality. Always selects the provider with the highest quality score.",
		Policy:      PolicyQuality,
		Weights:     PolicyWeights{Quality: 1.0},
	},
}

// GetPolicy returns the PolicyConfig for a named preset policy.
// Returns the "balanced" policy if the name is not recognized.
func GetPolicy(name string) PolicyConfig {
	if cfg, ok := predefinedPolicies[name]; ok {
		return cfg
	}
	return predefinedPolicies["balanced"]
}

// AllPolicies returns all predefined policy configurations.
func AllPolicies() map[string]PolicyConfig {
	result := make(map[string]PolicyConfig, len(predefinedPolicies))
	for k, v := range predefinedPolicies {
		result[k] = v
	}
	return result
}

// PolicyNames returns the names of all predefined policies.
func PolicyNames() []string {
	names := make([]string, 0, len(predefinedPolicies))
	for name := range predefinedPolicies {
		names = append(names, name)
	}
	return names
}

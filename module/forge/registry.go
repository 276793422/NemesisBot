package forge

import (
	"encoding/json"
	"os"
	"sync"
	"time"
)

// ArtifactStatus represents the lifecycle state of a Forge artifact.
type ArtifactStatus string

const (
	StatusDraft      ArtifactStatus = "draft"
	StatusTesting    ArtifactStatus = "testing"
	StatusActive     ArtifactStatus = "active"
	StatusDeprecated ArtifactStatus = "deprecated"
)

// ArtifactType represents the type of Forge artifact.
type ArtifactType string

const (
	ArtifactSkill  ArtifactType = "skill"
	ArtifactScript ArtifactType = "script"
	ArtifactMCP    ArtifactType = "mcp"
)

// Artifact represents a self-learning artifact tracked in the registry.
type Artifact struct {
	ID          string         `json:"id"`
	Type        ArtifactType   `json:"type"`
	Name        string         `json:"name"`
	Version     string         `json:"version"`
	Status      ArtifactStatus `json:"status"`
	UsageCount  int            `json:"usage_count"`
	SuccessRate float64        `json:"success_rate"`
	Path        string         `json:"path"`
	CreatedAt   time.Time      `json:"created_at"`
	UpdatedAt   time.Time      `json:"updated_at"`
	Evolution   []Evolution    `json:"evolution,omitempty"`
	Validation  *ArtifactValidation `json:"validation,omitempty"`
}

// Evolution records a version change for an artifact.
type Evolution struct {
	Version string    `json:"version"`
	Date    time.Time `json:"date"`
	Change  string    `json:"change"`
}

// RegistryData is the top-level structure for registry.json.
type RegistryData struct {
	Version   string     `json:"version"`
	Artifacts []Artifact `json:"artifacts"`
}

// Registry manages the artifact registry stored in registry.json.
type Registry struct {
	path string
	mu   sync.RWMutex
	data *RegistryData
}

// NewRegistry creates or loads a registry from the given path.
func NewRegistry(path string) *Registry {
	r := &Registry{
		path: path,
		data: &RegistryData{
			Version:   "1.0",
			Artifacts: []Artifact{},
		},
	}
	r.load()
	return r
}

// load reads the registry from disk.
func (r *Registry) load() {
	data, err := os.ReadFile(r.path)
	if err != nil {
		return
	}
	var rd RegistryData
	if err := json.Unmarshal(data, &rd); err != nil {
		return
	}
	r.data = &rd
}

// save writes the registry to disk.
func (r *Registry) save() error {
	data, err := json.MarshalIndent(r.data, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(r.path, data, 0644)
}

// Add adds a new artifact to the registry.
func (r *Registry) Add(artifact Artifact) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	artifact.CreatedAt = time.Now().UTC()
	artifact.UpdatedAt = time.Now().UTC()
	r.data.Artifacts = append(r.data.Artifacts, artifact)
	return r.save()
}

// Get retrieves an artifact by ID.
func (r *Registry) Get(id string) (Artifact, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	for _, a := range r.data.Artifacts {
		if a.ID == id {
			return a, true
		}
	}
	return Artifact{}, false
}

// Update updates an existing artifact.
func (r *Registry) Update(id string, fn func(*Artifact)) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	for i := range r.data.Artifacts {
		if r.data.Artifacts[i].ID == id {
			fn(&r.data.Artifacts[i])
			r.data.Artifacts[i].UpdatedAt = time.Now().UTC()
			return r.save()
		}
	}
	return nil
}

// List returns all artifacts, optionally filtered by type and/or status.
func (r *Registry) List(artifactType ArtifactType, status ArtifactStatus) []Artifact {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var result []Artifact
	for _, a := range r.data.Artifacts {
		if artifactType != "" && a.Type != artifactType {
			continue
		}
		if status != "" && a.Status != status {
			continue
		}
		result = append(result, a)
	}
	return result
}

// ListAll returns all artifacts.
func (r *Registry) ListAll() []Artifact {
	r.mu.RLock()
	defer r.mu.RUnlock()

	result := make([]Artifact, len(r.data.Artifacts))
	copy(result, r.data.Artifacts)
	return result
}

// Count returns the number of artifacts, optionally filtered by type.
func (r *Registry) Count(artifactType ArtifactType) int {
	r.mu.RLock()
	defer r.mu.RUnlock()

	if artifactType == "" {
		return len(r.data.Artifacts)
	}
	count := 0
	for _, a := range r.data.Artifacts {
		if a.Type == artifactType {
			count++
		}
	}
	return count
}

// Delete removes an artifact from the registry.
func (r *Registry) Delete(id string) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	for i, a := range r.data.Artifacts {
		if a.ID == id {
			r.data.Artifacts = append(r.data.Artifacts[:i], r.data.Artifacts[i+1:]...)
			return r.save()
		}
	}
	return nil
}

package workflow

import (
	"fmt"
	"os"

	"gopkg.in/yaml.v3"
)

// ParseYAML parses a YAML byte slice into a Workflow definition.
func ParseYAML(data []byte) (*Workflow, error) {
	var wf Workflow
	if err := yaml.Unmarshal(data, &wf); err != nil {
		return nil, fmt.Errorf("yaml parse error: %w", err)
	}
	return &wf, nil
}

// ParseFile reads a YAML file and parses it into a Workflow definition.
func ParseFile(path string) (*Workflow, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read file %s: %w", path, err)
	}
	return ParseYAML(data)
}

// Validate checks a Workflow definition for correctness.
// It verifies:
//   - Workflow has a name
//   - At least one node exists
//   - All node IDs are unique
//   - All edge references point to valid nodes
//   - The graph is a valid DAG (no cycles)
func Validate(wf *Workflow) error {
	if wf.Name == "" {
		return fmt.Errorf("workflow must have a name")
	}
	if len(wf.Nodes) == 0 {
		return fmt.Errorf("workflow %q must have at least one node", wf.Name)
	}

	// Check unique node IDs
	nodeIDs := make(map[string]bool, len(wf.Nodes))
	for _, n := range wf.Nodes {
		if n.ID == "" {
			return fmt.Errorf("node missing id in workflow %q", wf.Name)
		}
		if nodeIDs[n.ID] {
			return fmt.Errorf("duplicate node id %q in workflow %q", n.ID, wf.Name)
		}
		nodeIDs[n.ID] = true
	}

	// Validate edges reference existing nodes
	for i, e := range wf.Edges {
		if !nodeIDs[e.From] {
			return fmt.Errorf("edge %d references unknown 'from' node %q", i, e.From)
		}
		if !nodeIDs[e.To] {
			return fmt.Errorf("edge %d references unknown 'to' node %q", i, e.To)
		}
	}

	// Validate DependsOn references
	for _, n := range wf.Nodes {
		for _, dep := range n.DependsOn {
			if !nodeIDs[dep] {
				return fmt.Errorf("node %q depends_on unknown node %q", n.ID, dep)
			}
		}
	}

	// Check for cycles using topological sort
	if _, err := TopologicalSort(wf.Nodes, wf.Edges); err != nil {
		return fmt.Errorf("workflow %q: %w", wf.Name, err)
	}

	// Validate trigger configs
	for i, t := range wf.Triggers {
		switch t.Type {
		case "cron", "webhook", "event", "message":
			// valid trigger types
		default:
			return fmt.Errorf("trigger %d has unknown type %q", i, t.Type)
		}
	}

	return nil
}

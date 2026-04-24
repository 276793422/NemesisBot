package workflow

import (
	"fmt"
	"strings"
	"sync"
)

// TriggerManager manages workflow triggers.
// It supports cron schedules, webhook endpoints, event matching, and message patterns.
type TriggerManager struct {
	// triggers maps workflow names to their trigger configs.
	triggers map[string][]TriggerConfig

	// cronJobs maps workflow names to cron expression strings.
	cronJobs map[string][]string

	mu sync.RWMutex
}

// NewTriggerManager creates a new trigger manager.
func NewTriggerManager() *TriggerManager {
	return &TriggerManager{
		triggers: make(map[string][]TriggerConfig),
		cronJobs: make(map[string][]string),
	}
}

// RegisterTrigger registers a trigger for a workflow.
// Returns an error if the trigger type is unknown.
func (tm *TriggerManager) RegisterTrigger(workflowName string, trigger TriggerConfig) error {
	switch trigger.Type {
	case "cron", "webhook", "event", "message":
		// valid
	default:
		return fmt.Errorf("unknown trigger type %q for workflow %q", trigger.Type, workflowName)
	}

	tm.mu.Lock()
	defer tm.mu.Unlock()

	tm.triggers[workflowName] = append(tm.triggers[workflowName], trigger)

	// Track cron jobs separately for easy lookup
	if trigger.Type == "cron" {
		if expr := getTriggerString(trigger.Config, "expression"); expr != "" {
			tm.cronJobs[workflowName] = append(tm.cronJobs[workflowName], expr)
		}
	}

	return nil
}

// RemoveTrigger removes all triggers for a workflow.
func (tm *TriggerManager) RemoveTrigger(workflowName string) error {
	tm.mu.Lock()
	defer tm.mu.Unlock()

	delete(tm.triggers, workflowName)
	delete(tm.cronJobs, workflowName)
	return nil
}

// RegisterWorkflowTriggers registers all triggers defined in a Workflow.
func (tm *TriggerManager) RegisterWorkflowTriggers(wf *Workflow) error {
	for _, t := range wf.Triggers {
		if err := tm.RegisterTrigger(wf.Name, t); err != nil {
			return err
		}
	}
	return nil
}

// MatchEvent returns workflow names that should be triggered by an event.
// The eventType is matched against trigger types, and the data map is
// used for additional matching criteria.
func (tm *TriggerManager) MatchEvent(eventType string, data map[string]interface{}) []string {
	tm.mu.RLock()
	defer tm.mu.RUnlock()

	var matched []string

	for wfName, triggers := range tm.triggers {
		for _, trigger := range triggers {
			if trigger.Type != eventType {
				continue
			}

			if tm.matchTriggerData(trigger, data) {
				matched = append(matched, wfName)
				break // one match per workflow is enough
			}
		}
	}

	return matched
}

// GetCronWorkflows returns all workflow names that have cron triggers.
func (tm *TriggerManager) GetCronWorkflows() map[string][]string {
	tm.mu.RLock()
	defer tm.mu.RUnlock()

	result := make(map[string][]string, len(tm.cronJobs))
	for k, v := range tm.cronJobs {
		result[k] = append([]string{}, v...)
	}
	return result
}

// GetWebhookWorkflows returns all workflow names that have webhook triggers.
func (tm *TriggerManager) GetWebhookWorkflows() []string {
	tm.mu.RLock()
	defer tm.mu.RUnlock()

	var names []string
	for wfName, triggers := range tm.triggers {
		for _, t := range triggers {
			if t.Type == "webhook" {
				names = append(names, wfName)
				break
			}
		}
	}
	return names
}

// ListTriggers returns all triggers for a workflow.
func (tm *TriggerManager) ListTriggers(workflowName string) []TriggerConfig {
	tm.mu.RLock()
	defer tm.mu.RUnlock()

	return append([]TriggerConfig{}, tm.triggers[workflowName]...)
}

// ListAllTriggers returns all registered triggers across all workflows.
func (tm *TriggerManager) ListAllTriggers() map[string][]TriggerConfig {
	tm.mu.RLock()
	defer tm.mu.RUnlock()

	result := make(map[string][]TriggerConfig, len(tm.triggers))
	for k, v := range tm.triggers {
		result[k] = append([]TriggerConfig{}, v...)
	}
	return result
}

// matchTriggerData checks if the event data matches the trigger's config criteria.
func (tm *TriggerManager) matchTriggerData(trigger TriggerConfig, data map[string]interface{}) bool {
	if len(trigger.Config) == 0 {
		return true // no filter means match all
	}

	for key, expected := range trigger.Config {
		actual, ok := data[key]
		if !ok {
			return false
		}

		// String comparison
		expectedStr := fmt.Sprintf("%v", expected)
		actualStr := fmt.Sprintf("%v", actual)

		// Support glob-like matching with "*"
		if strings.Contains(expectedStr, "*") {
			if !matchGlob(expectedStr, actualStr) {
				return false
			}
		} else {
			if expectedStr != actualStr {
				return false
			}
		}
	}

	return true
}

// matchGlob does simple glob matching with "*" wildcard.
func matchGlob(pattern, s string) bool {
	parts := strings.Split(pattern, "*")
	if len(parts) == 1 {
		return pattern == s
	}

	// Must start with first part
	if !strings.HasPrefix(s, parts[0]) {
		return false
	}

	// Must end with last part
	if !strings.HasSuffix(s, parts[len(parts)-1]) {
		return false
	}

	// Check middle parts appear in order
	idx := len(parts[0])
	for i := 1; i < len(parts)-1; i++ {
		pos := strings.Index(s[idx:], parts[i])
		if pos < 0 {
			return false
		}
		idx += pos + len(parts[i])
	}

	return true
}

// getTriggerString extracts a string value from a trigger config map.
func getTriggerString(config map[string]interface{}, key string) string {
	if v, ok := config[key]; ok {
		if s, ok := v.(string); ok {
			return s
		}
		return fmt.Sprintf("%v", v)
	}
	return ""
}

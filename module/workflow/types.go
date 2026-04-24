package workflow

import (
	"context"
	"time"
)

// Workflow represents a complete workflow definition.
type Workflow struct {
	Name        string            `json:"name" yaml:"name"`
	Description string            `json:"description" yaml:"description"`
	Version     string            `json:"version" yaml:"version"`
	Triggers    []TriggerConfig   `json:"triggers,omitempty" yaml:"triggers,omitempty"`
	Nodes       []NodeDef         `json:"nodes" yaml:"nodes"`
	Edges       []Edge            `json:"edges" yaml:"edges"`
	Variables   map[string]string `json:"variables,omitempty" yaml:"variables,omitempty"`
	Metadata    map[string]string `json:"metadata,omitempty" yaml:"metadata,omitempty"`
}

// NodeDef defines a workflow node.
type NodeDef struct {
	ID         string                 `json:"id" yaml:"id"`
	Type       string                 `json:"type" yaml:"type"` // llm, tool, condition, parallel, loop, sub_workflow, transform, http, script, delay, human_review
	Config     map[string]interface{} `json:"config" yaml:"config"`
	DependsOn  []string               `json:"depends_on,omitempty" yaml:"depends_on,omitempty"`
	RetryCount int                    `json:"retry_count,omitempty" yaml:"retry_count,omitempty"`
	Timeout    string                 `json:"timeout,omitempty" yaml:"timeout,omitempty"` // duration string
}

// Edge defines a connection between nodes.
type Edge struct {
	From      string `json:"from" yaml:"from"`
	To        string `json:"to" yaml:"to"`
	Condition string `json:"condition,omitempty" yaml:"condition,omitempty"` // for conditional edges
}

// TriggerConfig defines a workflow trigger.
type TriggerConfig struct {
	Type   string                 `json:"type" yaml:"type"` // cron, webhook, event, message
	Config map[string]interface{} `json:"config" yaml:"config"`
}

// ExecutionState tracks workflow execution state.
type ExecutionState int

const (
	StatePending ExecutionState = iota
	StateRunning
	StateCompleted
	StateFailed
	StateCancelled
	StateWaiting // waiting for human review
)

func (s ExecutionState) String() string {
	switch s {
	case StatePending:
		return "pending"
	case StateRunning:
		return "running"
	case StateCompleted:
		return "completed"
	case StateFailed:
		return "failed"
	case StateCancelled:
		return "cancelled"
	case StateWaiting:
		return "waiting"
	default:
		return "unknown"
	}
}

// MarshalJSON implements json.Marshaler for ExecutionState.
func (s ExecutionState) MarshalJSON() ([]byte, error) {
	return []byte(`"` + s.String() + `"`), nil
}

// UnmarshalJSON implements json.Unmarshaler for ExecutionState.
func (s *ExecutionState) UnmarshalJSON(data []byte) error {
	str := string(data)
	str = str[1 : len(str)-1] // strip quotes
	switch str {
	case "pending":
		*s = StatePending
	case "running":
		*s = StateRunning
	case "completed":
		*s = StateCompleted
	case "failed":
		*s = StateFailed
	case "cancelled":
		*s = StateCancelled
	case "waiting":
		*s = StateWaiting
	default:
		*s = StatePending
	}
	return nil
}

// NodeResult holds the result of a node execution.
type NodeResult struct {
	NodeID    string                 `json:"node_id"`
	Output    interface{}            `json:"output"`
	Error     string                 `json:"error,omitempty"`
	State     ExecutionState         `json:"state"`
	StartedAt time.Time              `json:"started_at"`
	EndedAt   time.Time              `json:"ended_at"`
	Metadata  map[string]interface{} `json:"metadata,omitempty"`
}

// Execution represents a workflow execution instance.
type Execution struct {
	ID           string                 `json:"id"`
	WorkflowName string                 `json:"workflow_name"`
	State        ExecutionState         `json:"state"`
	Input        map[string]interface{} `json:"input,omitempty"`
	NodeResults  map[string]*NodeResult `json:"node_results"`
	Variables    map[string]string      `json:"variables,omitempty"`
	StartedAt    time.Time              `json:"started_at"`
	EndedAt      time.Time              `json:"ended_at,omitempty"`
	Error        string                 `json:"error,omitempty"`
}

// NodeExecutor is the interface for executing workflow nodes.
type NodeExecutor interface {
	Execute(ctx context.Context, node *NodeDef, wfCtx *Context) (*NodeResult, error)
	Type() string
}

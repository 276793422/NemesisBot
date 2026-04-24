package workflow

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
)

// Engine is the main DAG workflow execution engine.
// It manages workflow registration, execution, and state persistence.
type Engine struct {
	workflows  map[string]*Workflow
	executions map[string]*Execution
	executors  *ExecutorRegistry

	persistenceDir string

	mu sync.RWMutex
}

// NewEngine creates a new workflow engine.
// persistenceDir is the directory where execution state is saved.
// If empty, persistence is disabled.
func NewEngine(persistenceDir string) *Engine {
	e := &Engine{
		workflows:      make(map[string]*Workflow),
		executions:     make(map[string]*Execution),
		executors:      DefaultExecutorRegistry(),
		persistenceDir: persistenceDir,
	}

	// Register sub_workflow node with engine reference
	e.executors.Register("sub_workflow", &SubWorkflowNode{Engine: e})

	return e
}

// Register adds a workflow definition to the engine.
// The workflow is validated before registration.
func (e *Engine) Register(wf *Workflow) error {
	if err := Validate(wf); err != nil {
		return fmt.Errorf("validate workflow %q: %w", wf.Name, err)
	}

	e.mu.Lock()
	defer e.mu.Unlock()
	e.workflows[wf.Name] = wf
	return nil
}

// Unregister removes a workflow definition from the engine.
func (e *Engine) Unregister(name string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	delete(e.workflows, name)
}

// Run executes a registered workflow by name.
func (e *Engine) Run(ctx context.Context, name string, input map[string]interface{}) (*Execution, error) {
	e.mu.RLock()
	wf, ok := e.workflows[name]
	e.mu.RUnlock()

	if !ok {
		return nil, fmt.Errorf("workflow %q not found", name)
	}

	exec := &Execution{
		ID:           uuid.New().String(),
		WorkflowName: name,
		State:        StateRunning,
		Input:        input,
		NodeResults:  make(map[string]*NodeResult),
		Variables:    make(map[string]string),
		StartedAt:    time.Now(),
	}

	// Initialize variables from workflow definition
	for k, v := range wf.Variables {
		exec.Variables[k] = v
	}

	// Add input to variables (input.xxx accessible via context)
	wfCtx := NewContext(input)
	for k, v := range exec.Variables {
		wfCtx.SetVar(k, v)
	}

	// Store execution
	e.mu.Lock()
	e.executions[exec.ID] = exec
	e.mu.Unlock()

	// Persist initial state
	e.persistExecution(exec)

	// Execute the workflow
	scheduleErr := Schedule(ctx, wf.Nodes, wf.Edges, e.executors, wfCtx)

	now := time.Now()
	exec.EndedAt = now

	if scheduleErr != nil {
		if errors.Is(scheduleErr, context.Canceled) {
			exec.State = StateCancelled
		} else {
			exec.State = StateFailed
		}
		exec.Error = scheduleErr.Error()
	} else {
		// Check if any node is in waiting state (human review)
		allCompleted := true
		for _, result := range wfCtx.NodeResults {
			if result.State == StateWaiting {
				exec.State = StateWaiting
				allCompleted = false
				break
			}
		}
		if allCompleted {
			exec.State = StateCompleted
		}
	}

	// Copy results from context
	exec.NodeResults = wfCtx.NodeResults
	exec.Variables = wfCtx.GetAllVariables()

	// Persist final state
	e.persistExecution(exec)

	return exec, scheduleErr
}

// GetExecution retrieves an execution by its ID.
func (e *Engine) GetExecution(id string) (*Execution, error) {
	e.mu.RLock()
	defer e.mu.RUnlock()

	exec, ok := e.executions[id]
	if !ok {
		// Try loading from persistence
		if e.persistenceDir != "" {
			loaded, err := e.loadExecutionFromDisk(id)
			if err == nil {
				return loaded, nil
			}
		}
		return nil, fmt.Errorf("execution %q not found", id)
	}
	return exec, nil
}

// CancelExecution marks an execution as cancelled.
func (e *Engine) CancelExecution(id string) error {
	e.mu.Lock()
	defer e.mu.Unlock()

	exec, ok := e.executions[id]
	if !ok {
		return fmt.Errorf("execution %q not found", id)
	}

	if exec.State != StateRunning && exec.State != StateWaiting {
		return fmt.Errorf("execution %q is not running (state=%s)", id, exec.State)
	}

	exec.State = StateCancelled
	exec.EndedAt = time.Now()
	e.persistExecution(exec)

	return nil
}

// ResumeExecution resumes a waiting execution (e.g., after human review).
// The reviewer response is passed via the reviewResult map.
func (e *Engine) ResumeExecution(id string, reviewResult map[string]interface{}) error {
	e.mu.Lock()
	defer e.mu.Unlock()

	exec, ok := e.executions[id]
	if !ok {
		return fmt.Errorf("execution %q not found", id)
	}

	if exec.State != StateWaiting {
		return fmt.Errorf("execution %q is not waiting (state=%s)", id, exec.State)
	}

	// Find the waiting node and update its result
	for nodeID, result := range exec.NodeResults {
		if result.State == StateWaiting {
			result.Output = reviewResult
			result.State = StateCompleted
			result.EndedAt = time.Now()
			exec.NodeResults[nodeID] = result

			// Set variable for downstream nodes
			if approved, ok := reviewResult["approved"]; ok {
				if b, ok := approved.(bool); ok {
					if exec.Variables == nil {
						exec.Variables = make(map[string]string)
					}
					exec.Variables[nodeID+"_approved"] = fmt.Sprintf("%v", b)
				}
			}
			break
		}
	}

	exec.State = StateCompleted
	exec.EndedAt = time.Now()
	e.persistExecution(exec)

	return nil
}

// ListWorkflows returns the names of all registered workflows.
func (e *Engine) ListWorkflows() []string {
	e.mu.RLock()
	defer e.mu.RUnlock()

	names := make([]string, 0, len(e.workflows))
	for name := range e.workflows {
		names = append(names, name)
	}
	return names
}

// ListExecutions returns all executions, optionally filtered by workflow name.
func (e *Engine) ListExecutions(workflowName string) []*Execution {
	e.mu.RLock()
	defer e.mu.RUnlock()

	var result []*Execution
	for _, exec := range e.executions {
		if workflowName == "" || exec.WorkflowName == workflowName {
			result = append(result, exec)
		}
	}
	return result
}

// GetWorkflow returns a registered workflow by name.
func (e *Engine) GetWorkflow(name string) (*Workflow, bool) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	wf, ok := e.workflows[name]
	return wf, ok
}

// Close stops the engine and cleans up resources.
func (e *Engine) Close() {
	// Future: cancel running executions, close resources
}

// persistExecution saves the execution state to disk if persistence is enabled.
func (e *Engine) persistExecution(exec *Execution) {
	if e.persistenceDir == "" {
		return
	}
	// Ignore error - persistence is best-effort
	_ = SaveExecution(e.persistenceDir, exec)
}

// loadExecutionFromDisk loads an execution from disk.
func (e *Engine) loadExecutionFromDisk(id string) (*Execution, error) {
	if e.persistenceDir == "" {
		return nil, fmt.Errorf("persistence disabled")
	}
	// Search all workflow directories
	return LoadExecutionByID(e.persistenceDir, id)
}

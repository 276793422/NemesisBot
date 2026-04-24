package workflow

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// TopologicalSort performs a topological sort on the workflow graph.
// It returns execution levels where each level contains node IDs that
// can be executed in parallel. Returns an error if a cycle is detected.
func TopologicalSort(nodes []NodeDef, edges []Edge) ([][]string, error) {
	// Build adjacency list and in-degree map
	inDegree := make(map[string]int)
	adjacency := make(map[string][]string)
	nodeIDs := make(map[string]bool)

	for _, n := range nodes {
		nodeIDs[n.ID] = true
		inDegree[n.ID] = 0
	}

	for _, e := range edges {
		adjacency[e.From] = append(adjacency[e.From], e.To)
		inDegree[e.To]++
	}

	// Also account for DependsOn
	for _, n := range nodes {
		for _, dep := range n.DependsOn {
			adjacency[dep] = append(adjacency[dep], n.ID)
			inDegree[n.ID]++
		}
	}

	// Kahn's algorithm with level tracking
	var levels [][]string
	var queue []string

	// Seed with nodes that have no incoming edges
	for id := range nodeIDs {
		if inDegree[id] == 0 {
			queue = append(queue, id)
		}
	}

	visited := 0

	for len(queue) > 0 {
		levels = append(levels, append([]string{}, queue...))
		var nextQueue []string

		for _, id := range queue {
			visited++
			for _, neighbor := range adjacency[id] {
				inDegree[neighbor]--
				if inDegree[neighbor] == 0 {
					nextQueue = append(nextQueue, neighbor)
				}
			}
		}

		queue = nextQueue
	}

	if visited != len(nodeIDs) {
		return nil, fmt.Errorf("cycle detected in workflow graph")
	}

	return levels, nil
}

// Schedule executes workflow nodes respecting dependencies and parallelism.
// Nodes at the same topological level are executed concurrently.
func Schedule(ctx context.Context, nodes []NodeDef, edges []Edge, executors *ExecutorRegistry, wfCtx *Context) error {
	// Build node lookup map
	nodeMap := make(map[string]*NodeDef, len(nodes))
	for i := range nodes {
		nodeMap[nodes[i].ID] = &nodes[i]
	}

	// Build conditional edge map: from -> []conditional edge
	condEdges := make(map[string][]Edge)
	uncondEdges := make(map[string][]string)
	for _, e := range edges {
		if e.Condition != "" {
			condEdges[e.From] = append(condEdges[e.From], e)
		} else {
			uncondEdges[e.From] = append(uncondEdges[e.From], e.To)
		}
	}

	// Compute execution levels
	levels, err := TopologicalSort(nodes, edges)
	if err != nil {
		return err
	}

	// Execute level by level
	for _, level := range levels {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		// Filter nodes that should run based on conditional edges from previous nodes
		var runnableNodes []string
		for _, nodeID := range level {
			if shouldRunNode(nodeID, condEdges, wfCtx) {
				runnableNodes = append(runnableNodes, nodeID)
			}
		}

		if len(runnableNodes) == 0 {
			continue
		}

		// Execute all nodes in this level concurrently
		var wg sync.WaitGroup
		errCh := make(chan error, len(runnableNodes))

		for _, nodeID := range runnableNodes {
			wg.Add(1)
			go func(id string) {
				defer wg.Done()

				node := nodeMap[id]
				if node == nil {
					errCh <- fmt.Errorf("node %q not found in definition", id)
					return
				}

				executor := executors.Get(node.Type)
				if executor == nil {
					errCh <- fmt.Errorf("no executor for node type %q (node %s)", node.Type, id)
					return
				}

				// Apply node-level timeout
				nodeCtx := ctx
				if node.Timeout != "" {
					timeout, parseErr := time.ParseDuration(node.Timeout)
					if parseErr == nil {
						var cancel context.CancelFunc
						nodeCtx, cancel = context.WithTimeout(ctx, timeout)
						defer cancel()
					}
				}

				// Execute with retry
				var result *NodeResult
				var execErr error
				maxRetries := node.RetryCount
				if maxRetries < 0 {
					maxRetries = 0
				}

				for attempt := 0; attempt <= maxRetries; attempt++ {
					result, execErr = executor.Execute(nodeCtx, node, wfCtx)
					if execErr == nil && result.State != StateFailed {
						break
					}
					if attempt < maxRetries {
						select {
						case <-ctx.Done():
							return
						case <-time.After(time.Duration(attempt+1) * 500 * time.Millisecond):
							// Exponential-ish backoff
						}
					}
				}

				if execErr != nil {
					errCh <- fmt.Errorf("node %q execution failed: %w", id, execErr)
					return
				}

				wfCtx.SetNodeResult(id, result)

				// If node produced variables, set them in context
				if result.Output != nil {
					if m, ok := result.Output.(map[string]interface{}); ok {
						for k, v := range m {
							wfCtx.SetVar(id+"."+k, fmt.Sprintf("%v", v))
						}
					}
				}
			}(nodeID)
		}

		wg.Wait()
		close(errCh)

		// Collect errors
		var errs []error
		for err := range errCh {
			errs = append(errs, err)
		}

		if len(errs) > 0 {
			// Return first error
			return errs[0]
		}
	}

	return nil
}

// shouldRunNode checks if a node should be executed based on conditional
// edges from its predecessors. If there are no conditional edges leading
// to this node, it should always run.
func shouldRunNode(nodeID string, condEdges map[string][]Edge, wfCtx *Context) bool {
	// Check if any node has a conditional edge pointing to this node
	for _, edges := range condEdges {
		for _, e := range edges {
			if e.To == nodeID {
				// Evaluate the condition
				resolved := wfCtx.Resolve(e.Condition)
				result, err := Evaluate(resolved, wfCtx.GetAllVariables())
				if err != nil {
					// On evaluation error, skip the node
					return false
				}
				if !result {
					return false
				}
			}
		}
	}
	return true
}

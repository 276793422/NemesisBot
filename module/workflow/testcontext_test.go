package workflow_test

import (
	"context"
	"testing"
	"time"
)

// testContext wraps a context with cancellation for tests.
type testContext struct {
	ctx    context.Context
	cancel context.CancelFunc
}

func newTestContext(t *testing.T) *testContext {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	return &testContext{ctx: ctx, cancel: cancel}
}

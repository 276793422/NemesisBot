package services

import (
	"context"

	"golang.org/x/sync/errgroup"
)

// parallelInit runs multiple initialization functions in parallel using errgroup.
// It returns the first error encountered by any init function.
// If ctx is cancelled, all in-flight init functions receive cancellation.
func parallelInit(ctx context.Context, inits ...func() error) error {
	g, gctx := errgroup.WithContext(ctx)

	for _, initFn := range inits {
		fn := initFn // capture loop variable
		g.Go(func() error {
			// Check if context is already cancelled before starting
			select {
			case <-gctx.Done():
				return gctx.Err()
			default:
			}
			return fn()
		})
	}

	return g.Wait()
}

// sequentialInit runs initialization functions one after another.
// It returns the first error encountered.
// This is used for components that have ordering dependencies.
func sequentialInit(inits ...func() error) error {
	for _, initFn := range inits {
		if err := initFn(); err != nil {
			return err
		}
	}
	return nil
}

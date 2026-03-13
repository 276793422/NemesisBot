// Package cluster provides cluster configuration and management utilities
package cluster

import (
	"time"
)

// GetCurrentTime returns current time (wrapper for time.Now())
// This is useful for testing and consistency
func GetCurrentTime() time.Time {
	return time.Now()
}

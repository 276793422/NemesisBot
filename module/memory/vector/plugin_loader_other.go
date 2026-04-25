//go:build !windows

package vector

import (
	"fmt"
	"runtime"
)

// LoadPlugin returns an error on non-Windows platforms.
// Linux/macOS support requires dlopen-based implementation.
func LoadPlugin(path string) (EmbeddingPlugin, error) {
	return nil, fmt.Errorf("vector: plugin loading not supported on %s", runtime.GOOS)
}

package desktop

// Config holds configuration for the desktop UI (all platforms)
type Config struct {
	Enabled bool
	Width   int
	Height  int
	X       int
	Y       int
	Debug   bool
}

// SystemInfo holds system information
type SystemInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
	Status  string `json:"status"`
	OS      string `json:"os"`
	Arch    string `json:"arch"`
}

// WebView2VersionInfo holds WebView2 version information (Windows only)
type WebView2VersionInfo struct {
	Installed bool
	Version   string
	Path      string
}

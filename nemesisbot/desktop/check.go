package desktop

import (
	"os/exec"
	"runtime"
	"syscall"
	"unsafe"

	"github.com/276793422/NemesisBot/module/logger"
)

const (
	WebView2DownloadURL = "https://developer.microsoft.com/en-us/microsoft-edge/webview2/"

	// Windows API constants
	MB_YESNO       = 0x00000004
	MB_ICONWARNING = 0x00000030
	MB_DEFBUTTON2  = 0x00000100
	IDYES          = 6
	IDNO           = 7
	IDCANCEL       = 2
)

var (
	user32            = syscall.NewLazyDLL("user32.dll")
	procMessageBoxW   = user32.NewProc("MessageBoxW")
	shell32           = syscall.NewLazyDLL("shell32.dll")
	procShellExecuteW = shell32.NewProc("ShellExecuteW")
)

// WebView2VersionInfo holds WebView2 version information
type WebView2VersionInfo struct {
	Installed bool
	Version   string
	Path      string
}

// CheckSystemRequirements checks if the system meets requirements
func CheckSystemRequirements() bool {
	// Check if running on Windows
	if runtime.GOOS != "windows" {
		logger.ErrorC("desktop", "Desktop UI is only supported on Windows")
		logger.InfoC("desktop", "TODO: Implement desktop UI for other platforms (macOS, Linux)")
		logger.InfoC("desktop", "For now, please use the web interface or run on Windows")
		return false
	}

	// Check WebView2 Runtime installation
	info := checkWebView2Installation()

	if !info.Installed {
		logger.WarnC("desktop", "WebView2 Runtime is not installed")
		logger.InfoC("desktop", "WebView2 Runtime is required to run NemesisBot Desktop")
		logger.InfoCF("desktop", "Download URL", map[string]interface{}{
			"url": WebView2DownloadURL,
		})

		// Show message box to user
		showWebView2DownloadPrompt()

		return false
	}

	logger.InfoCF("desktop", "WebView2 Runtime detected", map[string]interface{}{
		"version": info.Version,
		"path":    info.Path,
	})

	return true
}

// checkWebView2Installation checks if WebView2 Runtime is installed
func checkWebView2Installation() WebView2VersionInfo {
	// Method 1: Check registry for WebView2 Runtime
	// HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}
	version := checkRegistryWebView2()
	if version != "" {
		return WebView2VersionInfo{
			Installed: true,
			Version:   version,
			Path:      "Registry",
		}
	}

	// Method 2: Check for WebView2Loader.dll in common locations
	paths := []string{
		"C:\\Windows\\System32\\WebView2Loader.dll",
		"C:\\Windows\\SysWOW64\\WebView2Loader.dll",
		"C:\\Program Files\\Microsoft Edge WebView2 Runtime\\",
	}

	for _, path := range paths {
		if checkFileExists(path) {
			return WebView2VersionInfo{
				Installed: true,
				Version:   "Detected",
				Path:      path,
			}
		}
	}

	return WebView2VersionInfo{
		Installed: false,
	}
}

// checkRegistryWebView2 checks registry for WebView2 version
func checkRegistryWebView2() string {
	// Try to read WebView2 version from registry
	cmd := exec.Command("reg", "query",
		"HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
		"/v", "pv")

	output, err := cmd.CombinedOutput()
	if err != nil {
		// Try alternate registry path
		cmd = exec.Command("reg", "query",
			"HKCU\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
			"/v", "pv")
		output, err = cmd.CombinedOutput()
		if err != nil {
			return ""
		}
	}

	// Parse version from output
	// Expected output: "pv    REG_SZ    1.0.1.2"
	if len(output) > 0 {
		str := string(output)
		// Simple parsing - look for version pattern
		for i := 0; i < len(str)-3; i++ {
			if str[i] >= '0' && str[i] <= '9' {
				// Found start of version
				end := i
				for end < len(str) && (str[end] >= '0' && str[end] <= '9' || str[end] == '.') {
					end++
				}
				if end > i && end-i < 20 { // Reasonable version length
					return str[i:end]
				}
			}
		}
	}

	return ""
}

// checkFileExists checks if a file or directory exists
func checkFileExists(path string) bool {
	_, err := exec.Command("cmd", "/c", "if exist "+path+" (echo exists)").CombinedOutput()
	return err == nil
}

// showWebView2DownloadPrompt shows a message box prompting user to download WebView2
func showWebView2DownloadPrompt() {
	message := `WebView2 Runtime is required to run NemesisBot Desktop.

Would you like to download and install it now?

After installation, please restart NemesisBot Desktop.`

	title := "NemesisBot Desktop - Missing Component"

	// Convert strings to UTF-16 LE for Windows API
	messagePtr := syscall.StringToUTF16Ptr(message)
	titlePtr := syscall.StringToUTF16Ptr(title)

	// Call MessageBoxW
	// MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 (default to No)
	ret, _, _ := procMessageBoxW.Call(
		0, // hWnd (no owner window)
		uintptr(unsafe.Pointer(messagePtr)),
		uintptr(unsafe.Pointer(titlePtr)),
		MB_YESNO|MB_ICONWARNING|MB_DEFBUTTON2,
	)

	if ret == IDYES {
		logger.InfoC("desktop", "User chose to download WebView2 Runtime")
		openDownloadPage()
	} else {
		logger.InfoC("desktop", "User declined to download WebView2 Runtime")
	}
}

// openDownloadPage opens the WebView2 download page in default browser
func openDownloadPage() {
	logger.InfoCF("desktop", "Opening download page", map[string]interface{}{
		"url": WebView2DownloadURL,
	})

	// Convert URL to UTF-16 LE
	urlPtr := syscall.StringToUTF16Ptr(WebView2DownloadURL)
	operationPtr := syscall.StringToUTF16Ptr("open")

	// Call ShellExecuteW to open URL in default browser
	procShellExecuteW.Call(
		0, // hWnd
		uintptr(unsafe.Pointer(operationPtr)),
		uintptr(unsafe.Pointer(urlPtr)),
		0, // parameters
		0, // directory
		1, // SW_SHOWNORMAL
	)
}

// GetWebView2Version returns detailed version information
func GetWebView2Version() *WebView2VersionInfo {
	info := checkWebView2Installation()
	return &info
}

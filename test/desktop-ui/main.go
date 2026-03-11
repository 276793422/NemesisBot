package main

import (
	"embed"
	"fmt"
	"io/fs"
	"log"
	"net"
	"net/http"
	"os"
	"os/exec"
	"runtime"
	"time"
)

//go:embed static
var staticFiles embed.FS

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	command := os.Args[1]

	switch command {
	case "desktop":
		runDesktop()
	case "web":
		runWebServer()
	default:
		fmt.Printf("Unknown command: %s\n", command)
		printUsage()
		os.Exit(1)
	}
}

func printUsage() {
	fmt.Println("NemesisBot Desktop UI - Prototype")
	fmt.Println()
	fmt.Println("Usage: desktop-ui <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  desktop    Launch desktop UI (opens in browser window)")
	fmt.Println("  web        Launch web server only")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  desktop-ui desktop")
	fmt.Println("  desktop-ui web")
	fmt.Println()
	fmt.Println("Note: For this prototype, 'desktop' mode opens a browser")
	fmt.Println("      window. The full version will use WebView for a")
	fmt.Println("      native desktop window.")
}

// runDesktop launches the desktop UI
func runDesktop() {
	log.SetFlags(log.Ltime | log.Lshortfile)
	log.Println("========================================")
	log.Println(" NemesisBot Desktop UI - Prototype")
	log.Println("========================================")
	log.Println("")

	// Start web server
	port := startWebServer()
	url := fmt.Sprintf("http://127.0.0.1:%s", port)

	log.Printf("✓ Server started on http://127.0.0.1:%s", port)
	log.Println("✓ Initializing desktop mode...")
	log.Println("")

	// Open in browser with app-like window
	log.Println("Opening browser window...")
	openBrowserWindow(url)

	log.Println("")
	log.Println("========================================")
	log.Println(" NemesisBot Desktop UI is running!")
	log.Println("========================================")
	log.Println("")
	log.Printf("URL: %s", url)
	log.Println("")
	log.Println("Controls:")
	log.Println("  - Click on navigation items to switch pages")
	log.Println("  - Type in the chat box and click Send")
	log.Println("  - Try Ctrl+Enter to send messages")
	log.Println("  - Switch between light/dark themes")
	log.Println("")
	log.Println("Press Ctrl+C to stop the server")
	log.Println("")

	// Keep running
	select {}
}

// runWebServer starts only the web server
func runWebServer() {
	log.SetFlags(log.Ltime | log.Lshortfile)
	log.Println("Starting NemesisBot Web UI...")
	log.Println("")

	// Start web server
	port := startWebServer()
	url := fmt.Sprintf("http://127.0.0.1:%s", port)

	log.Printf("Server started on %s", url)
	log.Println("")
	log.Println("Open your browser and visit:")
	log.Printf("  %s", url)
	log.Println("")
	log.Println("Press Ctrl+C to stop")

	// Keep running
	select {}
}

// startWebServer starts the embedded web server
func startWebServer() string {
	// Create a sub-filesystem for static files
	staticFS, err := fs.Sub(staticFiles, "static")
	if err != nil {
		log.Fatal("Error creating static filesystem:", err)
	}

	// Create file server handler
	fileServer := http.FileServer(http.FS(staticFS))

	// Create mux for handling routes
	mux := http.NewServeMux()

	// Serve static files
	mux.Handle("/static/", http.StripPrefix("/static/", fileServer))

	// Serve index.html at root
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/" {
			index, err := staticFiles.ReadFile("static/index.html")
			if err != nil {
				http.Error(w, "Index not found", http.StatusNotFound)
				return
			}
			w.Header().Set("Content-Type", "text/html")
			w.Write(index)
		} else {
			http.FileServer(http.FS(staticFS)).ServeHTTP(w, r)
		}
	})

	// Health check endpoint
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"status":"ok","version":"0.0.1","mode":"desktop"}`))
	})

	// API endpoints for JavaScript
	mux.HandleFunc("/api/config", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{
			"name": "NemesisBot",
			"version": "0.0.1",
			"theme": "dark",
			"status": "running",
			"os": "` + runtime.GOOS + `",
			"arch": "` + runtime.GOARCH + `"
		}`))
	})

	mux.HandleFunc("/api/test", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"message":"Hello from NemesisBot Desktop!"}`))
	})

	// Find an available port
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		log.Fatal("Error starting server:", err)
	}

	port := fmt.Sprintf("%d", listener.Addr().(*net.TCPAddr).Port)

	// Start server
	go func() {
		if err := http.Serve(listener, mux); err != nil && err != http.ErrServerClosed {
			log.Printf("Server error: %v\n", err)
		}
	}()

	// Wait a bit for server to start
	time.Sleep(100 * time.Millisecond)

	return port
}

// openBrowserWindow opens the URL in a browser window
func openBrowserWindow(url string) {
	switch runtime.GOOS {
	case "windows":
		// On Windows, open with Microsoft Edge for best experience
		cmd := exec.Command("cmd", "/c", "start", "msedge", "--kiosk", url,
			"--app="+url,
			"--disable-web-security",
			"--disable-features=TranslateUI")
		go func() {
			_ = cmd.Start()
			log.Println("✓ Opened in Microsoft Edge (app mode)")
		}()

	case "darwin":
		// On macOS, open in Safari or Chrome
		cmd := exec.Command("open", "-a", "Safari", url)
		go func() {
			_ = cmd.Start()
			log.Println("✓ Opened in Safari")
		}()

	default: // Linux
		// Try different browsers on Linux
		browsers := []string{"google-chrome", "chromium", "firefox"}
		for _, browser := range browsers {
			cmd := exec.Command("xdg-open", url)
			if exec.Command("which", browser).Run() == nil {
				go func() {
					_ = cmd.Start()
					log.Printf("✓ Opened in %s", browser)
				}()
				break
			}
		}
	}
}

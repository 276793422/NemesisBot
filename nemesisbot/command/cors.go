// Package command implements CLI commands for NemesisBot
package command

import (
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/path"
)

// CmdCORS manages CORS configuration
func CmdCORS() {
	if len(os.Args) < 3 {
		CORSHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "list":
		cmdCORSList()
	case "add":
		cmdCORSAdd()
	case "remove":
		cmdCORSRemove()
	case "dev-mode":
		cmdCORSDevMode()
	case "show":
		cmdCORSShow()
	case "validate":
		cmdCORSValidate()
	default:
		fmt.Printf("Unknown CORS command: %s\n", subcommand)
		CORSHelp()
	}
}

// CORSHelp prints CORS command help
func CORSHelp() {
	fmt.Println("\nCORS Management Commands:")
	fmt.Println("  list                  List all allowed origins")
	fmt.Println("  add <origin>          Add an allowed origin")
	fmt.Println("  remove <origin>       Remove an allowed origin")
	fmt.Println("  dev-mode              Manage development mode")
	fmt.Println("  show                  Show CORS configuration")
	fmt.Println("  validate <origin>     Validate if an origin is allowed")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  add:")
	fmt.Println("    --cdn               Add as CDN domain instead of origin")
	fmt.Println()
	fmt.Println("  remove:")
	fmt.Println("    --cdn               Remove from CDN domains instead of origins")
	fmt.Println()
	fmt.Println("  dev-mode:")
	fmt.Println("    enable              Enable development mode (allows localhost)")
	fmt.Println("    disable             Disable development mode")
	fmt.Println("    status              Show development mode status")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # List allowed origins")
	fmt.Println("  nemesisbot cors list")
	fmt.Println()
	fmt.Println("  # Add an origin")
	fmt.Println("  nemesisbot cors add https://example.com")
	fmt.Println()
	fmt.Println("  # Add a CDN domain")
	fmt.Println("  nemesisbot cors add --cdn cdn.cloudflare.com")
	fmt.Println()
	fmt.Println("  # Remove an origin")
	fmt.Println("  nemesisbot cors remove https://example.com")
	fmt.Println()
	fmt.Println("  # Enable development mode")
	fmt.Println("  nemesisbot cors dev-mode enable")
	fmt.Println()
	fmt.Println("  # Validate an origin")
	fmt.Println("  nemesisbot cors validate https://example.com")
	fmt.Println()
}

// CORSConfig represents CORS configuration
type CORSConfig struct {
	AllowedOrigins    []string `json:"allowed_origins"`
	AllowedMethods    []string `json:"allowed_methods"`
	AllowedHeaders    []string `json:"allowed_headers"`
	AllowCredentials  bool     `json:"allow_credentials"`
	MaxAge            int      `json:"max_age"`
	AllowLocalhost    bool     `json:"allow_localhost"`
	DevelopmentMode   bool     `json:"development_mode"`
	AllowedCDNDomains []string `json:"allowed_cdn_domains"`
	AllowNoOrigin     bool     `json:"allow_no_origin"`
}

// cmdCORSList lists all allowed origins
func cmdCORSList() {
	// Load CORS config
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	fmt.Println("\nCORS Configuration:")
	fmt.Println("═" + strings.Repeat("═", 79))

	// Allowed Origins
	fmt.Println("\nAllowed Origins:")
	if len(config.AllowedOrigins) == 0 {
		fmt.Println("  (none)")
	} else {
		for i, origin := range config.AllowedOrigins {
			fmt.Printf("  %d. %s\n", i+1, origin)
		}
	}

	// CDN Domains
	fmt.Println("\nCDN Domains:")
	if len(config.AllowedCDNDomains) == 0 {
		fmt.Println("  (none)")
	} else {
		for i, cdn := range config.AllowedCDNDomains {
			fmt.Printf("  %d. %s\n", i+1, cdn)
		}
	}

	// Mode
	fmt.Println("\nMode:")
	if config.DevelopmentMode {
		fmt.Println("  🔧 Development (allows localhost)")
	} else {
		fmt.Println("  🏭 Production (strict whitelist)")
	}

	// Other settings
	fmt.Println("\nOther Settings:")
	fmt.Printf("  Allow localhost: %v\n", config.AllowLocalhost)
	fmt.Printf("  Allow no-origin: %v\n", config.AllowNoOrigin)
	fmt.Printf("  Allow credentials: %v\n", config.AllowCredentials)
	fmt.Printf("  Max age: %d seconds\n", config.MaxAge)

	fmt.Println("═" + strings.Repeat("═", 79))
}

// cmdCORSAdd adds an allowed origin
func cmdCORSAdd() {
	if len(os.Args) < 4 {
		fmt.Println("❌ Error: No origin provided")
		fmt.Println()
		fmt.Println("Usage:")
		fmt.Println("  nemesisbot cors add <origin>")
		fmt.Println("  nemesisbot cors add --cdn <cdn-domain>")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot cors add https://example.com")
		fmt.Println("  nemesisbot cors add --cdn cdn.cloudflare.com")
		return
	}

	// Parse flags
	isCDN := false
	origin := ""

	for i := 3; i < len(os.Args); i++ {
		arg := os.Args[i]
		switch arg {
		case "--cdn":
			isCDN = true
		default:
			if !strings.HasPrefix(arg, "--") && origin == "" {
				origin = arg
			}
		}
	}

	// Validate origin
	if origin == "" {
		fmt.Println("❌ Error: No origin provided")
		return
	}

	// Validate URL format (for origins, not CDN)
	if !isCDN {
		if _, err := url.Parse(origin); err != nil {
			fmt.Printf("❌ Error: Invalid URL format: %v\n", err)
			fmt.Println()
			fmt.Println("URL must include protocol, e.g.:")
			fmt.Println("  https://example.com")
			fmt.Println("  http://localhost:3000")
			return
		}

		if !strings.HasPrefix(origin, "http://") && !strings.HasPrefix(origin, "https://") {
			fmt.Println("❌ Error: Origin must start with http:// or https://")
			return
		}
	}

	// Load config
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	// Add to appropriate list
	if isCDN {
		// Check if already exists
		for _, cdn := range config.AllowedCDNDomains {
			if cdn == origin {
				fmt.Printf("ℹ️  CDN domain already exists: %s\n", origin)
				return
			}
		}

		config.AllowedCDNDomains = append(config.AllowedCDNDomains, origin)
		fmt.Printf("✅ Added CDN domain: %s\n", origin)
	} else {
		// Check if already exists
		for _, o := range config.AllowedOrigins {
			if o == origin {
				fmt.Printf("ℹ️  Origin already exists: %s\n", origin)
				return
			}
		}

		config.AllowedOrigins = append(config.AllowedOrigins, origin)
		fmt.Printf("✅ Added origin: %s\n", origin)
	}

	// Save config
	if err := saveCORSConfig(config); err != nil {
		fmt.Printf("❌ Error saving CORS config: %v\n", err)
		return
	}

	fmt.Println("ℹ️  CORS configuration updated")
	fmt.Println("ℹ️  Changes will be automatically reloaded within 30 seconds")
}

// cmdCORSRemove removes an allowed origin
func cmdCORSRemove() {
	if len(os.Args) < 4 {
		fmt.Println("❌ Error: No origin provided")
		fmt.Println()
		fmt.Println("Usage:")
		fmt.Println("  nemesisbot cors remove <origin>")
		fmt.Println("  nemesisbot cors remove --cdn <cdn-domain>")
		return
	}

	// Parse flags
	isCDN := false
	origin := ""

	for i := 3; i < len(os.Args); i++ {
		arg := os.Args[i]
		switch arg {
		case "--cdn":
			isCDN = true
		default:
			if !strings.HasPrefix(arg, "--") && origin == "" {
				origin = arg
			}
		}
	}

	// Validate origin
	if origin == "" {
		fmt.Println("❌ Error: No origin provided")
		return
	}

	// Load config
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	// Remove from appropriate list
	if isCDN {
		// Filter out the CDN domain
		newCDNs := []string{}
		found := false
		for _, cdn := range config.AllowedCDNDomains {
			if cdn != origin {
				newCDNs = append(newCDNs, cdn)
			} else {
				found = true
			}
		}

		if !found {
			fmt.Printf("ℹ️  CDN domain not found: %s\n", origin)
			return
		}

		config.AllowedCDNDomains = newCDNs
		fmt.Printf("✅ Removed CDN domain: %s\n", origin)
	} else {
		// Filter out the origin
		newOrigins := []string{}
		found := false
		for _, o := range config.AllowedOrigins {
			if o != origin {
				newOrigins = append(newOrigins, o)
			} else {
				found = true
			}
		}

		if !found {
			fmt.Printf("ℹ️  Origin not found: %s\n", origin)
			return
		}

		config.AllowedOrigins = newOrigins
		fmt.Printf("✅ Removed origin: %s\n", origin)
	}

	// Save config
	if err := saveCORSConfig(config); err != nil {
		fmt.Printf("❌ Error saving CORS config: %v\n", err)
		return
	}

	fmt.Println("ℹ️  CORS configuration updated")
	fmt.Println("ℹ️  Changes will be automatically reloaded within 30 seconds")
}

// cmdCORSDevMode manages development mode
func cmdCORSDevMode() {
	if len(os.Args) < 4 {
		fmt.Println("❌ Error: No subcommand provided")
		fmt.Println()
		fmt.Println("Usage:")
		fmt.Println("  nemesisbot cors dev-mode enable")
		fmt.Println("  nemesisbot cors dev-mode disable")
		fmt.Println("  nemesisbot cors dev-mode status")
		return
	}

	subcommand := os.Args[3]

	switch subcommand {
	case "enable":
		cmdCORSDevModeEnable()
	case "disable":
		cmdCORSDevModeDisable()
	case "status":
		cmdCORSDevModeStatus()
	default:
		fmt.Printf("Unknown dev-mode command: %s\n", subcommand)
	}
}

// cmdCORSDevModeEnable enables development mode
func cmdCORSDevModeEnable() {
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	if config.DevelopmentMode {
		fmt.Println("ℹ️  Development mode is already enabled")
		return
	}

	config.DevelopmentMode = true

	if err := saveCORSConfig(config); err != nil {
		fmt.Printf("❌ Error saving CORS config: %v\n", err)
		return
	}

	fmt.Println("✅ Development mode enabled")
	fmt.Println()
	fmt.Println("ℹ️  Localhost origins are now allowed:")
	fmt.Println("   - http://localhost:*")
	fmt.Println("   - http://127.0.0.1:*")
	fmt.Println()
	fmt.Println("⚠️  WARNING: Development mode should NOT be used in production!")
	fmt.Println("ℹ️  Changes will be automatically reloaded within 30 seconds")
}

// cmdCORSDevModeDisable disables development mode
func cmdCORSDevModeDisable() {
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	if !config.DevelopmentMode {
		fmt.Println("ℹ️  Development mode is already disabled")
		return
	}

	config.DevelopmentMode = false

	if err := saveCORSConfig(config); err != nil {
		fmt.Printf("❌ Error saving CORS config: %v\n", err)
		return
	}

	fmt.Println("✅ Development mode disabled")
	fmt.Println()
	fmt.Println("ℹ️  Now using strict whitelist mode")
	fmt.Println("ℹ️  Changes will be automatically reloaded within 30 seconds")
}

// cmdCORSDevModeStatus shows development mode status
func cmdCORSDevModeStatus() {
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	fmt.Println("\nDevelopment Mode Status:")
	fmt.Println("═" + strings.Repeat("═", 40))
	if config.DevelopmentMode {
		fmt.Println("  Status: 🔧 ENABLED")
		fmt.Println()
		fmt.Println("  Allowed:")
		fmt.Println("    ✅ http://localhost:*")
		fmt.Println("    ✅ http://127.0.0.1:*")
		fmt.Println()
		fmt.Println("  ⚠️  WARNING: Not safe for production!")
	} else {
		fmt.Println("  Status: 🏭 DISABLED")
		fmt.Println()
		fmt.Println("  Mode: Strict whitelist")
		fmt.Println("  Only origins in the whitelist are allowed")
	}
	fmt.Println("═" + strings.Repeat("═", 40))
}

// cmdCORSShow shows CORS configuration
func cmdCORSShow() {
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	homeDir, _ := path.ResolveHomeDir()
	configPath := filepath.Join(homeDir, "config", "cors.json")

	fmt.Println("\nCORS Configuration:")
	fmt.Println("═" + strings.Repeat("═", 60))
	fmt.Printf("Config file: %s\n", configPath)
	fmt.Println()
	fmt.Printf("Allowed origins: %d\n", len(config.AllowedOrigins))
	fmt.Printf("CDN domains: %d\n", len(config.AllowedCDNDomains))
	fmt.Printf("Development mode: %v\n", config.DevelopmentMode)
	fmt.Printf("Allow localhost: %v\n", config.AllowLocalhost)
	fmt.Printf("Allow no-origin: %v\n", config.AllowNoOrigin)
	fmt.Printf("Allow credentials: %v\n", config.AllowCredentials)
	fmt.Printf("Max age: %d seconds\n", config.MaxAge)
	fmt.Println("═" + strings.Repeat("═", 60))
}

// cmdCORSValidate validates if an origin is allowed
func cmdCORSValidate() {
	if len(os.Args) < 4 {
		fmt.Println("❌ Error: No origin provided")
		fmt.Println()
		fmt.Println("Usage: nemesisbot cors validate <origin>")
		fmt.Println()
		fmt.Println("Example:")
		fmt.Println("  nemesisbot cors validate https://example.com")
		return
	}

	origin := os.Args[3]

	// Load config
	config, err := loadCORSConfig()
	if err != nil {
		fmt.Printf("❌ Error loading CORS config: %v\n", err)
		return
	}

	// Validate origin
	allowed := validateOrigin(config, origin)

	fmt.Println("\nOrigin Validation Result:")
	fmt.Println("═" + strings.Repeat("═", 60))
	fmt.Printf("Origin: %s\n", origin)

	if allowed {
		fmt.Println("Status: ✅ ALLOWED")
		fmt.Println()
		fmt.Println("This origin will be accepted by the CORS policy")
	} else {
		fmt.Println("Status: ❌ NOT ALLOWED")
		fmt.Println()
		fmt.Println("This origin will be blocked by the CORS policy")
		fmt.Println()
		fmt.Println("To allow this origin:")
		fmt.Printf("  nemesisbot cors add %s\n", origin)
	}

	fmt.Println("═" + strings.Repeat("═", 60))
}

// validateOrigin checks if an origin is allowed
func validateOrigin(config *CORSConfig, origin string) bool {
	// Empty origin
	if origin == "" {
		return config.AllowNoOrigin
	}

	// Development mode
	if config.DevelopmentMode {
		if strings.HasPrefix(origin, "http://localhost:") ||
			strings.HasPrefix(origin, "http://127.0.0.1:") {
			return true
		}
	}

	// Allow localhost
	if config.AllowLocalhost {
		if strings.HasPrefix(origin, "http://localhost:") ||
			strings.HasPrefix(origin, "http://127.0.0.1:") {
			return true
		}
	}

	// Check allowed origins
	for _, allowed := range config.AllowedOrigins {
		if origin == allowed {
			return true
		}
	}

	// Check CDN domains
	originURL, err := url.Parse(origin)
	if err != nil {
		return false
	}

	host := originURL.Hostname()
	for _, cdn := range config.AllowedCDNDomains {
		if host == cdn || strings.HasSuffix(host, "."+cdn) {
			return true
		}
	}

	return false
}

// loadCORSConfig loads CORS configuration from file
func loadCORSConfig() (*CORSConfig, error) {
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		return nil, fmt.Errorf("failed to resolve home directory: %w", err)
	}

	configPath := filepath.Join(homeDir, "config", "cors.json")

	// Check if file exists
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		// Return default config
		return &CORSConfig{
			AllowedOrigins:    []string{},
			AllowedMethods:    []string{"GET", "POST"},
			AllowedHeaders:    []string{"Content-Type", "Authorization"},
			AllowCredentials:  true,
			MaxAge:            3600,
			AllowLocalhost:    true,
			DevelopmentMode:   false,
			AllowedCDNDomains: []string{},
			AllowNoOrigin:     true,
		}, nil
	}

	// Read file
	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read config file: %w", err)
	}

	// Parse JSON
	var config CORSConfig
	if err := json.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("failed to parse config: %w", err)
	}

	return &config, nil
}

// saveCORSConfig saves CORS configuration to file
func saveCORSConfig(config *CORSConfig) error {
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		return fmt.Errorf("failed to resolve home directory: %w", err)
	}

	configPath := filepath.Join(homeDir, "config", "cors.json")

	// Ensure directory exists
	configDir := filepath.Dir(configPath)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	// Marshal JSON
	data, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal config: %w", err)
	}

	// Write to temp file first
	tmpPath := configPath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write config: %w", err)
	}

	// Atomic rename
	if err := os.Rename(tmpPath, configPath); err != nil {
		os.Remove(tmpPath)
		return fmt.Errorf("failed to save config: %w", err)
	}

	return nil
}

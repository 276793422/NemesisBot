package command

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/security/scanner"
)

// cmdScanner dispatches scanner subcommands.
func cmdScanner() {
	if len(os.Args) < 4 {
		scannerHelp()
		return
	}

	subcmd := os.Args[3]
	switch subcmd {
	case "list":
		cmdScannerList()
	case "add":
		cmdScannerAdd()
	case "remove":
		cmdScannerRemove()
	case "enable":
		cmdScannerEnable()
	case "disable":
		cmdScannerDisable()
	case "info":
		cmdScannerInfo()
	case "download":
		cmdScannerDownload()
	case "check":
		cmdScannerCheck()
	case "install":
		cmdScannerInstall()
	case "test":
		cmdScannerTest()
	case "update":
		cmdScannerUpdate()
	default:
		fmt.Printf("Unknown scanner command: %s\n", subcmd)
		scannerHelp()
	}
}

// scannerHelp prints scanner command help.
func scannerHelp() {
	fmt.Println("\nScanner Commands:")
	fmt.Println("  list                              List all scanner engines and status")
	fmt.Println("  add <engine> [--url URL] [--path DIR] [--address ADDR]")
	fmt.Println("                                    Add or update a scanner engine configuration")
	fmt.Println("  remove <engine>                   Remove a scanner engine")
	fmt.Println("  enable <engine>                   Enable a scanner engine")
	fmt.Println("  disable <engine>                  Disable a scanner engine")
	fmt.Println("  check                             Check install and database status of engines")
	fmt.Println("  install [--dir DIR]               Install all pending enabled engines")
	fmt.Println("  info <engine>                     Show scanner engine information")
	fmt.Println("  download <engine> [--dir DIR]     Download scanner engine")
	fmt.Println("  test <engine> <path>              Test scan a file")
	fmt.Println("  update <engine>                   Update scanner virus database")
	fmt.Println()
	fmt.Println("Available engines: clamav")
	fmt.Println()
	fmt.Println("Typical Workflow:")
	fmt.Println("  Step 1:  nemesisbot security scanner enable clamav")
	fmt.Println("  Step 2:  nemesisbot security scanner check")
	fmt.Println("           # 预期: install=pending, db=missing")
	fmt.Println("  Step 3:  nemesisbot security scanner install")
	fmt.Println("           # 自动下载 ClamAV + 病毒库（使用默认官方地址）")
	fmt.Println("  Step 4:  nemesisbot security scanner check")
	fmt.Println("           # 预期: install=installed, db=ready")
	fmt.Println("  Step 5:  nemesisbot gateway")
	fmt.Println("           # 启动后自动加载扫描引擎")
	fmt.Println()
	fmt.Println("  若下载地址需要更换:")
	fmt.Println("  nemesisbot security scanner add clamav --url https://your-mirror/clamav.zip")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot security scanner enable clamav")
	fmt.Println("  nemesisbot security scanner check")
	fmt.Println("  nemesisbot security scanner install")
	fmt.Println("  nemesisbot security scanner test clamav /path/to/file.exe")
	fmt.Println("  nemesisbot security scanner list")
	fmt.Println("  nemesisbot security scanner update clamav")
	fmt.Println()
	fmt.Println("Default download URL:")
	fmt.Println("  https://www.clamav.net/downloads/production/clamav-1.5.2.win.x64.zip")
	fmt.Println()
}

// loadScannerConfig loads the scanner config from the configured path.
func loadScannerConfig() (*config.ScannerFullConfig, string, error) {
	configPath := GetScannerConfigPath()
	cfg, err := config.LoadScannerConfig(configPath)
	if err != nil {
		return nil, "", fmt.Errorf("failed to load scanner config: %w", err)
	}
	return cfg, configPath, nil
}

// cmdScannerList lists all scanner engines and their status.
func cmdScannerList() {
	cfg, _, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	enabledSet := make(map[string]bool)
	for _, name := range cfg.Enabled {
		enabledSet[name] = true
	}

	fmt.Println("\nScanner Engines:")
	fmt.Println(strings.Repeat("-", 70))

	if len(cfg.Engines) == 0 {
		fmt.Println("  No scanner engines configured.")
		fmt.Println("  Use 'nemesisbot security scanner add <engine>' to add one.")
		return
	}

	for name, rawCfg := range cfg.Engines {
		status := "disabled"
		if enabledSet[name] {
			status = "enabled"
		}

		// Parse config for additional info
		var summary string
		var parsed struct {
			Address   string               `json:"address"`
			ClamAVPath string              `json:"clamav_path"`
			State     config.EngineState   `json:"state"`
		}
		if json.Unmarshal(rawCfg, &parsed) == nil {
			parts := make([]string, 0, 3)
			if parsed.Address != "" {
				parts = append(parts, fmt.Sprintf("address=%s", parsed.Address))
			}
			if parsed.State.InstallStatus != "" {
				parts = append(parts, fmt.Sprintf("install=%s", parsed.State.InstallStatus))
			}
			if parsed.State.DBStatus != "" {
				parts = append(parts, fmt.Sprintf("db=%s", parsed.State.DBStatus))
			}
			summary = strings.Join(parts, ", ")
		}

		fmt.Printf("  %-15s  %-10s  %s\n", name, status, summary)
	}

	fmt.Println(strings.Repeat("-", 70))
	fmt.Printf("  Enabled order: %v\n", cfg.Enabled)
}

// cmdScannerAdd adds or updates a scanner engine configuration.
// If the engine already exists, only the specified flags are merged (existing values preserved).
func cmdScannerAdd() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner add <engine> [--url URL] [--path DIR] [--address ADDR]")
		os.Exit(1)
	}

	engineName := os.Args[4]
	if !isValidEngine(engineName) {
		fmt.Printf("Unknown engine: %s\nAvailable: %v\n", engineName, scanner.AvailableEngines())
		os.Exit(1)
	}

	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if cfg.Engines == nil {
		cfg.Engines = make(map[string]json.RawMessage)
	}

	// If engine already exists, merge new flags into existing config
	if existingRaw, exists := cfg.Engines[engineName]; exists {
		var existingCfg config.ClamAVEngineConfig
		if json.Unmarshal(existingRaw, &existingCfg) == nil {
			// Apply only explicitly provided flags
			if v := parseStringFlag("--url", ""); v != "" {
				existingCfg.URL = v
			}
			if v := parseStringFlag("--path", ""); v != "" {
				existingCfg.ClamAVPath = v
			}
			if v := parseStringFlag("--address", ""); v != "" {
				existingCfg.Address = v
			}
			if updated, err := json.Marshal(existingCfg); err == nil {
				cfg.Engines[engineName] = updated
			}
			if err := config.SaveScannerConfig(configPath, cfg); err != nil {
				fmt.Printf("Error saving config: %v\n", err)
				os.Exit(1)
			}
			fmt.Printf("Scanner engine '%s' updated.\n", engineName)
			return
		}
	}

	// New engine: use defaults + provided flags
	engineCfg := parseScannerFlags(engineName)

	rawCfg, err := json.Marshal(engineCfg)
	if err != nil {
		fmt.Printf("Error encoding config: %v\n", err)
		os.Exit(1)
	}

	cfg.Engines[engineName] = rawCfg

	if err := config.SaveScannerConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Scanner engine '%s' added.\n", engineName)
	fmt.Printf("Configuration saved to: %s\n", configPath)
	fmt.Printf("Use 'nemesisbot security scanner enable %s' to enable it.\n", engineName)
}

// cmdScannerRemove removes a scanner engine.
func cmdScannerRemove() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner remove <engine>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if _, ok := cfg.Engines[engineName]; !ok {
		fmt.Printf("Engine '%s' not found in configuration.\n", engineName)
		os.Exit(1)
	}

	// Remove from engines map
	delete(cfg.Engines, engineName)

	// Remove from enabled list
	var newEnabled []string
	for _, name := range cfg.Enabled {
		if name != engineName {
			newEnabled = append(newEnabled, name)
		}
	}
	cfg.Enabled = newEnabled

	if err := config.SaveScannerConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Scanner engine '%s' removed.\n", engineName)
}

// cmdScannerEnable enables a scanner engine.
func cmdScannerEnable() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner enable <engine>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if _, ok := cfg.Engines[engineName]; !ok {
		fmt.Printf("Engine '%s' not found. Add it first with 'scanner add %s'.\n", engineName, engineName)
		os.Exit(1)
	}

	// Check if already enabled
	for _, name := range cfg.Enabled {
		if name == engineName {
			fmt.Printf("Engine '%s' is already enabled.\n", engineName)
			return
		}
	}

	cfg.Enabled = append(cfg.Enabled, engineName)

	// Set install_status to pending if not already set
	rawCfg := cfg.Engines[engineName]
	var engineCfg config.ClamAVEngineConfig
	if json.Unmarshal(rawCfg, &engineCfg) == nil && engineCfg.State.InstallStatus == "" {
		engineCfg.State.InstallStatus = scanner.InstallStatusPending
		if updated, err := json.Marshal(engineCfg); err == nil {
			cfg.Engines[engineName] = updated
		}
	}

	if err := config.SaveScannerConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Scanner engine '%s' enabled.\n", engineName)
	fmt.Printf("Enabled engines: %v\n", cfg.Enabled)
}

// cmdScannerDisable disables a scanner engine.
func cmdScannerDisable() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner disable <engine>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	var newEnabled []string
	found := false
	for _, name := range cfg.Enabled {
		if name != engineName {
			newEnabled = append(newEnabled, name)
		} else {
			found = true
		}
	}

	if !found {
		fmt.Printf("Engine '%s' is not enabled.\n", engineName)
		return
	}

	cfg.Enabled = newEnabled

	if err := config.SaveScannerConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Scanner engine '%s' disabled.\n", engineName)
	fmt.Printf("Enabled engines: %v\n", cfg.Enabled)
}

// cmdScannerCheck checks the install and database status of all enabled engines.
func cmdScannerCheck() {
	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if len(cfg.Enabled) == 0 {
		fmt.Println("No engines enabled. Use 'scanner enable <engine>' first.")
		return
	}

	fmt.Println("\nScanner Engine Status:")
	fmt.Println(strings.Repeat("-", 70))

	changed := false

	for _, name := range cfg.Enabled {
		rawCfg, ok := cfg.Engines[name]
		if !ok {
			fmt.Printf("  %-15s  config missing\n", name)
			continue
		}

		engine, err := scanner.CreateEngine(name, rawCfg)
		if err != nil {
			fmt.Printf("  %-15s  error: %v\n", name, err)
			continue
		}

		// Type-assert to InstallableEngine
		ie, ok := engine.(scanner.InstallableEngine)
		if !ok {
			fmt.Printf("  %-15s  install=N/A (built-in engine)\n", name)
			continue
		}

		state := ie.GetEngineState()

		// Resolve install path: config path first, then system PATH
		resolvedPath := ""
		persistPath := ""
		if getter, ok := ie.(interface{ GetClamAVPath() string }); ok {
			resolvedPath = getter.GetClamAVPath()
		}

		if resolvedPath != "" {
			// Check executable at configured path
			targets := ie.TargetExecutables()
			found := false
			for _, exe := range targets {
				if _, err := os.Stat(filepath.Join(resolvedPath, exe)); err == nil {
					found = true
					break
				}
			}
			if found {
				state.InstallStatus = scanner.InstallStatusInstalled
				state.InstallError = ""
			} else {
				state.InstallStatus = scanner.InstallStatusFailed
				state.InstallError = "executable not found at " + resolvedPath
			}
		} else {
			// Config path empty → check system PATH
			if sysPath := lookupSystemClamAV(); sysPath != "" {
				resolvedPath = sysPath
				persistPath = sysPath
				state.InstallStatus = scanner.InstallStatusInstalled
				state.InstallError = ""
			} else if state.InstallStatus == "" {
				state.InstallStatus = scanner.InstallStatusPending
			}
		}

		// Check database status
		var engineCfg config.ClamAVEngineConfig
		if json.Unmarshal(rawCfg, &engineCfg) == nil {
			dataDir := engineCfg.DataDir
			if dataDir == "" && resolvedPath != "" {
				dataDir = resolvedPath
			}
			if dataDir != "" {
				// Manager appends "database" internally, so DB file is at {dataDir}/database/{filename}
				dbFile := filepath.Join(dataDir, "database", ie.DatabaseFileName())
				if _, err := os.Stat(dbFile); err == nil {
					state.DBStatus = scanner.DBStatusReady
				} else {
					state.DBStatus = scanner.DBStatusMissing
				}
			}
		}

		// Update config with new state and paths
		if updated := marshalEngineConfig(rawCfg, state, persistPath, ""); updated != nil {
			cfg.Engines[name] = updated
			changed = true
		}

		// Print status
		fmt.Printf("  %-15s  install=%-10s  db=%-10s", name, state.InstallStatus, state.DBStatus)
		if state.InstallError != "" {
			fmt.Printf("  error=%s", state.InstallError)
		}
		fmt.Println()
	}

	fmt.Println(strings.Repeat("-", 70))

	// Save if state changed
	if changed {
		if err := config.SaveScannerConfig(configPath, cfg); err != nil {
			fmt.Printf("Warning: failed to save updated state: %v\n", err)
		}
	}

	// Print recommendations
	fmt.Println("\nRecommendations:")
	for _, name := range cfg.Enabled {
		rawCfg, ok := cfg.Engines[name]
		if !ok {
			continue
		}
		var engineCfg config.ClamAVEngineConfig
		if json.Unmarshal(rawCfg, &engineCfg) != nil {
			continue
		}
		switch engineCfg.State.InstallStatus {
		case scanner.InstallStatusPending:
			fmt.Printf("  - Run 'scanner install' to install %s\n", name)
		case scanner.InstallStatusFailed:
			fmt.Printf("  - Re-run 'scanner install' to fix %s installation\n", name)
		case scanner.InstallStatusInstalled:
			if engineCfg.State.DBStatus == scanner.DBStatusMissing {
				fmt.Printf("  - Run 'scanner update %s' to download virus database\n", name)
			}
		}
	}
}

// cmdScannerInstall installs all pending enabled engines.
func cmdScannerInstall() {
	cfg, configPath, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if len(cfg.Enabled) == 0 {
		fmt.Println("No engines enabled. Use 'scanner enable <engine>' first.")
		return
	}

	// Parse --dir flag
	dir := parseStringFlag("--dir", "")
	if dir == "" {
		dir = resolveToolsDir()
	}

	fmt.Printf("Install directory: %s\n\n", dir)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Minute)
	defer cancel()

	changed := false

	for _, name := range cfg.Enabled {
		rawCfg, ok := cfg.Engines[name]
		if !ok {
			continue
		}

		// Check if already installed
		var engineCfg config.ClamAVEngineConfig
		if json.Unmarshal(rawCfg, &engineCfg) == nil {
			if engineCfg.State.InstallStatus == scanner.InstallStatusInstalled {
				fmt.Printf("  %-15s  already installed, skipping\n", name)
				continue
			}
		}

		engine, err := scanner.CreateEngine(name, rawCfg)
		if err != nil {
			fmt.Printf("  %-15s  error creating engine: %v\n", name, err)
			continue
		}

		// Type-assert to InstallableEngine
		ie, ok := engine.(scanner.InstallableEngine)
		if !ok {
			fmt.Printf("  %-15s  built-in engine, nothing to install\n", name)
			continue
		}

		state := ie.GetEngineState()
		state.LastInstallAttempt = time.Now().Format(time.RFC3339)

		detectedPath := ""

		// Step 1: Use configured path if set
		if engineCfg.ClamAVPath != "" {
			detectedPath = engineCfg.ClamAVPath
		}

		// Step 2: Check system PATH
		if detectedPath == "" {
			if sysPath := lookupSystemClamAV(); sysPath != "" {
				detectedPath = sysPath
				fmt.Printf("  %-15s  found in system PATH: %s\n", name, sysPath)
			}
		}

		// Step 3: Download if still not found
		if detectedPath == "" && engineCfg.URL != "" {
			fmt.Printf("  Installing %-15s  downloading... ", name)
			if err := engine.Download(ctx, dir); err != nil {
				state.InstallStatus = scanner.InstallStatusFailed
				state.InstallError = err.Error()
				fmt.Printf("FAILED: %v\n", err)
				if updated := marshalEngineConfig(rawCfg, state, "", ""); updated != nil {
					cfg.Engines[name] = updated
					changed = true
				}
				continue
			}
			detectedPath = ie.(interface{ GetClamAVPath() string }).GetClamAVPath()
		}
		// Safe: Download always sets the path internally, so this assertion
		// can't fail after a successful Download call.

		// Step 4: Validate
		if detectedPath == "" {
			state.InstallStatus = scanner.InstallStatusFailed
			state.InstallError = "no download URL, install path, or system installation found"
			fmt.Printf("  %-15s  FAILED: not found (no URL, path, or system PATH)\n", name)
		} else if err := engine.Validate(detectedPath); err != nil {
			state.InstallStatus = scanner.InstallStatusFailed
			state.InstallError = err.Error()
			fmt.Printf("  %-15s  FAILED: %v\n", name, err)
		} else {
			state.InstallStatus = scanner.InstallStatusInstalled
			state.InstallError = ""
			fmt.Printf("  %-15s  OK (path: %s)\n", name, detectedPath)
		}

		// Step 5: Set DataDir to {detectedPath} if empty.
		// Note: The ClamAV Manager appends "database" internally, so we don't add it here.
		dataDir := engineCfg.DataDir
		if dataDir == "" && detectedPath != "" {
			dataDir = detectedPath
		}

		// Step 6: Check/update virus database
		if state.InstallStatus == scanner.InstallStatusInstalled && dataDir != "" {
			// Manager appends "database" internally, so DB file is at {dataDir}/database/{filename}
			dbFile := filepath.Join(dataDir, "database", ie.DatabaseFileName())
			if _, err := os.Stat(dbFile); os.IsNotExist(err) {
				state.DBStatus = scanner.DBStatusMissing
				fmt.Printf("  %-15s  updating virus database...\n", name)

				// Update engine's internal DataDir before starting
				if setter, ok := engine.(interface{ SetDataDir(string) }); ok {
					setter.SetDataDir(dataDir)
				}

				startCtx, startCancel := context.WithTimeout(context.Background(), 120*time.Second)
				if startErr := engine.Start(startCtx); startErr == nil {
					// Poll for DB file to appear (updater downloads it)
					deadline := time.Now().Add(90 * time.Second)
					for time.Now().Before(deadline) {
						if _, statErr := os.Stat(dbFile); statErr == nil {
							state.DBStatus = scanner.DBStatusReady
							state.LastDBUpdate = time.Now().Format(time.RFC3339)
							fmt.Printf("  %-15s  database ready\n", name)
							break
						}
						time.Sleep(3 * time.Second)
					}
					engine.Stop()
					startCancel()
				} else {
					startCancel()
					fmt.Printf("  %-15s  database update skipped (engine start failed: %v)\n", name, startErr)
					fmt.Printf("  %-15s  run 'scanner update %s' to update database later\n", name, name)
				}
			} else {
				state.DBStatus = scanner.DBStatusReady
			}
		}

		// Step 7: Persist ClamAVPath, DataDir, and State
		if updated := marshalEngineConfig(rawCfg, state, detectedPath, dataDir); updated != nil {
			cfg.Engines[name] = updated
			changed = true
		}
	}

	// Save config
	if changed {
		if err := config.SaveScannerConfig(configPath, cfg); err != nil {
			fmt.Printf("\nWarning: failed to save config: %v\n", err)
		}
	}

	fmt.Println()
}

// marshalEngineConfig re-serializes engine config with updated state and paths.
// Empty clamavPath/dataDir means "don't change the existing value".
func marshalEngineConfig(rawCfg json.RawMessage, state *config.EngineState, clamavPath, dataDir string) json.RawMessage {
	var engineCfg config.ClamAVEngineConfig
	if err := json.Unmarshal(rawCfg, &engineCfg); err != nil {
		return nil
	}
	engineCfg.State = *state
	if clamavPath != "" {
		engineCfg.ClamAVPath = clamavPath
	}
	if dataDir != "" {
		engineCfg.DataDir = dataDir
	}
	updated, err := json.Marshal(engineCfg)
	if err != nil {
		return nil
	}
	return updated
}

// cmdScannerInfo shows scanner engine information.
func cmdScannerInfo() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner info <engine>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, _, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	rawCfg, ok := cfg.Engines[engineName]
	if !ok {
		fmt.Printf("Engine '%s' not found in configuration.\n", engineName)
		os.Exit(1)
	}

	// Create engine instance
	engine, err := scanner.CreateEngine(engineName, rawCfg)
	if err != nil {
		fmt.Printf("Error creating engine: %v\n", err)
		os.Exit(1)
	}

	// Get info
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	info, err := engine.GetInfo(ctx)
	if err != nil {
		fmt.Printf("Error getting engine info: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("\nEngine: %s\n", info.Name)
	fmt.Println(strings.Repeat("-", 40))
	if info.Version != "" {
		fmt.Printf("  Version:   %s\n", info.Version)
	}
	if info.Address != "" {
		fmt.Printf("  Address:   %s\n", info.Address)
	}
	fmt.Printf("  Ready:     %v\n", info.Ready)
	if info.StartTime != "" {
		fmt.Printf("  Started:   %s\n", info.StartTime)
	}

	// Show parsed config
	var parsed map[string]interface{}
	if json.Unmarshal(rawCfg, &parsed) == nil {
		fmt.Println("\nConfiguration:")
		for k, v := range parsed {
			fmt.Printf("  %-18s %v\n", k+":", v)
		}
	}
}

// cmdScannerDownload downloads a scanner engine.
func cmdScannerDownload() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner download <engine> [--dir DIR]")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, _, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	rawCfg, ok := cfg.Engines[engineName]
	if !ok {
		fmt.Printf("Engine '%s' not found in configuration.\n", engineName)
		os.Exit(1)
	}

	engine, err := scanner.CreateEngine(engineName, rawCfg)
	if err != nil {
		fmt.Printf("Error creating engine: %v\n", err)
		os.Exit(1)
	}

	// Parse --dir flag
	dir := parseStringFlag("--dir", ".")
	if dir == "" {
		dir = "."
	}

	fmt.Printf("Downloading %s to %s...\n", engineName, dir)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()

	if err := engine.Download(ctx, dir); err != nil {
		fmt.Printf("Download failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Download complete.\n")

	// Validate
	if ie, ok := engine.(scanner.InstallableEngine); ok {
		validateDir := dir
		if getter, ok := ie.(interface{ GetClamAVPath() string }); ok {
			if p := getter.GetClamAVPath(); p != "" {
				validateDir = p
			}
		}
		if err := engine.Validate(validateDir); err != nil {
			fmt.Printf("Validation warning: %v\n", err)
		} else {
			fmt.Printf("Validation passed.\n")
		}
	} else if err := engine.Validate(dir); err != nil {
		fmt.Printf("Validation warning: %v\n", err)
	} else {
		fmt.Printf("Validation passed.\n")
	}
}

// cmdScannerTest tests scanning a file.
func cmdScannerTest() {
	if len(os.Args) < 6 {
		fmt.Println("Usage: nemesisbot security scanner test <engine> <path>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	filePath := os.Args[5]
	cfg, _, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	rawCfg, ok := cfg.Engines[engineName]
	if !ok {
		fmt.Printf("Engine '%s' not found in configuration.\n", engineName)
		os.Exit(1)
	}

	engine, err := scanner.CreateEngine(engineName, rawCfg)
	if err != nil {
		fmt.Printf("Error creating engine: %v\n", err)
		os.Exit(1)
	}

	// Start the engine for testing
	ctx := context.Background()
	if err := engine.Start(ctx); err != nil {
		fmt.Printf("Warning: Failed to start engine: %v\n", err)
		fmt.Println("Attempting scan anyway (may use existing daemon)...")
	}
	defer engine.Stop()

	// Wait briefly for engine to be ready
	time.Sleep(2 * time.Second)

	if !engine.IsReady() {
		fmt.Printf("Engine '%s' is not ready. Make sure the daemon is running.\n", engineName)
		os.Exit(1)
	}

	fmt.Printf("Scanning: %s\n", filePath)
	result, err := engine.ScanFile(ctx, filePath)
	if err != nil {
		fmt.Printf("Scan error: %v\n", err)
		os.Exit(1)
	}

	fmt.Println(strings.Repeat("-", 40))
	fmt.Printf("  Engine:   %s\n", result.Engine)
	fmt.Printf("  Path:     %s\n", result.Path)
	if result.Infected {
		fmt.Printf("  Status:   INFECTED\n")
		fmt.Printf("  Virus:    %s\n", result.Virus)
	} else {
		fmt.Printf("  Status:   CLEAN\n")
	}
	if result.Raw != "" {
		fmt.Printf("  Details:  %s\n", result.Raw)
	}
}

// cmdScannerUpdate updates the scanner virus database.
func cmdScannerUpdate() {
	if len(os.Args) < 5 {
		fmt.Println("Usage: nemesisbot security scanner update <engine>")
		os.Exit(1)
	}

	engineName := os.Args[4]
	cfg, _, err := loadScannerConfig()
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	rawCfg, ok := cfg.Engines[engineName]
	if !ok {
		fmt.Printf("Engine '%s' not found in configuration.\n", engineName)
		os.Exit(1)
	}

	engine, err := scanner.CreateEngine(engineName, rawCfg)
	if err != nil {
		fmt.Printf("Error creating engine: %v\n", err)
		os.Exit(1)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()

	fmt.Printf("Updating virus database for %s...\n", engineName)
	if err := engine.UpdateDatabase(ctx); err != nil {
		fmt.Printf("Update failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Virus database update complete.\n")

	// Show database status
	status, err := engine.GetDatabaseStatus(ctx)
	if err == nil {
		fmt.Printf("  Available:  %v\n", status.Available)
		if !status.LastUpdate.IsZero() {
			fmt.Printf("  Last update: %s\n", status.LastUpdate.Format(time.RFC3339))
		}
	}
}

// parseScannerFlags extracts scanner configuration from command-line flags.
func parseScannerFlags(engineName string) *config.ClamAVEngineConfig {
	cfg := &config.ClamAVEngineConfig{
		Address:        "127.0.0.1:3310",
		ScanOnWrite:    true,
		ScanOnDownload: false,
		ScanOnExec:     true,
		MaxFileSize:    52428800,
		UpdateInterval: "24h",
		SkipExtensions: []string{".txt", ".md", ".json", ".yaml", ".yml", ".toml", ".log", ".css", ".html"},
	}

	if v := parseStringFlag("--url", ""); v != "" {
		cfg.URL = v
	}
	if v := parseStringFlag("--path", ""); v != "" {
		cfg.ClamAVPath = v
	}
	if v := parseStringFlag("--address", ""); v != "" {
		cfg.Address = v
	}

	return cfg
}

// parseStringFlag finds a flag value in os.Args.
func parseStringFlag(flagName, defaultValue string) string {
	for i, arg := range os.Args {
		if arg == flagName && i+1 < len(os.Args) {
			return os.Args[i+1]
		}
		if strings.HasPrefix(arg, flagName+"=") {
			return strings.TrimPrefix(arg, flagName+"=")
		}
	}
	return defaultValue
}

// isValidEngine checks if an engine name is recognized.
func isValidEngine(name string) bool {
	for _, e := range scanner.AvailableEngines() {
		if e == name {
			return true
		}
	}
	return false
}

// resolveToolsDir returns the default tools installation directory:
// {scanner_config_parent}/tools/
// Since the scanner config is at {nemesisbot_home}/workspace/config/config.scanner.json,
// this resolves to {nemesisbot_home}/workspace/tools/.
func resolveToolsDir() string {
	configPath := GetScannerConfigPath()
	configDir := filepath.Dir(configPath)
	workspaceDir := filepath.Dir(configDir)
	return filepath.Join(workspaceDir, "tools")
}

// lookupSystemClamAV checks if clamd is available in the system PATH.
// Returns the directory containing the executable, or "" if not found.
func lookupSystemClamAV() string {
	if exePath, err := exec.LookPath("clamd"); err == nil {
		return filepath.Dir(exePath)
	}
	return ""
}

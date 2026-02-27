package command

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdChannelExternal handles external channel specific commands
func CmdChannelExternal(cfg *config.Config) {
	if len(os.Args) < 4 {
		ExternalHelp()
		return
	}

	subcommand := os.Args[3]

	switch subcommand {
	case "setup":
		cmdExternalSetup(cfg)
	case "config":
		cmdExternalConfig(cfg)
	case "test":
		cmdExternalTest(cfg)
	case "set":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot channel external set <parameter> <value>")
			fmt.Println()
			fmt.Println("Parameters:")
			fmt.Println("  input     Set input executable path")
			fmt.Println("  output    Set output executable path")
			fmt.Println("  chat_id   Set chat ID")
			fmt.Println("  sync      Enable/disable web sync (true/false)")
			fmt.Println("  session   Set web session ID")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot channel external set input C:\\Tools\\input.exe")
			fmt.Println("  nemesisbot channel external set output C:\\Tools\\output.exe")
			fmt.Println("  nemesisbot channel external set chat_id external:myapp")
			fmt.Println("  nemesisbot channel external set sync true")
			fmt.Println("  nemesisbot channel external set session abc123")
			os.Exit(1)
		}
		cmdExternalSet(cfg, os.Args[4], os.Args[5])
	case "get":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot channel external get <parameter>")
			fmt.Println()
			fmt.Println("Parameters:")
			fmt.Println("  input     Get input executable path")
			fmt.Println("  output    Get output executable path")
			fmt.Println("  chat_id   Get chat ID")
			fmt.Println("  sync      Get web sync setting")
			fmt.Println("  session   Get web session ID")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot channel external get input")
			fmt.Println("  nemesisbot channel external get sync")
			os.Exit(1)
		}
		cmdExternalGet(cfg, os.Args[4])
	default:
		fmt.Printf("Unknown external command: %s\n", subcommand)
		ExternalHelp()
	}
}

// ExternalHelp prints external channel help
func ExternalHelp() {
	fmt.Println("External Channel Commands")
	fmt.Println("=======================")
	fmt.Println()
	fmt.Println("The external channel allows you to connect custom input/output programs")
	fmt.Println("to NemesisBot. Input program reads from stdin and outputs to stdout.")
	fmt.Println("Output program receives AI responses via stdin.")
	fmt.Println()
	fmt.Println("Usage: nemesisbot channel external <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  setup    Interactive setup for external channel")
	fmt.Println("  config   Show current external channel configuration")
	fmt.Println("  test     Test external programs")
	fmt.Println("  set      Set a specific configuration parameter")
	fmt.Println("  get      Get a specific configuration parameter")
	fmt.Println()
	fmt.Println("Set command usage:")
	fmt.Println("  nemesisbot channel external set <parameter> <value>")
	fmt.Println()
	fmt.Println("  Parameters:")
	fmt.Println("    input     - Set input executable path")
	fmt.Println("    output    - Set output executable path")
	fmt.Println("    chat_id   - Set chat ID")
	fmt.Println("    sync      - Enable/disable web sync (true/false)")
	fmt.Println("    session   - Set web session ID")
	fmt.Println()
	fmt.Println("Get command usage:")
	fmt.Println("  nemesisbot channel external get <parameter>")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # Interactive setup")
	fmt.Println("  nemesisbot channel external setup")
	fmt.Println()
	fmt.Println("  # Set parameters directly")
	fmt.Println("  nemesisbot channel external set input C:\\Tools\\input.exe")
	fmt.Println("  nemesisbot channel external set output C:\\Tools\\output.exe")
	fmt.Println("  nemesisbot channel external set chat_id external:myapp")
	fmt.Println("  nemesisbot channel external set sync true")
	fmt.Println("  nemesisbot channel external set session abc123")
	fmt.Println()
	fmt.Println("  # Get parameters")
	fmt.Println("  nemesisbot channel external get input")
	fmt.Println("  nemesisbot channel external get sync")
	fmt.Println()
	fmt.Println("Setup Requirements:")
	fmt.Println("  - Input program: Reads from stdin, outputs to stdout")
	fmt.Println("  - Output program: Reads from stdin (AI responses)")
	fmt.Println()
	fmt.Println("Example Workflow:")
	fmt.Println("  1. nemesisbot channel external set input C:\\Tools\\input.exe")
	fmt.Println("  2. nemesisbot channel external set output C:\\Tools\\output.exe")
	fmt.Println("  3. nemesisbot channel enable external")
	fmt.Println("  4. nemesisbot gateway")
	fmt.Println()
	fmt.Println("Note: Use absolute paths for executables to avoid issues")
}

// cmdExternalSetup interactively sets up external channel
func cmdExternalSetup(cfg *config.Config) {
	configPath := GetConfigPath()

	fmt.Println("======================================")
	fmt.Println("  External Channel Setup")
	fmt.Println("======================================")
	fmt.Println()
	fmt.Println("This will help you configure the external channel to connect")
	fmt.Println("custom input/output programs to NemesisBot.")
	fmt.Println()
	fmt.Println("Input Program: Reads user input from stdin and outputs to stdout")
	fmt.Println("Output Program: Receives AI responses via stdin")
	fmt.Println()

	reader := bufio.NewReader(os.Stdin)

	// Get input exe path
	fmt.Print("Enter path to input executable (or press Enter to skip): ")
	inputEXE, _ := reader.ReadString('\n')
	inputEXE = strings.TrimSpace(inputEXE)

	if inputEXE != "" {
		// Validate path
		if !filepath.IsAbs(inputEXE) {
			abs, err := filepath.Abs(inputEXE)
			if err == nil {
				inputEXE = abs
				fmt.Printf("→ Using absolute path: %s\n", inputEXE)
			}
		}

		// Check if file exists
		if _, err := os.Stat(inputEXE); err != nil {
			fmt.Printf("⚠️  Warning: File not found: %s\n", inputEXE)
			fmt.Print("Continue anyway? (y/N): ")
			resp, _ := reader.ReadString('\n')
			if strings.ToLower(strings.TrimSpace(resp)) != "y" {
				fmt.Println("Setup cancelled")
				return
			}
		} else {
			fmt.Printf("✅ Input executable found: %s\n", inputEXE)
		}

		cfg.Channels.External.InputEXE = inputEXE
	}

	// Get output exe path
	fmt.Print("\nEnter path to output executable (or press Enter to skip): ")
	outputEXE, _ := reader.ReadString('\n')
	outputEXE = strings.TrimSpace(outputEXE)

	if outputEXE != "" {
		// Validate path
		if !filepath.IsAbs(outputEXE) {
			abs, err := filepath.Abs(outputEXE)
			if err == nil {
				outputEXE = abs
				fmt.Printf("→ Using absolute path: %s\n", outputEXE)
			}
		}

		// Check if file exists
		if _, err := os.Stat(outputEXE); err != nil {
			fmt.Printf("⚠️  Warning: File not found: %s\n", outputEXE)
			fmt.Print("Continue anyway? (y/N): ")
			resp, _ := reader.ReadString('\n')
			if strings.ToLower(strings.TrimSpace(resp)) != "y" {
				fmt.Println("Setup cancelled")
				return
			}
		} else {
			fmt.Printf("✅ Output executable found: %s\n", outputEXE)
		}

		cfg.Channels.External.OutputEXE = outputEXE
	}

	// Ask for chat ID
	fmt.Println("\nChat ID Configuration")
	fmt.Println("----------------------")
	fmt.Println("The chat ID identifies this external channel session.")
	fmt.Println("Format: 'external:<name>' (e.g., 'external:main', 'external:speech')")
	fmt.Printf("\nCurrent chat ID: %s\n", cfg.Channels.External.ChatID)
	fmt.Print("Enter new chat ID (or press Enter to keep current): ")
	chatID, _ := reader.ReadString('\n')
	chatID = strings.TrimSpace(chatID)

	if chatID != "" {
		if !strings.HasPrefix(chatID, "external:") {
			chatID = "external:" + chatID
		}
		cfg.Channels.External.ChatID = chatID
		fmt.Printf("✅ Chat ID set to: %s\n", chatID)
	}

	// Ask about web sync
	fmt.Println("\nWeb Synchronization")
	fmt.Println("---------------------")
	fmt.Println("When enabled, all messages will also appear in the Web interface.")
	fmt.Printf("Current setting: %v\n", cfg.Channels.External.SyncToWeb)
	fmt.Print("Enable web sync? (Y/n): ")
	syncResp, _ := reader.ReadString('\n')
	syncResp = strings.ToLower(strings.TrimSpace(syncResp))

	if syncResp == "" || syncResp == "y" || syncResp == "yes" {
		cfg.Channels.External.SyncToWeb = true
		fmt.Println("✅ Web sync enabled")
	} else {
		cfg.Channels.External.SyncToWeb = false
		fmt.Println("❌ Web sync disabled")
	}

	// Save configuration
	fmt.Println("\nSaving configuration...")
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\n======================================")
	fmt.Println("✅ External channel configured successfully!")
	fmt.Println("======================================")
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Println("  1. Enable the channel:")
	fmt.Println("     nemesisbot channel enable external")
	fmt.Println()
	fmt.Println("  2. Start the gateway:")
	fmt.Println("     nemesisbot gateway")
	fmt.Println()
	fmt.Println("  3. View configuration:")
	fmt.Println("     nemesisbot channel external config")
}

// cmdExternalConfig shows current external channel configuration
func cmdExternalConfig(cfg *config.Config) {
	fmt.Println("External Channel Configuration")
	fmt.Println("===============================")
	fmt.Println()

	fmt.Printf("Enabled:         ")
	if cfg.Channels.External.Enabled {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}

	fmt.Printf("Input EXE:       %s\n", formatFilePath(cfg.Channels.External.InputEXE))
	fmt.Printf("Output EXE:      %s\n", formatFilePath(cfg.Channels.External.OutputEXE))
	fmt.Printf("Chat ID:         %s\n", cfg.Channels.External.ChatID)
	fmt.Printf("Sync to Web:     ")
	if cfg.Channels.External.SyncToWeb {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}
	fmt.Printf("Web Session ID:  %s\n", formatFilePath(cfg.Channels.External.WebSessionID))
	fmt.Printf("Allow From:      %v\n", cfg.Channels.External.AllowFrom)

	fmt.Println()
	if cfg.Channels.External.InputEXE == "" || cfg.Channels.External.OutputEXE == "" {
		fmt.Println("⚠️  Warning: External channel is not fully configured")
		fmt.Println()
		fmt.Println("To complete setup, run:")
		fmt.Println("  nemesisbot channel external setup")
	} else {
		fmt.Println("✅ External channel is ready to use")
		fmt.Println()
		fmt.Println("To enable, run:")
		fmt.Println("  nemesisbot channel enable external")
	}
}

// cmdExternalTest tests external programs
func cmdExternalTest(cfg *config.Config) {
	fmt.Println("Testing External Programs")
	fmt.Println("=========================")
	fmt.Println()

	if cfg.Channels.External.InputEXE == "" {
		fmt.Println("❌ No input program configured")
		fmt.Println("Run: nemesisbot channel external setup")
		return
	}

	if cfg.Channels.External.OutputEXE == "" {
		fmt.Println("❌ No output program configured")
		fmt.Println("Run: nemesisbot channel external setup")
		return
	}

	// Test input program
	fmt.Println("Testing input program...")
	fmt.Printf("Program: %s\n", cfg.Channels.External.InputEXE)
	fmt.Println("Expected behavior: Should read from stdin and output to stdout")
	fmt.Print("Do you want to test the input program? (y/N): ")

	reader := bufio.NewReader(os.Stdin)
	resp, _ := reader.ReadString('\n')
	resp = strings.ToLower(strings.TrimSpace(resp))

	if resp == "y" || resp == "yes" {
		testInputProgram(cfg.Channels.External.InputEXE)
	}

	// Test output program
	fmt.Println("\nTesting output program...")
	fmt.Printf("Program: %s\n", cfg.Channels.External.OutputEXE)
	fmt.Println("Expected behavior: Should read from stdin and process the input")
	fmt.Print("Do you want to test the output program? (y/N): ")

	resp, _ = reader.ReadString('\n')
	resp = strings.ToLower(strings.TrimSpace(resp))

	if resp == "y" || resp == "yes" {
		testOutputProgram(cfg.Channels.External.OutputEXE)
	}

	fmt.Println()
	fmt.Println("Testing complete!")
}

// testInputProgram tests the input program
func testInputProgram(path string) {
	fmt.Println("\n--- Input Program Test ---")
	fmt.Println("Starting input program...")
	fmt.Println("Type your message and press Enter (Ctrl+C to finish):")

	cmd := exec.Command(path)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		fmt.Printf("❌ Error getting stdin: %v\n", err)
		return
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		fmt.Printf("❌ Error getting stdout: %v\n", err)
		return
	}

	if err := cmd.Start(); err != nil {
		fmt.Printf("❌ Error starting program: %v\n", err)
		return
	}

	// Start reader goroutine
	go func() {
		scanner := bufio.NewScanner(stdout)
		for scanner.Scan() {
			fmt.Printf("← Program output: %s\n", scanner.Text())
		}
	}()

	// Write to stdin
	fmt.Print("→ Send: ")
	scanner := bufio.NewScanner(os.Stdin)
	if scanner.Scan() {
		input := scanner.Text() + "\n"
		stdin.Write([]byte(input))
	}

	stdin.Close()
	cmd.Wait()

	fmt.Println("✅ Input program test finished")
}

// testOutputProgram tests the output program
func testOutputProgram(path string) {
	fmt.Println("\n--- Output Program Test ---")
	fmt.Println("Starting output program...")
	fmt.Println("Sending test message (press Ctrl+C to finish):")

	cmd := exec.Command(path)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		fmt.Printf("❌ Error getting stdin: %v\n", err)
		return
	}

	if err := cmd.Start(); err != nil {
		fmt.Printf("❌ Error starting program: %v\n", err)
		return
	}

	// Send test message
	testMessage := "Test message from NemesisBot external channel\n"
	fmt.Printf("→ Sending to program: %s", testMessage)
	stdin.Write([]byte(testMessage))
	stdin.Close()

	// Wait a bit for program to process
	fmt.Println("✅ Test message sent")
	fmt.Println("✅ Output program test finished")

	cmd.Wait()
}

// formatFilePath formats file path for display
func formatFilePath(path string) string {
	if path == "" {
		return "(not set)"
	}

	// Shorten path if too long
	maxLen := 50
	if runtime.GOOS == "windows" {
		maxLen = 60
	}

	if len(path) > maxLen {
		if len(path) > 10 {
			return "..." + path[len(path)-maxLen+3:]
		}
	}

	return path
}

// cmdExternalSet sets a specific external channel parameter
func cmdExternalSet(cfg *config.Config, param, value string) {
	configPath := GetConfigPath()

	var updated bool
	var requiresRestart bool

	switch param {
	case "input":
		// Validate path
		if _, err := os.Stat(value); err != nil {
			fmt.Printf("⚠️  Warning: File not found: %s\n", value)
			fmt.Print("Continue anyway? (y/N): ")
			reader := bufio.NewReader(os.Stdin)
			resp, _ := reader.ReadString('\n')
			if strings.ToLower(strings.TrimSpace(resp)) != "y" {
				fmt.Println("❌ Operation cancelled")
				return
			}
		}
		cfg.Channels.External.InputEXE = value
		updated = true
		requiresRestart = true
		fmt.Printf("✅ Input executable set to: %s\n", value)

	case "output":
		// Validate path
		if _, err := os.Stat(value); err != nil {
			fmt.Printf("⚠️  Warning: File not found: %s\n", value)
			fmt.Print("Continue anyway? (y/N): ")
			reader := bufio.NewReader(os.Stdin)
			resp, _ := reader.ReadString('\n')
			if strings.ToLower(strings.TrimSpace(resp)) != "y" {
				fmt.Println("❌ Operation cancelled")
				return
			}
		}
		cfg.Channels.External.OutputEXE = value
		updated = true
		requiresRestart = true
		fmt.Printf("✅ Output executable set to: %s\n", value)

	case "chat_id", "chatid", "chat-id":
		// Add prefix if not present
		if !strings.HasPrefix(value, "external:") {
			value = "external:" + value
		}
		cfg.Channels.External.ChatID = value
		updated = true
		requiresRestart = true
		fmt.Printf("✅ Chat ID set to: %s\n", value)

	case "sync":
		// Parse boolean value
		boolVal := strings.ToLower(value)
		if boolVal == "true" || boolVal == "yes" || boolVal == "y" || boolVal == "1" || boolVal == "on" {
			cfg.Channels.External.SyncToWeb = true
			updated = true
			requiresRestart = false
			fmt.Println("✅ Web sync enabled")
		} else if boolVal == "false" || boolVal == "no" || boolVal == "n" || boolVal == "0" || boolVal == "off" {
			cfg.Channels.External.SyncToWeb = false
			updated = true
			requiresRestart = false
			fmt.Println("❌ Web sync disabled")
		} else {
			fmt.Printf("❌ Invalid value for sync: %s\n", value)
			fmt.Println("Valid values: true, false, yes, no, y, n, 1, 0, on, off")
			return
		}

	case "session":
		cfg.Channels.External.WebSessionID = value
		updated = true
		requiresRestart = false
		if value == "" {
			fmt.Println("✅ Web session ID cleared (will broadcast to all sessions)")
		} else {
			fmt.Printf("✅ Web session ID set to: %s\n", value)
		}

	default:
		fmt.Printf("❌ Unknown parameter: %s\n", param)
		fmt.Println()
		fmt.Println("Valid parameters:")
		fmt.Println("  input     - Set input executable path")
		fmt.Println("  output    - Set output executable path")
		fmt.Println("  chat_id   - Set chat ID")
		fmt.Println("  sync      - Enable/disable web sync")
		fmt.Println("  session   - Set web session ID")
		return
	}

	if !updated {
		return
	}

	// Save configuration
	fmt.Println("\nSaving configuration...")
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✅ Configuration saved successfully")

	if requiresRestart {
		fmt.Println()
		fmt.Println("⚠️  Restart gateway for changes to take effect:")
		fmt.Println("   nemesisbot gateway")
	}
}

// cmdExternalGet gets a specific external channel parameter
func cmdExternalGet(cfg *config.Config, param string) {
	switch param {
	case "input":
		value := cfg.Channels.External.InputEXE
		if value == "" {
			fmt.Println("Input executable: (not set)")
		} else {
			fmt.Printf("Input executable: %s\n", value)
		}

	case "output":
		value := cfg.Channels.External.OutputEXE
		if value == "" {
			fmt.Println("Output executable: (not set)")
		} else {
			fmt.Printf("Output executable: %s\n", value)
		}

	case "chat_id", "chatid", "chat-id":
		fmt.Printf("Chat ID: %s\n", cfg.Channels.External.ChatID)

	case "sync":
		if cfg.Channels.External.SyncToWeb {
			fmt.Println("Web sync: enabled (true)")
		} else {
			fmt.Println("Web sync: disabled (false)")
		}

	case "session":
		value := cfg.Channels.External.WebSessionID
		if value == "" {
			fmt.Println("Web session ID: (not set - will broadcast to all sessions)")
		} else {
			fmt.Printf("Web session ID: %s\n", value)
		}

	default:
		fmt.Printf("❌ Unknown parameter: %s\n", param)
		fmt.Println()
		fmt.Println("Valid parameters:")
		fmt.Println("  input     - Get input executable path")
		fmt.Println("  output    - Get output executable path")
		fmt.Println("  chat_id   - Get chat ID")
		fmt.Println("  sync      - Get web sync setting")
		fmt.Println("  session   - Get web session ID")
	}
}

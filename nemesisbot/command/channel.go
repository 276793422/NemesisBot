package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdChannel manages communication channels
func CmdChannel() {
	if len(os.Args) < 3 {
		ChannelHelp()
		return
	}

	subcommand := os.Args[2]

	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	switch subcommand {
	case "list":
		cmdChannelList(cfg)
	case "enable":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot channel enable <channel-name>")
			fmt.Println()
			fmt.Println("Available channels: web, telegram, discord, whatsapp, feishu, slack, line, onebot, qq, dingtalk, maixcam, external")
			os.Exit(1)
		}
		cmdChannelEnable(cfg, os.Args[3])
	case "disable":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot channel disable <channel-name>")
			fmt.Println()
			fmt.Println("Available channels: web, telegram, discord, whatsapp, feishu, slack, line, onebot, qq, dingtalk, maixcam, external")
			os.Exit(1)
		}
		cmdChannelDisable(cfg, os.Args[3])
	case "status":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot channel status <channel-name>")
			fmt.Println()
			fmt.Println("Available channels: web, telegram, discord, whatsapp, feishu, slack, line, onebot, qq, dingtalk, maixcam, external")
			os.Exit(1)
		}
		cmdChannelStatus(cfg, os.Args[3])
	case "web":
		// Web channel specific commands
		CmdChannelWeb(cfg)
	case "external":
		// External channel specific commands
		CmdChannelExternal(cfg)
	default:
		fmt.Printf("Unknown channel command: %s\n", subcommand)
		ChannelHelp()
	}
}

// ChannelHelp prints channel command help
func ChannelHelp() {
	fmt.Println("Manage NemesisBot communication channels")
	fmt.Println()
	fmt.Println("Usage: nemesisbot channel <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  list              List all channels and their status")
	fmt.Println("  enable <name>     Enable a channel")
	fmt.Println("  disable <name>    Disable a channel")
	fmt.Println("  status <name>     Show detailed status of a channel")
	fmt.Println("  web               Web channel specific commands")
	fmt.Println("  external          External channel specific commands")
	fmt.Println()
	fmt.Println("Available channels:")
	fmt.Println("  web       Web chat interface (WebSocket)")
	fmt.Println("  telegram  Telegram bot")
	fmt.Println("  discord   Discord bot")
	fmt.Println("  whatsapp  WhatsApp bridge")
	fmt.Println("  feishu    Feishu bot")
	fmt.Println("  slack     Slack bot")
	fmt.Println("  line      LINE bot")
	fmt.Println("  onebot    OneBot protocol")
	fmt.Println("  qq        QQ bot")
	fmt.Println("  dingtalk  DingTalk bot")
	fmt.Println("  maixcam   MaixCam device")
	fmt.Println("  external  External program channel (stdin/stdout)")
	fmt.Println()
	fmt.Println("Web subcommands:")
	fmt.Println("  nemesisbot channel web auth     Set authentication token (secure)")
	fmt.Println("  nemesisbot channel web status   Show web configuration")
	fmt.Println("  nemesisbot channel web clear    Remove authentication token")
	fmt.Println("  nemesisbot channel web config   Show detailed configuration")
	fmt.Println()
	fmt.Println("External subcommands:")
	fmt.Println("  nemesisbot channel external setup   Interactive setup for external channel")
	fmt.Println("  nemesisbot channel external config  Show external channel configuration")
	fmt.Println("  nemesisbot channel external test    Test external programs")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot channel list")
	fmt.Println("  nemesisbot channel enable web")
	fmt.Println("  nemesisbot channel enable external")
	fmt.Println("  nemesisbot channel disable telegram")
	fmt.Println("  nemesisbot channel status external")
	fmt.Println("  nemesisbot channel external setup")
}

func cmdChannelList(cfg *config.Config) {
	fmt.Println("NemesisBot Channel Status")
	fmt.Println("========================")
	fmt.Println()

	channels := []struct {
		name    string
		enabled bool
	}{
		{"web", cfg.Channels.Web.Enabled},
		{"telegram", cfg.Channels.Telegram.Enabled},
		{"discord", cfg.Channels.Discord.Enabled},
		{"whatsapp", cfg.Channels.WhatsApp.Enabled},
		{"feishu", cfg.Channels.Feishu.Enabled},
		{"slack", cfg.Channels.Slack.Enabled},
		{"line", cfg.Channels.LINE.Enabled},
		{"onebot", cfg.Channels.OneBot.Enabled},
		{"qq", cfg.Channels.QQ.Enabled},
		{"dingtalk", cfg.Channels.DingTalk.Enabled},
		{"maixcam", cfg.Channels.MaixCam.Enabled},
		{"external", cfg.Channels.External.Enabled},
	}

	// Print header
	fmt.Printf("%-12s %-12s %-12s\n", "Channel", "Status", "Running")
	fmt.Println(strings.Repeat("-", 40))

	// Print each channel
	for _, ch := range channels {
		status := "❌ Disabled"
		if ch.enabled {
			status = "✅ Enabled"
		}
		running := "❌"
		if ch.enabled {
			running = "✅"
		}
		fmt.Printf("%-12s %-12s %-12s\n", ch.name, status, running)
	}

	fmt.Println()
	fmt.Println("Note: 'Running' status is only accurate when gateway is running")
	fmt.Println("Use 'nemesisbot channel status <name>' for detailed information")
}

func cmdChannelEnable(cfg *config.Config, channelName string) {
	configPath := GetConfigPath()

	switch channelName {
	case "web":
		cfg.Channels.Web.Enabled = true
		fmt.Println("✅ Web channel enabled")
		fmt.Printf("🌐 URL: http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
	case "telegram":
		cfg.Channels.Telegram.Enabled = true
		fmt.Println("✅ Telegram channel enabled")
	case "discord":
		cfg.Channels.Discord.Enabled = true
		fmt.Println("✅ Discord channel enabled")
	case "whatsapp":
		cfg.Channels.WhatsApp.Enabled = true
		fmt.Println("✅ WhatsApp channel enabled")
	case "feishu":
		cfg.Channels.Feishu.Enabled = true
		fmt.Println("✅ Feishu channel enabled")
	case "slack":
		cfg.Channels.Slack.Enabled = true
		fmt.Println("✅ Slack channel enabled")
	case "line":
		cfg.Channels.LINE.Enabled = true
		fmt.Println("✅ LINE channel enabled")
	case "onebot":
		cfg.Channels.OneBot.Enabled = true
		fmt.Println("✅ OneBot channel enabled")
	case "qq":
		cfg.Channels.QQ.Enabled = true
		fmt.Println("✅ QQ channel enabled")
	case "dingtalk":
		cfg.Channels.DingTalk.Enabled = true
		fmt.Println("✅ DingTalk channel enabled")
	case "maixcam":
		cfg.Channels.MaixCam.Enabled = true
		fmt.Println("✅ MaixCam channel enabled")
	case "external":
		fmt.Println("⚠️  External channel requires additional configuration")
		fmt.Println("   Use: nemesisbot channel external setup")
		cfg.Channels.External.Enabled = true
		fmt.Println("✅ External channel enabled (not configured yet)")
	default:
		fmt.Printf("❌ Unknown channel: %s\n", channelName)
		fmt.Println()
		fmt.Println("Available channels: web, telegram, discord, whatsapp, feishu, slack, line, onebot, qq, dingtalk, maixcam, external")
		os.Exit(1)
	}

	// Save config
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println()
	fmt.Println("Restart gateway for changes to take effect:")
	fmt.Println("  nemesisbot gateway")
}

func cmdChannelDisable(cfg *config.Config, channelName string) {
	configPath := GetConfigPath()

	switch channelName {
	case "web":
		cfg.Channels.Web.Enabled = false
		fmt.Println("❌ Web channel disabled")
	case "telegram":
		cfg.Channels.Telegram.Enabled = false
		fmt.Println("❌ Telegram channel disabled")
	case "discord":
		cfg.Channels.Discord.Enabled = false
		fmt.Println("❌ Discord channel disabled")
	case "whatsapp":
		cfg.Channels.WhatsApp.Enabled = false
		fmt.Println("❌ WhatsApp channel disabled")
	case "feishu":
		cfg.Channels.Feishu.Enabled = false
		fmt.Println("❌ Feishu channel disabled")
	case "slack":
		cfg.Channels.Slack.Enabled = false
		fmt.Println("❌ Slack channel disabled")
	case "line":
		cfg.Channels.LINE.Enabled = false
		fmt.Println("❌ LINE channel disabled")
	case "onebot":
		cfg.Channels.OneBot.Enabled = false
		fmt.Println("❌ OneBot channel disabled")
	case "qq":
		cfg.Channels.QQ.Enabled = false
		fmt.Println("❌ QQ channel disabled")
	case "dingtalk":
		cfg.Channels.DingTalk.Enabled = false
		fmt.Println("❌ DingTalk channel disabled")
	case "maixcam":
		cfg.Channels.MaixCam.Enabled = false
		fmt.Println("❌ MaixCam channel disabled")
	case "external":
		cfg.Channels.External.Enabled = false
		fmt.Println("❌ External channel disabled")
	default:
		fmt.Printf("❌ Unknown channel: %s\n", channelName)
		fmt.Println()
		fmt.Println("Available channels: web, telegram, discord, whatsapp, feishu, slack, line, onebot, qq, dingtalk, maixcam, external")
		os.Exit(1)
	}

	// Save config
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println()
	fmt.Println("Restart gateway for changes to take effect:")
	fmt.Println("  nemesisbot gateway")
}

func cmdChannelStatus(cfg *config.Config, channelName string) {
	fmt.Printf("%s Channel Status\n", strings.Title(channelName))
	fmt.Println("==================")
	fmt.Println()

	switch channelName {
	case "web":
		fmt.Printf("Enabled:         ")
		if cfg.Channels.Web.Enabled {
			fmt.Println("✅ Yes")
		} else {
			fmt.Println("❌ No")
		}
		fmt.Printf("Host:            %s\n", cfg.Channels.Web.Host)
		fmt.Printf("Port:            %d\n", cfg.Channels.Web.Port)
		fmt.Printf("WebSocket Path:  %s\n", cfg.Channels.Web.Path)
		fmt.Printf("Auth Required:   ")
		if cfg.Channels.Web.AuthToken != "" {
			fmt.Println("✅ Yes")
		} else {
			fmt.Println("❌ No")
		}
		fmt.Printf("Heartbeat:       %d seconds\n", cfg.Channels.Web.HeartbeatInterval)
		fmt.Printf("Session Timeout: %d seconds\n", cfg.Channels.Web.SessionTimeout)

		if cfg.Channels.Web.Enabled {
			fmt.Println()
			fmt.Println("🌐 Access URL:")
			fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
		}

	case "telegram":
		fmt.Printf("Enabled:   ")
		if cfg.Channels.Telegram.Enabled {
			fmt.Println("✅ Yes")
		} else {
			fmt.Println("❌ No")
		}
		fmt.Printf("Token:     ")
		if cfg.Channels.Telegram.Token != "" {
			fmt.Println("✅ Configured")
		} else {
			fmt.Println("❌ Not configured")
		}
		fmt.Printf("Proxy:     %s\n", cfg.Channels.Telegram.Proxy)

	case "discord":
		fmt.Printf("Enabled:   ")
		if cfg.Channels.Discord.Enabled {
			fmt.Println("✅ Yes")
		} else {
			fmt.Println("❌ No")
		}
		fmt.Printf("Token:     ")
		if cfg.Channels.Discord.Token != "" {
			fmt.Println("✅ Configured")
		} else {
			fmt.Println("❌ Not configured")
		}

	default:
		fmt.Printf("Enabled:   ")
		enabled := false
		switch channelName {
		case "whatsapp":
			enabled = cfg.Channels.WhatsApp.Enabled
		case "feishu":
			enabled = cfg.Channels.Feishu.Enabled
		case "slack":
			enabled = cfg.Channels.Slack.Enabled
		case "line":
			enabled = cfg.Channels.LINE.Enabled
		case "onebot":
			enabled = cfg.Channels.OneBot.Enabled
		case "qq":
			enabled = cfg.Channels.QQ.Enabled
		case "dingtalk":
			enabled = cfg.Channels.DingTalk.Enabled
		case "maixcam":
			enabled = cfg.Channels.MaixCam.Enabled
		case "external":
			enabled = cfg.Channels.External.Enabled
		}

		if enabled {
			fmt.Println("✅ Yes")
		} else {
			fmt.Println("❌ No")
		}
	}

	fmt.Println()
	fmt.Println("Note: This shows the configuration status, not the runtime status.")
	fmt.Println("Use 'nemesisbot gateway' to start the gateway and see runtime status.")
}

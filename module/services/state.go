package services

import "fmt"

// BotState represents the current state of the Bot service
type BotState int

const (
	// BotStateNotStarted indicates the bot has not been started
	BotStateNotStarted BotState = iota
	// BotStateStarting indicates the bot is currently starting
	BotStateStarting
	// BotStateRunning indicates the bot is running normally
	BotStateRunning
	// BotStateError indicates the bot is in an error state
	BotStateError
)

// String returns the string representation of the state
func (s BotState) String() string {
	switch s {
	case BotStateNotStarted:
		return "not_started"
	case BotStateStarting:
		return "starting"
	case BotStateRunning:
		return "running"
	case BotStateError:
		return "error"
	default:
		return "unknown"
	}
}

// MarshalJSON implements json.Marshaler interface
func (s BotState) MarshalJSON() ([]byte, error) {
	return []byte(fmt.Sprintf(`"%s"`, s.String())), nil
}

// UnmarshalJSON implements json.Unmarshaler interface
func (s *BotState) UnmarshalJSON(data []byte) error {
	str := string(data)
	if len(str) >= 2 && str[0] == '"' && str[len(str)-1] == '"' {
		str = str[1 : len(str)-1]
	}

	switch str {
	case "not_started":
		*s = BotStateNotStarted
	case "starting":
		*s = BotStateStarting
	case "running":
		*s = BotStateRunning
	case "error":
		*s = BotStateError
	default:
		return fmt.Errorf("unknown bot state: %s", str)
	}
	return nil
}

// IsRunning returns true if the bot is in a running state
func (s BotState) IsRunning() bool {
	return s == BotStateRunning
}

// CanStart returns true if the bot can be started
func (s BotState) CanStart() bool {
	return s == BotStateNotStarted || s == BotStateError
}

// CanStop returns true if the bot can be stopped
func (s BotState) CanStop() bool {
	return s == BotStateRunning || s == BotStateStarting
}

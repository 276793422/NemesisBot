package approval

import (
	"fmt"
	"time"
)

// ApprovalRequest 审批请求
type ApprovalRequest struct {
	RequestID      string            `json:"request_id"`
	Operation      string            `json:"operation"`
	Target         string            `json:"target"`
	RiskLevel      string            `json:"risk_level"`
	Reason         string            `json:"reason"`
	Context        map[string]string `json:"context"`
	TimeoutSeconds int               `json:"timeout_seconds"`
	Timestamp      int64             `json:"timestamp"`
}

// Validate 验证审批请求的有效性
//
// 检查请求的所有必填字段是否已填写，以及值是否有效
// 返回:
//   - error: 验证失败时返回错误信息
func (r *ApprovalRequest) Validate() error {
	if r.RequestID == "" {
		return fmt.Errorf("request_id is required")
	}

	if r.Operation == "" {
		return fmt.Errorf("operation is required")
	}

	if r.Target == "" {
		return fmt.Errorf("target is required")
	}

	if r.RiskLevel == "" {
		return fmt.Errorf("risk_level is required")
	}

	// 验证风险级别是否有效
	validRiskLevels := map[string]bool{
		RiskLevelLow:      true,
		RiskLevelMedium:   true,
		RiskLevelHigh:     true,
		RiskLevelCritical: true,
	}
	if !validRiskLevels[r.RiskLevel] {
		return fmt.Errorf("invalid risk_level: %s", r.RiskLevel)
	}

	if r.TimeoutSeconds <= 0 {
		return fmt.Errorf("timeout_seconds must be positive")
	}

	return nil
}

// ApprovalResponse 审批响应
type ApprovalResponse struct {
	RequestID       string  `json:"request_id"`
	Approved        bool    `json:"approved"`
	TimedOut        bool    `json:"timed_out"`
	DurationSeconds float64 `json:"duration_seconds"`
	ResponseTime    int64   `json:"response_time"`
}

// ApprovalConfig 审批配置
type ApprovalConfig struct {
	Enabled         bool          `json:"enabled"`
	Timeout         time.Duration `json:"timeout"`
	MinRiskLevel    string        `json:"min_risk_level"`
	DialogWidth     int           `json:"dialog_width"`
	DialogHeight    int           `json:"dialog_height"`
	EnableSound     bool          `json:"enable_sound"`
	EnableAnimation bool          `json:"enable_animation"`
}

// DefaultApprovalConfig 返回默认配置
func DefaultApprovalConfig() *ApprovalConfig {
	return &ApprovalConfig{
		Enabled:         true,
		Timeout:         30 * time.Second,
		MinRiskLevel:    "MEDIUM",
		DialogWidth:     550,
		DialogHeight:    480,
		EnableSound:     true,
		EnableAnimation: true,
	}
}

// DangerLevel 危险等级
type DangerLevel int

const (
	DangerLow      DangerLevel = 1 // LOW
	DangerMedium   DangerLevel = 2 // MEDIUM
	DangerHigh     DangerLevel = 3 // HIGH
	DangerCritical DangerLevel = 4 // CRITICAL
)

// String 返回危险等级的字符串表示
func (d DangerLevel) String() string {
	switch d {
	case DangerLow:
		return "LOW"
	case DangerMedium:
		return "MEDIUM"
	case DangerHigh:
		return "HIGH"
	case DangerCritical:
		return "CRITICAL"
	default:
		return "UNKNOWN"
	}
}

// ParseDangerLevel 从字符串解析危险等级
func ParseDangerLevel(s string) DangerLevel {
	switch s {
	case "LOW", "low":
		return DangerLow
	case "MEDIUM", "medium":
		return DangerMedium
	case "HIGH", "high":
		return DangerHigh
	case "CRITICAL", "critical":
		return DangerCritical
	default:
		return DangerLow
	}
}

// OperationType 操作类型
type OperationType string

const (
	OpFileRead        OperationType = "file_read"
	OpFileWrite       OperationType = "file_write"
	OpFileDelete      OperationType = "file_delete"
	OpFileEdit        OperationType = "file_edit"
	OpFileAppend      OperationType = "file_append"
	OpDirRead         OperationType = "dir_read"
	OpDirList         OperationType = "dir_list"
	OpDirCreate       OperationType = "dir_create"
	OpDirDelete       OperationType = "dir_delete"
	OpProcessExec     OperationType = "process_exec"
	OpProcessSpawn    OperationType = "process_spawn"
	OpProcessKill     OperationType = "process_kill"
	OpProcessSuspend  OperationType = "process_suspend"
	OpNetworkDownload OperationType = "network_download"
	OpNetworkUpload   OperationType = "network_upload"
	OpNetworkRequest  OperationType = "network_request"
	OpHardwareI2C     OperationType = "hardware_i2c"
	OpHardwareSPI     OperationType = "hardware_spi"
	OpHardwareGPIO    OperationType = "hardware_gpio"
	OpSystemShutdown  OperationType = "system_shutdown"
	OpSystemReboot    OperationType = "system_reboot"
	OpSystemConfig    OperationType = "system_config"
	OpSystemService   OperationType = "system_service"
	OpSystemInstall   OperationType = "system_install"
	OpRegistryRead    OperationType = "registry_read"
	OpRegistryWrite   OperationType = "registry_write"
	OpRegistryDelete  OperationType = "registry_delete"
)

// Operation type constants (string form for backward compatibility)
const (
	OperationFileRead       = "file_read"
	OperationFileWrite      = "file_write"
	OperationFileDelete     = "file_delete"
	OperationFileEdit       = "file_edit"
	OperationFileAppend     = "file_append"
	OperationDirList        = "dir_list"
	OperationDirCreate      = "dir_create"
	OperationDirDelete      = "dir_delete"
	OperationProcessExec    = "process_exec"
	OperationProcessSpawn   = "process_spawn"
	OperationProcessKill    = "process_kill"
	OperationRegistryRead   = "registry_read"
	OperationRegistryWrite  = "registry_write"
	OperationNetworkRequest = "network_request"
	OperationNetworkDownload = "network_download"
	OperationHardwareI2C    = "hardware_i2c"
	OperationSystemShutdown = "system_shutdown"
	OperationSystemReboot   = "system_reboot"
)

// Risk level constants
const (
	RiskLevelLow      = "LOW"
	RiskLevelMedium   = "MEDIUM"
	RiskLevelHigh     = "HIGH"
	RiskLevelCritical = "CRITICAL"
)


// GetOperationDisplayName 获取操作类型的显示名称
func GetOperationDisplayName(op string) string {
	names := map[string]string{
		"file_delete":      "File Delete",
		"file_write":       "File Write",
		"file_read":        "File Read",
		"file_edit":        "File Edit",
		"file_append":      "File Append",
		"process_exec":     "Process Execute",
		"process_kill":     "Process Kill",
		"process_spawn":    "Process Spawn",
		"registry_write":   "Registry Write",
		"registry_delete":  "Registry Delete",
		"registry_read":    "Registry Read",
		"dir_delete":       "Directory Delete",
		"dir_create":       "Directory Create",
		"dir_list":         "Directory List",
		"system_shutdown":  "System Shutdown",
		"system_reboot":    "System Reboot",
		"network_request":  "Network Request",
		"network_download": "Network Download",
		"hardware_i2c":     "Hardware I2C Access",
	}

	if name, ok := names[op]; ok {
		return name
	}
	return op
}

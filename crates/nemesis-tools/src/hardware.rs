//! Hardware tools - I2C and SPI bus interaction.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// P8 GPIO 白名单：I2C 地址访问策略（单一真相源）。
//
// 真实 ioctl 路径（本文件 I2CTool）与 agent loop 注册的生产工具路径
// （nemesis-agent loop_tools.rs）共用同一份策略，杜绝两处漂移。
// 三层：①内置编译期白名单（I2C 规范可寻址空间，排除保留段）；
// ②config tools.hardware 显式 allow/deny 覆盖（deny 优先）；
// ③拒绝理由面向模型可自纠（说明段用途 + 放行办法）。
// ---------------------------------------------------------------------------

/// I2C 地址访问策略。
///
/// `i2c_allow_ranges` 为空 = 用内置默认（0x08-0x77，I2C 规范可自由寻址
/// 空间）；非空 = 完全替换内置默认。`i2c_deny_ranges` 在 allow 之上再
/// 排除（deny 优先，用于屏蔽已知有风险的设备地址）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardwarePolicy {
    /// 允许的 7 位地址段（闭区间 [lo, hi]）；空 = 内置默认 0x08-0x77。
    pub i2c_allow_ranges: Vec<(u8, u8)>,
    /// 拒绝的地址段（在 allow 之上再排除）。
    pub i2c_deny_ranges: Vec<(u8, u8)>,
}

impl HardwarePolicy {
    /// 内置默认策略：只放行 I2C 规范可寻址空间 0x08-0x77。
    ///
    /// 旧的 `(0x03..=0x77)` 校验会放行保留段 0x03-0x07（通用呼叫/CBUS
    /// 等保留用途），向这些地址写数据可能干扰总线上的其他设备——白名单
    /// 一并把这个洞堵上。
    pub fn builtin() -> Self {
        Self {
            i2c_allow_ranges: vec![(0x08, 0x77)],
            i2c_deny_ranges: Vec::new(),
        }
    }

    /// 校验一个 7 位 I2C 地址；拒绝时返回面向模型可自纠的理由。
    pub fn validate_i2c_address(&self, addr: u8) -> Result<(), String> {
        // deny 优先：显式黑名单无条件拒绝（哪怕落在 allow 里）。
        for (lo, hi) in &self.i2c_deny_ranges {
            if addr >= *lo && addr <= *hi {
                return Err(format!(
                    "address 0x{addr:02x} rejected: 位于 config 显式黑名单段 \
                     0x{lo:02x}-0x{hi:02x}（deny 优先于 allow）。如需操作该地址，\
                     请先从 config.json tools.hardware.i2c_deny_ranges 移除该段"
                ));
            }
        }

        let allow = if self.i2c_allow_ranges.is_empty() {
            &[(0x08, 0x77)][..]
        } else {
            &self.i2c_allow_ranges[..]
        };
        for (lo, hi) in allow {
            if addr >= *lo && addr <= *hi {
                return Ok(());
            }
        }

        // 不在 allow 里：区分「保留段」与「普通越界」，给模型可行动的
        // 理由（段用途 + 放行办法），而不是一句干巴巴的 invalid。
        if let Some(reason) = i2c_reserved_reason(addr) {
            return Err(format!(
                "address 0x{addr:02x} rejected: {reason}。默认允许范围 \
                 0x08-0x77；如确有需要，可在 config.json \
                 tools.hardware.i2c_allow_ranges 显式放行（格式 [[lo, hi], ...]，\
                 7 位地址），但请先与用户确认"
            ));
        }
        Err(format!(
            "address 0x{addr:02x} rejected: 不在允许范围。默认允许 \
             0x08-0x77；可用 config.json tools.hardware.i2c_allow_ranges \
             配置其他允许段（格式 [[lo, hi], ...]，7 位地址）"
        ))
    }
}

/// I2C 地址参数解析（桩与真实实现的单一真相源，两路径同源校验的前提）。
///
/// 接受数字（≤0x7f）或字符串（`"0x38"` 十六进制带前缀 / `"56"` 十进制）；
/// 其余形态返回面向模型可自纠的错误。此前两处各自内联 `as_u64()`：真实
/// 实现对字符串形态诚实报错（fail-closed），而回显桩对字符串形态**静默
/// 跳过白名单校验**（fail-open）——桩自己的 schema 还写着 hex 字符串，
/// 正是放行保留段的洞（2026-09-26 复查修复）。
pub fn parse_i2c_address(args: &serde_json::Value) -> Result<u8, String> {
    let raw = &args["address"];
    let num = match raw {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => {
            let t = s.trim();
            let parsed = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16).ok()
            } else {
                t.parse::<u64>().ok()
            };
            // 数字字符串走解析结果；解析失败落到下方统一报错
            if parsed.is_some() {
                parsed
            } else {
                return Err(I2C_ADDRESS_ERR.to_string());
            }
        }
        _ => return Err(I2C_ADDRESS_ERR.to_string()),
    };
    match num {
        Some(a) if a <= 0x7f => Ok(a as u8),
        _ => Err(I2C_ADDRESS_ERR.to_string()),
    }
}

/// 地址错误统一文案（7-bit 上限 + 可自纠示例）。
const I2C_ADDRESS_ERR: &str = "address is required (7-bit, e.g. 56 or \"0x38\", max 0x7f)";

/// I2C 规范保留段用途说明（None = 非保留地址）。
///
/// 分段依据 NXP UM10204（I2C-bus specification）：
/// - 0x00-0x07：通用呼叫 / START byte / CBUS 等保留
/// - 0x78-0x7B：高速模式主机码保留
/// - 0x7C-0x7F：10 位寻址前缀保留
pub fn i2c_reserved_reason(addr: u8) -> Option<&'static str> {
    match addr {
        0x00..=0x07 => Some(
            "位于 I2C 保留段 0x00-0x07（通用呼叫/START byte/CBUS 等保留用途，\
             访问可能干扰总线）",
        ),
        0x78..=0x7B => Some("位于 I2C 保留段 0x78-0x7B（高速模式主机码保留）"),
        0x7C..=0x7F => Some("位于 I2C 保留段 0x7C-0x7F（10 位寻址前缀保留）"),
        _ => None,
    }
}

// --------------- I2C Tool ---------------

/// I2C tool - interacts with I2C bus devices (Linux only).
pub struct I2CTool {
    /// 地址访问策略（构造时注入；`new()` = 内置默认）。真实 I/O 前校验。
    policy: HardwarePolicy,
}

impl I2CTool {
    /// Create a new I2C tool (内置默认白名单).
    pub fn new() -> Self {
        Self {
            policy: HardwarePolicy::builtin(),
        }
    }

    /// Create a new I2C tool with a custom address policy.
    pub fn with_policy(policy: HardwarePolicy) -> Self {
        Self { policy }
    }
}

impl Default for I2CTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for I2CTool {
    fn name(&self) -> &str {
        "i2c"
    }

    fn description(&self) -> &str {
        "Interact with I2C bus devices. Actions: detect (list buses), scan (find devices), read (read bytes), write (send bytes). Linux only."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["detect", "scan", "read", "write"],
                    "description": "Action to perform"
                },
                "bus": {"type": "string", "description": "I2C bus number (e.g. \"1\")"},
                "address": {"type": "integer", "description": "7-bit device address (default allowed: 0x08-0x77; reserved segments rejected)"},
                "register": {"type": "integer", "description": "Register address"},
                "data": {"type": "array", "items": {"type": "integer"}, "description": "Bytes to write (0-255)"},
                "length": {"type": "integer", "description": "Number of bytes to read (1-256)"},
                "confirm": {"type": "boolean", "description": "Must be true for write operations"}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        // I2C requires Linux
        if !cfg!(target_os = "linux") {
            return ToolResult::error(
                "I2C is only supported on Linux. This tool requires /dev/i2c-* device files.",
            );
        }

        let action = match args["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::error("action is required"),
        };

        match action {
            "detect" => self.detect().await,
            "scan" => self.scan(args).await,
            "read" => self.read_device(args).await,
            "write" => self.write_device(args).await,
            _ => ToolResult::error(&format!(
                "unknown action: {} (valid: detect, scan, read, write)",
                action
            )),
        }
    }
}

impl I2CTool {
    /// Detect I2C buses by listing /dev/i2c-*.
    async fn detect(&self) -> ToolResult {
        let mut entries = Vec::new();
        let mut bus_num = 0;

        // Try to find i2c buses
        while bus_num < 32 {
            let path = format!("/dev/i2c-{}", bus_num);
            if tokio::fs::metadata(&path).await.is_ok() {
                entries.push(serde_json::json!({
                    "path": path,
                    "bus": bus_num
                }));
            }
            bus_num += 1;
        }

        if entries.is_empty() {
            return ToolResult::silent(
                "No I2C buses found. You may need to:\n\
                 1. Load the i2c-dev module: modprobe i2c-dev\n\
                 2. Check that I2C is enabled in device tree\n\
                 3. Configure pinmux for your board",
            );
        }

        let result = serde_json::to_string_pretty(&entries).unwrap_or_default();
        ToolResult::silent(&format!("Found {} I2C bus(es):\n{}", entries.len(), result))
    }

    /// Scan for devices on an I2C bus using SMBus probe (Linux ioctl).
    async fn scan(&self, args: &serde_json::Value) -> ToolResult {
        let bus = match args["bus"].as_str() {
            Some(b) if !b.is_empty() => b,
            _ => return ToolResult::error("bus is required (e.g. \"1\" for /dev/i2c-1)"),
        };

        // Validate bus ID is a simple number
        if !bus.chars().all(|c| c.is_ascii_digit()) {
            return ToolResult::error("invalid bus identifier: must be a number");
        }

        let device_path = format!("/dev/i2c-{}", bus);
        if tokio::fs::metadata(&device_path).await.is_err() {
            return ToolResult::error(&format!(
                "failed to open {}: device not found (check permissions and i2c-dev module)",
                device_path
            ));
        }

        #[cfg(target_os = "linux")]
        {
            match linux_i2c_scan(&device_path, &self.policy) {
                Ok(result) => result,
                Err(e) => ToolResult::error(&e),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = device_path;
            ToolResult::silent(&format!(
                "I2C scan on bus {} (platform not supported - requires Linux ioctl)",
                bus
            ))
        }
    }

    /// Read bytes from an I2C device.
    async fn read_device(&self, args: &serde_json::Value) -> ToolResult {
        let bus = match args["bus"].as_str() {
            Some(b) if !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()) => b,
            _ => return ToolResult::error("bus is required"),
        };

        let addr = match parse_i2c_address(args) {
            Ok(a) => a,
            Err(reason) => return ToolResult::error(&reason),
        };
        // P8 GPIO 白名单：I/O 前过地址策略（保留段/黑名单拒绝，理由可自纠）。
        if let Err(reason) = self.policy.validate_i2c_address(addr) {
            return ToolResult::error(&reason);
        }

        let length = args["length"].as_u64().unwrap_or(1).clamp(1, 256) as usize;

        let device_path = format!("/dev/i2c-{}", bus);

        #[cfg(target_os = "linux")]
        {
            match linux_i2c_read(&device_path, addr, args, length) {
                Ok(result) => result,
                Err(e) => ToolResult::error(&e),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = device_path;
            ToolResult::silent(&format!(
                "I2C read {} bytes from 0x{:02x} (platform not supported - requires Linux ioctl)",
                length, addr
            ))
        }
    }

    /// Write bytes to an I2C device.
    async fn write_device(&self, args: &serde_json::Value) -> ToolResult {
        let confirm = args["confirm"].as_bool().unwrap_or(false);
        if !confirm {
            return ToolResult::error(
                "write operations require confirm: true. Please confirm with the user before writing to I2C devices, as incorrect writes can misconfigure hardware.",
            );
        }

        let bus = match args["bus"].as_str() {
            Some(b) if !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()) => b,
            _ => return ToolResult::error("bus is required"),
        };

        let addr = match parse_i2c_address(args) {
            Ok(a) => a,
            Err(reason) => return ToolResult::error(&reason),
        };
        // P8 GPIO 白名单：写操作破坏力最大，同样过地址策略。
        if let Err(reason) = self.policy.validate_i2c_address(addr) {
            return ToolResult::error(&reason);
        }

        let data_array = match args["data"].as_array() {
            Some(d) if !d.is_empty() => d,
            _ => {
                return ToolResult::error(
                    "data is required for write (array of byte values 0-255)",
                );
            }
        };

        if data_array.len() > 256 {
            return ToolResult::error("data too long: maximum 256 bytes per I2C transaction");
        }

        // Parse data bytes, optionally prepending register
        let mut data_bytes: Vec<u8> = Vec::with_capacity(data_array.len() + 1);

        if let Some(reg) = args["register"].as_u64() {
            if reg > 255 {
                return ToolResult::error("register must be between 0x00 and 0xFF");
            }
            data_bytes.push(reg as u8);
        }

        for (i, v) in data_array.iter().enumerate() {
            match v.as_u64() {
                Some(b) if b <= 255 => data_bytes.push(b as u8),
                _ => return ToolResult::error(&format!("data[{}] is not a valid byte value", i)),
            }
        }

        let device_path = format!("/dev/i2c-{}", bus);

        #[cfg(target_os = "linux")]
        {
            match linux_i2c_write(&device_path, addr, &data_bytes) {
                Ok(result) => result,
                Err(e) => ToolResult::error(&e),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = device_path;
            ToolResult::silent(&format!(
                "I2C write {} bytes to 0x{:02x} (platform not supported - requires Linux ioctl)",
                data_bytes.len(),
                addr
            ))
        }
    }

    #[allow(dead_code)]
    fn parse_bus(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        match args["bus"].as_str() {
            Some(b) if !b.is_empty() => {
                if !b.chars().all(|c| c.is_ascii_digit()) {
                    Err(ToolResult::error(
                        "invalid bus identifier: must be a number",
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Err(ToolResult::error("bus is required")),
        }
    }

    #[allow(dead_code)]
    fn parse_address(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        // 单一真相源：数字/十六进制字符串统一解析（字符串形态此前被静默
        // 放过——parse 失败即整个跳过校验的 fail-open 洞，已堵）。
        match parse_i2c_address(args) {
            Ok(addr) => {
                if let Err(reason) = self.policy.validate_i2c_address(addr) {
                    Err(ToolResult::error(&reason))
                } else {
                    Ok(())
                }
            }
            Err(reason) => Err(ToolResult::error(&reason)),
        }
    }
}

// --------------- SPI Tool ---------------

/// SPI tool - interacts with SPI bus devices (Linux only).
pub struct SPITool;

impl SPITool {
    /// Create a new SPI tool.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SPITool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SPITool {
    fn name(&self) -> &str {
        "spi"
    }

    fn description(&self) -> &str {
        "Interact with SPI bus devices. Actions: list (find devices), transfer (full-duplex), read (receive bytes). Linux only."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "transfer", "read"],
                    "description": "Action to perform"
                },
                "device": {"type": "string", "description": "SPI device (e.g. \"2.0\")"},
                "speed": {"type": "integer", "description": "Clock speed in Hz (default: 1000000)"},
                "mode": {"type": "integer", "description": "SPI mode 0-3 (default: 0)"},
                "bits": {"type": "integer", "description": "Bits per word (default: 8)"},
                "data": {"type": "array", "items": {"type": "integer"}, "description": "Bytes to send"},
                "length": {"type": "integer", "description": "Bytes to read (1-4096)"},
                "confirm": {"type": "boolean", "description": "Must be true for transfer"}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        if !cfg!(target_os = "linux") {
            return ToolResult::error(
                "SPI is only supported on Linux. This tool requires /dev/spidev* device files.",
            );
        }

        let action = match args["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::error("action is required"),
        };

        match action {
            "list" => self.list().await,
            "transfer" => self.transfer(args).await,
            "read" => self.read_device(args).await,
            _ => ToolResult::error(&format!(
                "unknown action: {} (valid: list, transfer, read)",
                action
            )),
        }
    }
}

impl SPITool {
    /// List available SPI devices.
    async fn list(&self) -> ToolResult {
        let mut devices = Vec::new();

        // Try common SPI device paths
        for bus in 0..=8 {
            for cs in 0..=4 {
                let path = format!("/dev/spidev{}.{}", bus, cs);
                if tokio::fs::metadata(&path).await.is_ok() {
                    devices.push(serde_json::json!({
                        "path": path,
                        "device": format!("{}.{}", bus, cs)
                    }));
                }
            }
        }

        if devices.is_empty() {
            return ToolResult::silent(
                "No SPI devices found. You may need to:\n\
                 1. Enable SPI in device tree\n\
                 2. Configure pinmux for your board\n\
                 3. Check that spidev module is loaded",
            );
        }

        let result = serde_json::to_string_pretty(&devices).unwrap_or_default();
        ToolResult::silent(&format!(
            "Found {} SPI device(s):\n{}",
            devices.len(),
            result
        ))
    }

    /// Full-duplex SPI transfer.
    async fn transfer(&self, args: &serde_json::Value) -> ToolResult {
        let confirm = args["confirm"].as_bool().unwrap_or(false);
        if !confirm {
            return ToolResult::error(
                "transfer operations require confirm: true. Please confirm with the user before sending data to SPI devices.",
            );
        }

        let device = match args["device"].as_str() {
            Some(d) if !d.is_empty() => {
                let parts: Vec<&str> = d.split('.').collect();
                if parts.len() != 2
                    || !parts[0].chars().all(|c| c.is_ascii_digit())
                    || !parts[1].chars().all(|c| c.is_ascii_digit())
                {
                    return ToolResult::error(
                        "invalid device identifier: must be in format \"X.Y\"",
                    );
                }
                d.to_string()
            }
            _ => return ToolResult::error("device is required (e.g. \"2.0\" for /dev/spidev2.0)"),
        };

        if let Err(e) = self.validate_spi_params(args) {
            return e;
        }

        let data_array = match args["data"].as_array() {
            Some(d) if !d.is_empty() => d,
            _ => {
                return ToolResult::error(
                    "data is required for transfer (array of byte values 0-255)",
                );
            }
        };
        if data_array.len() > 4096 {
            return ToolResult::error("data too long: maximum 4096 bytes per SPI transfer");
        }

        let mut tx_buf: Vec<u8> = Vec::with_capacity(data_array.len());
        for (i, v) in data_array.iter().enumerate() {
            match v.as_u64() {
                Some(b) if b <= 255 => tx_buf.push(b as u8),
                _ => return ToolResult::error(&format!("data[{}] is not a valid byte value", i)),
            }
        }

        let _speed = args["speed"].as_u64().unwrap_or(1_000_000) as u32;
        let _mode = args["mode"].as_u64().unwrap_or(0) as u8;
        let _bits = args["bits"].as_u64().unwrap_or(8) as u8;
        let dev_path = format!("/dev/spidev{}", device);

        #[cfg(target_os = "linux")]
        {
            match linux_spi_transfer(&dev_path, _mode, _bits, _speed, &tx_buf) {
                Ok(result) => result,
                Err(e) => ToolResult::error(&e),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = dev_path;
            ToolResult::silent(&format!(
                "SPI transfer {} bytes (platform not supported - requires Linux ioctl)",
                tx_buf.len()
            ))
        }
    }

    /// Read bytes from SPI device.
    async fn read_device(&self, args: &serde_json::Value) -> ToolResult {
        let device = match args["device"].as_str() {
            Some(d) if !d.is_empty() => {
                let parts: Vec<&str> = d.split('.').collect();
                if parts.len() != 2
                    || !parts[0].chars().all(|c| c.is_ascii_digit())
                    || !parts[1].chars().all(|c| c.is_ascii_digit())
                {
                    return ToolResult::error(
                        "invalid device identifier: must be in format \"X.Y\"",
                    );
                }
                d.to_string()
            }
            _ => return ToolResult::error("device is required (e.g. \"2.0\" for /dev/spidev2.0)"),
        };

        if let Err(e) = self.validate_spi_params(args) {
            return e;
        }

        let length = args["length"].as_u64().unwrap_or(1);
        if length == 0 || length > 4096 {
            return ToolResult::error("length must be between 1 and 4096");
        }

        let speed = args["speed"].as_u64().unwrap_or(1_000_000) as u32;
        let mode = args["mode"].as_u64().unwrap_or(0) as u8;
        let bits = args["bits"].as_u64().unwrap_or(8) as u8;
        let dev_path = format!("/dev/spidev{}", device);

        #[cfg(target_os = "linux")]
        {
            match linux_spi_read(&dev_path, mode, bits, speed, length as usize) {
                Ok(result) => result,
                Err(e) => ToolResult::error(&e),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (dev_path, mode, bits, speed);
            ToolResult::silent(&format!(
                "SPI read {} bytes (platform not supported - requires Linux ioctl)",
                length
            ))
        }
    }

    #[allow(dead_code)]
    fn parse_device(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        match args["device"].as_str() {
            Some(d) if !d.is_empty() => {
                // Validate format X.Y
                let parts: Vec<&str> = d.split('.').collect();
                if parts.len() != 2
                    || !parts[0].chars().all(|c| c.is_ascii_digit())
                    || !parts[1].chars().all(|c| c.is_ascii_digit())
                {
                    Err(ToolResult::error(
                        "invalid device identifier: must be in format \"X.Y\"",
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Err(ToolResult::error(
                "device is required (e.g. \"2.0\" for /dev/spidev2.0)",
            )),
        }
    }

    fn validate_spi_params(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        if let Some(speed) = args["speed"].as_u64()
            && (speed == 0 || speed > 125_000_000)
        {
            return Err(ToolResult::error("speed must be between 1 Hz and 125 MHz"));
        }
        if let Some(mode) = args["mode"].as_u64()
            && mode > 3
        {
            return Err(ToolResult::error("mode must be 0-3"));
        }
        if let Some(bits) = args["bits"].as_u64()
            && (bits == 0 || bits > 32)
        {
            return Err(ToolResult::error("bits must be between 1 and 32"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Linux-specific I2C and SPI implementations using ioctl
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use std::os::unix::io::AsRawFd;

    // I2C ioctl constants from <linux/i2c-dev.h>, <linux/i2c.h>
    const I2C_SLAVE: u64 = 0x0703;
    const I2C_FUNCS: u64 = 0x0705;
    const I2C_SMBUS: u64 = 0x0720;

    // I2C_FUNC capability bits
    const I2C_FUNC_SMBUS_QUICK: usize = 0x00010000;
    const I2C_FUNC_SMBUS_READ_BYTE: usize = 0x00020000;

    // SMBus transaction types
    const I2C_SMBUS_WRITE: u8 = 1;
    const I2C_SMBUS_READ: u8 = 0;

    // SMBus protocol sizes
    const I2C_SMBUS_QUICK: u32 = 0;
    const I2C_SMBUS_BYTE: u32 = 1;

    /// i2c_smbus_ioctl_data matches the kernel struct.
    /// 12 bytes: read_write(u8) + command(u8) + size(u32) + data pointer.
    #[repr(C)]
    struct I2cSmbusIoctlData {
        read_write: u8,
        command: u8,
        _pad: u16,
        size: u32,
        data: *const [u8; 34],
    }

    // SPI ioctl constants from <linux/spi/spidev.h>
    // _IOW('k', nr, size) = direction(1)<<30 | size<<16 | type(0x6B)<<8 | nr
    const SPI_IOC_WR_MODE: u64 = 0x40016B01;
    const SPI_IOC_WR_BITS_PER_WORD: u64 = 0x40016B03;
    const SPI_IOC_WR_MAX_SPEED_HZ: u64 = 0x40046B04;
    const SPI_IOC_MESSAGE_1: u64 = 0x40206B00;

    /// spi_ioc_transfer matches the kernel struct (32 bytes).
    #[repr(C)]
    struct SpiIocTransfer {
        tx_buf: u64,
        rx_buf: u64,
        length: u32,
        speed_hz: u32,
        delay_usecs: u16,
        bits_per_word: u8,
        cs_change: u8,
        tx_nbits: u8,
        rx_nbits: u8,
        word_delay: u8,
        pad: u8,
    }

    /// Open an I2C device file.
    fn open_i2c(dev_path: &str) -> Result<std::fs::File, String> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dev_path)
            .map_err(|e| {
                format!(
                    "failed to open {}: {} (check permissions and i2c-dev module)",
                    dev_path, e
                )
            })
    }

    /// Probe a single I2C address using SMBus.
    fn smbus_probe(fd: &std::fs::File, addr: usize, has_quick: bool) -> bool {
        // EEPROM ranges: use read byte (quick write can corrupt AT24RF08)
        let use_read_byte = (0x30..=0x37).contains(&addr) || (0x50..=0x5F).contains(&addr);

        if !use_read_byte && has_quick {
            // SMBus Quick Write: safest probe, no data transferred
            let args = I2cSmbusIoctlData {
                read_write: I2C_SMBUS_WRITE,
                command: 0,
                _pad: 0,
                size: I2C_SMBUS_QUICK,
                data: std::ptr::null(),
            };
            let ret =
                unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SMBUS as _, &args as *const _ as usize) };
            return ret >= 0;
        }

        // SMBus Read Byte
        let _data = [0u8; 34];
        let args = I2cSmbusIoctlData {
            read_write: I2C_SMBUS_READ,
            command: 0,
            _pad: 0,
            size: I2C_SMBUS_BYTE,
            data: &_data as *const _,
        };
        let ret =
            unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SMBUS as _, &args as *const _ as usize) };
        ret >= 0
    }

    /// I2C bus scan using ioctl SMBus probes.
    ///
    /// `policy` 决定扫描范围（allow 段减去 deny 段），与 read/write 的
    /// 地址校验同源——白名单外的地址既不能操作也不会被探测。
    pub fn linux_i2c_scan(dev_path: &str, policy: &HardwarePolicy) -> Result<ToolResult, String> {
        let fd = open_i2c(dev_path)?;

        // Query adapter capabilities
        let mut funcs: usize = 0;
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                I2C_FUNCS as _,
                &mut funcs as *mut _ as usize,
            )
        };
        if ret < 0 {
            return Err(format!(
                "failed to query I2C adapter capabilities on {}",
                dev_path
            ));
        }

        let has_quick = funcs & I2C_FUNC_SMBUS_QUICK != 0;
        let has_read_byte = funcs & I2C_FUNC_SMBUS_READ_BYTE != 0;

        if !has_quick && !has_read_byte {
            return Err(format!(
                "I2C adapter {} supports neither SMBus Quick nor Read Byte - cannot probe safely",
                dev_path
            ));
        }

        let allowed = |addr: u8| policy.validate_i2c_address(addr).is_ok();

        let mut found: Vec<serde_json::Value> = Vec::new();

        // 7-bit 全空间扫描，逐地址过白名单（保留段/黑名单地址不探测）。
        for addr in 0x00..=0x7fu16 {
            if !allowed(addr as u8) {
                continue;
            }
            // Set slave address
            let ret = unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SLAVE as _, addr as usize) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EBUSY) {
                    found.push(serde_json::json!({
                        "address": format!("0x{:02x}", addr),
                        "status": "busy (in use by kernel driver)"
                    }));
                }
                continue;
            }

            if smbus_probe(&fd, addr as usize, has_quick) {
                found.push(serde_json::json!({
                    "address": format!("0x{:02x}", addr)
                }));
            }
        }

        if found.is_empty() {
            return Ok(ToolResult::silent(&format!(
                "No devices found on {}. Check wiring and pull-up resistors.",
                dev_path
            )));
        }

        let result = serde_json::json!({
            "bus": dev_path,
            "devices": found,
            "count": found.len(),
        });
        let output = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(ToolResult::silent(&format!(
            "Scan of {}:\n{}",
            dev_path, output
        )))
    }

    /// I2C read using ioctl.
    pub fn linux_i2c_read(
        dev_path: &str,
        addr: u8,
        args: &serde_json::Value,
        length: usize,
    ) -> Result<ToolResult, String> {
        let fd = open_i2c(dev_path)?;

        // Set slave address
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SLAVE as _, addr as usize) };
        if ret < 0 {
            return Err(format!("failed to set I2C address 0x{:02x}", addr));
        }

        // If register specified, write it first
        if let Some(reg) = args["register"].as_u64() {
            if reg > 255 {
                return Err("register must be between 0x00 and 0xFF".to_string());
            }
            std::os::unix::fs::FileExt::write_all_at(&fd, &[reg as u8], 0)
                .map_err(|e| format!("failed to write register 0x{:02x}: {}", reg, e))?;
        }

        // Read data
        let mut buf = vec![0u8; length];
        let n = std::os::unix::fs::FileExt::read_at(&fd, &mut buf, 0)
            .map_err(|e| format!("failed to read from device 0x{:02x}: {}", addr, e))?;

        let hex_bytes: Vec<String> = buf[..n].iter().map(|b| format!("0x{:02x}", b)).collect();
        let int_bytes: Vec<u64> = buf[..n].iter().map(|b| *b as u64).collect();

        let result = serde_json::json!({
            "bus": dev_path,
            "address": format!("0x{:02x}", addr),
            "bytes": int_bytes,
            "hex": hex_bytes,
            "length": n,
        });
        let output = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(ToolResult::silent(&output))
    }

    /// I2C write using ioctl.
    pub fn linux_i2c_write(dev_path: &str, addr: u8, data: &[u8]) -> Result<ToolResult, String> {
        let fd = open_i2c(dev_path)?;

        // Set slave address
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SLAVE as _, addr as usize) };
        if ret < 0 {
            return Err(format!("failed to set I2C address 0x{:02x}", addr));
        }

        // Write data
        std::os::unix::fs::FileExt::write_all_at(&fd, data, 0)
            .map_err(|e| format!("failed to write to device 0x{:02x}: {}", addr, e))?;

        Ok(ToolResult::silent(&format!(
            "Wrote {} byte(s) to device 0x{:02x} on {}",
            data.len(),
            addr,
            dev_path
        )))
    }

    /// Configure SPI device (open + set mode, bits, speed).
    fn configure_spi(
        dev_path: &str,
        mode: u8,
        bits: u8,
        speed: u32,
    ) -> Result<std::fs::File, String> {
        let fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dev_path)
            .map_err(|e| {
                format!(
                    "failed to open {}: {} (check permissions and spidev module)",
                    dev_path, e
                )
            })?;

        // Set SPI mode
        let mut m = mode;
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                SPI_IOC_WR_MODE as _,
                &mut m as *mut _ as usize,
            )
        };
        if ret < 0 {
            return Err(format!(
                "failed to set SPI mode {}: {}",
                mode,
                std::io::Error::last_os_error()
            ));
        }

        // Set bits per word
        let mut b = bits;
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                SPI_IOC_WR_BITS_PER_WORD as _,
                &mut b as *mut _ as usize,
            )
        };
        if ret < 0 {
            return Err(format!(
                "failed to set bits per word {}: {}",
                bits,
                std::io::Error::last_os_error()
            ));
        }

        // Set max speed
        let mut s = speed;
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                SPI_IOC_WR_MAX_SPEED_HZ as _,
                &mut s as *mut _ as usize,
            )
        };
        if ret < 0 {
            return Err(format!(
                "failed to set SPI speed {} Hz: {}",
                speed,
                std::io::Error::last_os_error()
            ));
        }

        Ok(fd)
    }

    /// SPI full-duplex transfer using ioctl.
    pub fn linux_spi_transfer(
        dev_path: &str,
        mode: u8,
        bits: u8,
        speed: u32,
        tx_buf: &[u8],
    ) -> Result<ToolResult, String> {
        let fd = configure_spi(dev_path, mode, bits, speed)?;

        let mut rx_buf = vec![0u8; tx_buf.len()];

        let xfer = SpiIocTransfer {
            tx_buf: tx_buf.as_ptr() as u64,
            rx_buf: rx_buf.as_mut_ptr() as u64,
            length: tx_buf.len() as u32,
            speed_hz: speed,
            delay_usecs: 0,
            bits_per_word: bits,
            cs_change: 0,
            tx_nbits: 0,
            rx_nbits: 0,
            word_delay: 0,
            pad: 0,
        };

        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                SPI_IOC_MESSAGE_1 as _,
                &xfer as *const _ as usize,
            )
        };

        let _ = tx_buf;
        let _ = &rx_buf;

        if ret < 0 {
            return Err(format!(
                "SPI transfer failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let hex_bytes: Vec<String> = rx_buf.iter().map(|b| format!("0x{:02x}", b)).collect();
        let int_bytes: Vec<u64> = rx_buf.iter().map(|b| *b as u64).collect();

        let result = serde_json::json!({
            "device": dev_path,
            "sent": tx_buf.len(),
            "received": int_bytes,
            "hex": hex_bytes,
        });
        let output = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(ToolResult::silent(&output))
    }

    /// SPI read using ioctl (sends zeros, receives bytes).
    pub fn linux_spi_read(
        dev_path: &str,
        mode: u8,
        bits: u8,
        speed: u32,
        length: usize,
    ) -> Result<ToolResult, String> {
        let fd = configure_spi(dev_path, mode, bits, speed)?;

        let tx_buf = vec![0u8; length];
        let mut rx_buf = vec![0u8; length];

        let xfer = SpiIocTransfer {
            tx_buf: tx_buf.as_ptr() as u64,
            rx_buf: rx_buf.as_mut_ptr() as u64,
            length: length as u32,
            speed_hz: speed,
            delay_usecs: 0,
            bits_per_word: bits,
            cs_change: 0,
            tx_nbits: 0,
            rx_nbits: 0,
            word_delay: 0,
            pad: 0,
        };

        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                SPI_IOC_MESSAGE_1 as _,
                &xfer as *const _ as usize,
            )
        };

        let _ = tx_buf;
        let _ = &rx_buf;

        if ret < 0 {
            return Err(format!(
                "SPI read failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let hex_bytes: Vec<String> = rx_buf.iter().map(|b| format!("0x{:02x}", b)).collect();
        let int_bytes: Vec<u64> = rx_buf.iter().map(|b| *b as u64).collect();

        let result = serde_json::json!({
            "device": dev_path,
            "bytes": int_bytes,
            "hex": hex_bytes,
            "length": rx_buf.len(),
        });
        let output = serde_json::to_string_pretty(&result).unwrap_or_default();
        Ok(ToolResult::silent(&output))
    }
}

#[cfg(target_os = "linux")]
use linux_impl::{
    linux_i2c_read, linux_i2c_scan, linux_i2c_write, linux_spi_read, linux_spi_transfer,
};

#[cfg(test)]
mod tests;

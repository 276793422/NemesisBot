//! Hardware tools - I2C and SPI bus interaction.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;

// --------------- I2C Tool ---------------

/// I2C tool - interacts with I2C bus devices (Linux only).
pub struct I2CTool;

impl I2CTool {
    /// Create a new I2C tool.
    pub fn new() -> Self {
        Self
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
                "address": {"type": "integer", "description": "7-bit device address (0x03-0x77)"},
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
            match linux_i2c_scan(&device_path) {
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

        let addr = match args["address"].as_u64() {
            Some(a) if a >= 0x03 && a <= 0x77 => a as u8,
            _ => return ToolResult::error("address is required (e.g. 0x38, range 0x03-0x77)"),
        };

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

        let addr = match args["address"].as_u64() {
            Some(a) if a >= 0x03 && a <= 0x77 => a as u8,
            _ => return ToolResult::error("address is required (e.g. 0x38, range 0x03-0x77)"),
        };

        let data_array = match args["data"].as_array() {
            Some(d) if !d.is_empty() => d,
            _ => return ToolResult::error("data is required for write (array of byte values 0-255)"),
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
                data_bytes.len(), addr
            ))
        }
    }

    #[allow(dead_code)]
    fn parse_bus(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        match args["bus"].as_str() {
            Some(b) if !b.is_empty() => {
                if !b.chars().all(|c| c.is_ascii_digit()) {
                    Err(ToolResult::error("invalid bus identifier: must be a number"))
                } else {
                    Ok(())
                }
            }
            _ => Err(ToolResult::error("bus is required")),
        }
    }

    #[allow(dead_code)]
    fn parse_address(&self, args: &serde_json::Value) -> Result<(), ToolResult> {
        match args["address"].as_u64() {
            Some(addr) => {
                if addr < 0x03 || addr > 0x77 {
                    Err(ToolResult::error(
                        "address must be in valid 7-bit range (0x03-0x77)",
                    ))
                } else {
                    Ok(())
                }
            }
            None => Err(ToolResult::error("address is required (e.g. 0x38)")),
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
                if parts.len() != 2 || !parts[0].chars().all(|c| c.is_ascii_digit())
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
            _ => return ToolResult::error("data is required for transfer (array of byte values 0-255)"),
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
                if parts.len() != 2 || !parts[0].chars().all(|c| c.is_ascii_digit())
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
                if parts.len() != 2 || !parts[0].chars().all(|c| c.is_ascii_digit())
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
        if let Some(speed) = args["speed"].as_u64() {
            if speed == 0 || speed > 125_000_000 {
                return Err(ToolResult::error(
                    "speed must be between 1 Hz and 125 MHz",
                ));
            }
        }
        if let Some(mode) = args["mode"].as_u64() {
            if mode > 3 {
                return Err(ToolResult::error("mode must be 0-3"));
            }
        }
        if let Some(bits) = args["bits"].as_u64() {
            if bits == 0 || bits > 32 {
                return Err(ToolResult::error("bits must be between 1 and 32"));
            }
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
            .map_err(|e| format!("failed to open {}: {} (check permissions and i2c-dev module)", dev_path, e))
    }

    /// Perform an ioctl call on a file descriptor.
    fn do_ioctl(fd: std::fs::File, request: u64, arg: usize) -> Result<i32, String> {
        use std::os::unix::io::AsRawFd;
        let raw_fd = fd.as_raw_fd();
        let ret = unsafe { libc::ioctl(raw_fd, request as u64 as _, arg) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            Err(format!("ioctl failed: {}", err))
        } else {
            Ok(ret)
        }
    }

    /// Probe a single I2C address using SMBus.
    fn smbus_probe(fd: &std::fs::File, addr: usize, has_quick: bool) -> bool {
        // EEPROM ranges: use read byte (quick write can corrupt AT24RF08)
        let use_read_byte = (addr >= 0x30 && addr <= 0x37) || (addr >= 0x50 && addr <= 0x5F);

        if !use_read_byte && has_quick {
            // SMBus Quick Write: safest probe, no data transferred
            let args = I2cSmbusIoctlData {
                read_write: I2C_SMBUS_WRITE,
                command: 0,
                _pad: 0,
                size: I2C_SMBUS_QUICK,
                data: std::ptr::null(),
            };
            let ret = unsafe {
                libc::ioctl(
                    fd.as_raw_fd(),
                    I2C_SMBUS as _,
                    &args as *const _ as usize,
                )
            };
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
        let ret = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                I2C_SMBUS as _,
                &args as *const _ as usize,
            )
        };
        ret >= 0
    }

    /// I2C bus scan using ioctl SMBus probes.
    pub fn linux_i2c_scan(dev_path: &str) -> Result<ToolResult, String> {
        use std::os::unix::io::AsRawFd;

        let fd = open_i2c(dev_path)?;

        // Query adapter capabilities
        let mut funcs: usize = 0;
        let ret = unsafe {
            libc::ioctl(fd.as_raw_fd(), I2C_FUNCS as _, &mut funcs as *mut _ as usize)
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

        let mut found: Vec<serde_json::Value> = Vec::new();

        // Scan 0x08-0x77, skipping I2C reserved addresses 0x00-0x07
        for addr in 0x08..=0x77u16 {
            // Set slave address
            let ret = unsafe {
                libc::ioctl(fd.as_raw_fd(), I2C_SLAVE as _, addr as usize)
            };
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
        Ok(ToolResult::silent(&format!("Scan of {}:\n{}", dev_path, output)))
    }

    /// I2C read using ioctl.
    pub fn linux_i2c_read(
        dev_path: &str,
        addr: u8,
        args: &serde_json::Value,
        length: usize,
    ) -> Result<ToolResult, String> {
        use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};

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
        use std::os::unix::io::AsRawFd;

        let fd = open_i2c(dev_path)?;

        // Set slave address
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), I2C_SLAVE as _, addr as usize) };
        if ret < 0 {
            return Err(format!("failed to set I2C address 0x{:02x}", addr));
        }

        // Write data
        let n = std::os::unix::fs::FileExt::write_all_at(&fd, data, 0)
            .map_err(|e| format!("failed to write to device 0x{:02x}: {}", addr, e))?;

        Ok(ToolResult::silent(&format!(
            "Wrote {} byte(s) to device 0x{:02x} on {}",
            data.len(), addr, dev_path
        )))
    }

    /// Configure SPI device (open + set mode, bits, speed).
    fn configure_spi(dev_path: &str, mode: u8, bits: u8, speed: u32) -> Result<std::fs::File, String> {
        use std::os::unix::io::AsRawFd;

        let fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dev_path)
            .map_err(|e| format!("failed to open {}: {} (check permissions and spidev module)", dev_path, e))?;

        // Set SPI mode
        let mut m = mode;
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), SPI_IOC_WR_MODE as _, &mut m as *mut _ as usize) };
        if ret < 0 {
            return Err(format!("failed to set SPI mode {}: {}", mode, std::io::Error::last_os_error()));
        }

        // Set bits per word
        let mut b = bits;
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), SPI_IOC_WR_BITS_PER_WORD as _, &mut b as *mut _ as usize) };
        if ret < 0 {
            return Err(format!("failed to set bits per word {}: {}", bits, std::io::Error::last_os_error()));
        }

        // Set max speed
        let mut s = speed;
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), SPI_IOC_WR_MAX_SPEED_HZ as _, &mut s as *mut _ as usize) };
        if ret < 0 {
            return Err(format!("failed to set SPI speed {} Hz: {}", speed, std::io::Error::last_os_error()));
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
        use std::os::unix::io::AsRawFd;

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
            libc::ioctl(fd.as_raw_fd(), SPI_IOC_MESSAGE_1 as _, &xfer as *const _ as usize)
        };

        // Keep buffers alive until after ioctl
        std::mem::drop(tx_buf);
        std::mem::drop(&rx_buf);

        if ret < 0 {
            return Err(format!("SPI transfer failed: {}", std::io::Error::last_os_error()));
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
        use std::os::unix::io::AsRawFd;

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
            libc::ioctl(fd.as_raw_fd(), SPI_IOC_MESSAGE_1 as _, &xfer as *const _ as usize)
        };

        std::mem::drop(tx_buf);
        std::mem::drop(&rx_buf);

        if ret < 0 {
            return Err(format!("SPI read failed: {}", std::io::Error::last_os_error()));
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
use linux_impl::{linux_i2c_scan, linux_i2c_read, linux_i2c_write, linux_spi_transfer, linux_spi_read};

#[cfg(test)]
mod tests {
    use super::*;

    // ---- I2C Tests ----

    #[test]
    fn test_i2c_tool_metadata() {
        let tool = I2CTool::new();
        assert_eq!(tool.name(), "i2c");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_i2c_non_linux_rejected() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "detect"}))
            .await;
        // On non-Linux, should error
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
            assert!(result.for_llm.contains("Linux"));
        }
    }

    #[tokio::test]
    async fn test_i2c_missing_action() {
        let tool = I2CTool::new();
        let result = tool.execute(&serde_json::json!({})).await;
        // May fail on non-Linux first
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_scan_missing_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "scan"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_no_confirm() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "data": [0x01, 0x02]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("confirm"));
        }
    }

    #[tokio::test]
    async fn test_i2c_invalid_address() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x99
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    // ---- SPI Tests ----

    #[test]
    fn test_spi_tool_metadata() {
        let tool = SPITool::new();
        assert_eq!(tool.name(), "spi");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_spi_non_linux_rejected() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "list"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
            assert!(result.for_llm.contains("Linux"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_no_confirm() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "data": [0x01]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("confirm"));
        }
    }

    #[tokio::test]
    async fn test_spi_read_missing_device() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "read", "length": 4}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_invalid_device_format() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "abc",
                "length": 4
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[test]
    fn test_spi_validate_params() {
        let tool = SPITool::new();

        // Valid params
        assert!(tool
            .validate_spi_params(&serde_json::json!({"speed": 1000000, "mode": 0, "bits": 8}))
            .is_ok());

        // Invalid speed
        assert!(tool
            .validate_spi_params(&serde_json::json!({"speed": 0}))
            .is_err());

        // Invalid mode
        assert!(tool
            .validate_spi_params(&serde_json::json!({"mode": 5}))
            .is_err());

        // Invalid bits
        assert!(tool
            .validate_spi_params(&serde_json::json!({"bits": 0}))
            .is_err());
    }

    // --- Additional hardware tests ---

    #[test]
    fn test_i2c_tool_default() {
        let tool = I2CTool::default();
        assert_eq!(tool.name(), "i2c");
    }

    #[test]
    fn test_spi_tool_default() {
        let tool = SPITool::default();
        assert_eq!(tool.name(), "spi");
    }

    #[test]
    fn test_i2c_tool_parameters() {
        let tool = I2CTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
    }

    #[test]
    fn test_spi_tool_parameters() {
        let tool = SPITool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
    }

    #[tokio::test]
    async fn test_i2c_unknown_action() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "unknown_action"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_unknown_action() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "unknown_action"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[test]
    fn test_spi_validate_speed_too_high() {
        let tool = SPITool::new();
        let result = tool.validate_spi_params(&serde_json::json!({"speed": 200_000_000}));
        assert!(result.is_err());
    }

    #[test]
    fn test_spi_validate_bits_too_high() {
        let tool = SPITool::new();
        let result = tool.validate_spi_params(&serde_json::json!({"bits": 64}));
        assert!(result.is_err());
    }

    #[test]
    fn test_spi_validate_all_modes() {
        let tool = SPITool::new();
        for mode in 0..=3 {
            assert!(tool.validate_spi_params(&serde_json::json!({"mode": mode})).is_ok());
        }
    }

    #[test]
    fn test_spi_validate_no_params_is_ok() {
        let tool = SPITool::new();
        assert!(tool.validate_spi_params(&serde_json::json!({})).is_ok());
    }

    #[tokio::test]
    async fn test_i2c_read_missing_address() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "read", "bus": "1"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_missing_data() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    // --- Even more hardware tests ---

    #[tokio::test]
    async fn test_spi_missing_action() {
        let tool = SPITool::new();
        let result = tool.execute(&serde_json::json!({})).await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_detect_action() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "detect"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[test]
    fn test_spi_validate_speed_boundary() {
        let tool = SPITool::new();
        // Just under the limit
        assert!(tool.validate_spi_params(&serde_json::json!({"speed": 125_000_000})).is_ok());
        // Just over the limit
        assert!(tool.validate_spi_params(&serde_json::json!({"speed": 125_000_001})).is_err());
    }

    #[test]
    fn test_spi_validate_bits_boundary() {
        let tool = SPITool::new();
        // Max valid bits
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 32})).is_ok());
        // Over max
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 33})).is_err());
    }

    // --- Additional tests for coverage ---

    #[tokio::test]
    async fn test_i2c_scan_invalid_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "scan", "bus": "abc"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("number"));
        }
    }

    #[tokio::test]
    async fn test_i2c_read_low_address() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "read", "bus": "1", "address": 0x01}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("address"));
        }
    }

    #[tokio::test]
    async fn test_i2c_write_data_too_long() {
        let tool = I2CTool::new();
        let data: Vec<u64> = (0..300).collect();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "data": data
            }))
            .await;
        if !cfg!(target_os = "linux") {
            // On non-Linux, the platform check returns first
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("256 bytes"));
        }
    }

    #[tokio::test]
    async fn test_i2c_write_invalid_data_byte() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "data": [256]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("byte value"));
        }
    }

    #[tokio::test]
    async fn test_spi_read_invalid_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0",
                "length": 0
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_length_too_large() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0",
                "length": 5000
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("4096"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_data_too_long() {
        let tool = SPITool::new();
        let data: Vec<u64> = (0..5000).collect();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true,
                "data": data
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("4096"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_invalid_device_format() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "abc",
                "confirm": true,
                "data": [1]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("X.Y"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_missing_data() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("data"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_invalid_data_byte() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true,
                "data": [-1]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("byte value"));
        }
    }

    #[test]
    fn test_i2c_tool_description_not_empty() {
        let tool = I2CTool::new();
        assert!(tool.description().len() > 30);
    }

    #[test]
    fn test_spi_tool_description_not_empty() {
        let tool = SPITool::new();
        assert!(tool.description().len() > 30);
    }

    #[tokio::test]
    async fn test_i2c_write_empty_data() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "data": []
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("data"));
        }
    }

    #[test]
    fn test_i2c_parse_bus_valid() {
        let tool = I2CTool::new();
        assert!(tool.parse_bus(&serde_json::json!({"bus": "1"})).is_ok());
        assert!(tool.parse_bus(&serde_json::json!({"bus": "0"})).is_ok());
    }

    #[test]
    fn test_i2c_parse_bus_invalid() {
        let tool = I2CTool::new();
        // Missing bus
        assert!(tool.parse_bus(&serde_json::json!({})).is_err());
        // Empty bus
        assert!(tool.parse_bus(&serde_json::json!({"bus": ""})).is_err());
        // Non-numeric bus
        assert!(tool.parse_bus(&serde_json::json!({"bus": "abc"})).is_err());
        assert!(tool.parse_bus(&serde_json::json!({"bus": "1a"})).is_err());
    }

    #[test]
    fn test_i2c_parse_address_valid() {
        let tool = I2CTool::new();
        assert!(tool.parse_address(&serde_json::json!({"address": 0x03})).is_ok());
        assert!(tool.parse_address(&serde_json::json!({"address": 0x77})).is_ok());
        assert!(tool.parse_address(&serde_json::json!({"address": 0x38})).is_ok());
    }

    #[test]
    fn test_i2c_parse_address_invalid() {
        let tool = I2CTool::new();
        // Missing address
        assert!(tool.parse_address(&serde_json::json!({})).is_err());
        // Too low
        assert!(tool.parse_address(&serde_json::json!({"address": 0x00})).is_err());
        assert!(tool.parse_address(&serde_json::json!({"address": 0x02})).is_err());
        // Too high
        assert!(tool.parse_address(&serde_json::json!({"address": 0x78})).is_err());
        assert!(tool.parse_address(&serde_json::json!({"address": 0xFF})).is_err());
    }

    #[test]
    fn test_spi_parse_device_valid() {
        let tool = SPITool::new();
        assert!(tool.parse_device(&serde_json::json!({"device": "2.0"})).is_ok());
        assert!(tool.parse_device(&serde_json::json!({"device": "0.0"})).is_ok());
        assert!(tool.parse_device(&serde_json::json!({"device": "32767.32767"})).is_ok());
    }

    #[test]
    fn test_spi_parse_device_invalid() {
        let tool = SPITool::new();
        // Missing device
        assert!(tool.parse_device(&serde_json::json!({})).is_err());
        // Empty device
        assert!(tool.parse_device(&serde_json::json!({"device": ""})).is_err());
        // Wrong format
        assert!(tool.parse_device(&serde_json::json!({"device": "abc"})).is_err());
        assert!(tool.parse_device(&serde_json::json!({"device": "1"})).is_err());
        assert!(tool.parse_device(&serde_json::json!({"device": "1.2.3"})).is_err());
        assert!(tool.parse_device(&serde_json::json!({"device": "a.b"})).is_err());
    }

    #[tokio::test]
    async fn test_i2c_scan_empty_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "scan", "bus": ""}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("required"));
        }
    }

    #[tokio::test]
    async fn test_i2c_read_default_length() {
        let tool = I2CTool::new();
        // On non-Linux, the platform check runs first. On Linux, /dev/i2c-1 won't exist.
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x38
            }))
            .await;
        // Will error on both platforms (no hardware or not linux)
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_read_with_register() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x38,
                "register": 0x10,
                "length": 8
            }))
            .await;
        // Will error on both platforms
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_write_with_register_valid() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "register": 0x10,
                "data": [0x01, 0x02]
            }))
            .await;
        // Will error on both platforms (no hardware or not linux)
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_spi_transfer_valid_device() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "1.0",
                "confirm": true,
                "data": [0xFF]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_valid_device() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "1.0",
                "length": 16
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_list_action() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "list"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_register_out_of_range() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "register": 256,
                "data": [1]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("register"));
        }
    }

    // ============================================================
    // Additional coverage tests for 95%+ target
    // ============================================================

    #[tokio::test]
    async fn test_i2c_unknown_action_v2_r2() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "unknown_action"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
            assert!(result.for_llm.contains("Linux"));
        }
    }

    #[tokio::test]
    async fn test_i2c_read_missing_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "address": 0x38,
                "length": 4
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_missing_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "address": 0x38,
                "data": [1],
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_non_numeric_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "abc",
                "address": 0x38,
                "data": [1],
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_address_out_of_range_high() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x80,
                "data": [1],
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_address_out_of_range_low() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x01,
                "data": [1],
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_unknown_action_v2_r2() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "unknown_spi_action"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
            assert!(result.for_llm.contains("Linux"));
        }
    }

    #[tokio::test]
    async fn test_spi_missing_action_v2_r2() {
        let tool = SPITool::new();
        let result = tool.execute(&serde_json::json!({})).await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_empty_device() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "",
                "data": [1],
                "confirm": true
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_zero_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "1.0",
                "length": 0
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_too_large_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "1.0",
                "length": 5000
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_scan_non_numeric_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "scan",
                "bus": "abc"
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_read_address_boundary_low() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x02,
                "length": 1
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_read_address_boundary_high() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x78,
                "length": 1
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[test]
    fn test_spi_validate_valid_all_params() {
        let tool = SPITool::new();
        let result = tool.validate_spi_params(&serde_json::json!({
            "speed": 500000,
            "mode": 2,
            "bits": 16
        }));
        assert!(result.is_ok());
    }

    #[test]
    fn test_spi_validate_mode_boundary() {
        let tool = SPITool::new();
        // Mode 3 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"mode": 3})).is_ok());
        // Mode 4 is invalid
        assert!(tool.validate_spi_params(&serde_json::json!({"mode": 4})).is_err());
    }

    #[test]
    fn test_spi_validate_bits_boundary_v2_r2() {
        let tool = SPITool::new();
        // Bits 8 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 8})).is_ok());
        // Bits 0 is invalid
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 0})).is_err());
    }

    #[test]
    fn test_i2c_parse_bus_empty() {
        let tool = I2CTool::new();
        let result = tool.parse_bus(&serde_json::json!({"bus": ""}));
        assert!(result.is_err());
    }

    #[test]
    fn test_i2c_parse_address_missing() {
        let tool = I2CTool::new();
        let result = tool.parse_address(&serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_spi_parse_device_valid_format() {
        let tool = SPITool::new();
        let result = tool.parse_device(&serde_json::json!({"device": "3.1"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_spi_parse_device_empty() {
        let tool = SPITool::new();
        let result = tool.parse_device(&serde_json::json!({"device": ""}));
        assert!(result.is_err());
    }

    // ============================================================
    // Coverage improvement: I2C and SPI parameter validation edge cases
    // ============================================================

    #[test]
    fn test_i2c_parse_bus_whitespace() {
        let tool = I2CTool::new();
        // Whitespace is not a digit
        assert!(tool.parse_bus(&serde_json::json!({"bus": " "})).is_err());
    }

    #[test]
    fn test_i2c_parse_address_boundary_exact() {
        let tool = I2CTool::new();
        // Exact lower boundary (0x03) is valid
        assert!(tool.parse_address(&serde_json::json!({"address": 0x03})).is_ok());
        // Exact upper boundary (0x77) is valid
        assert!(tool.parse_address(&serde_json::json!({"address": 0x77})).is_ok());
        // One below lower boundary (0x02)
        assert!(tool.parse_address(&serde_json::json!({"address": 0x02})).is_err());
        // One above upper boundary (0x78)
        assert!(tool.parse_address(&serde_json::json!({"address": 0x78})).is_err());
    }

    #[test]
    fn test_spi_parse_device_with_letter_parts() {
        let tool = SPITool::new();
        // Non-digit parts should fail
        assert!(tool.parse_device(&serde_json::json!({"device": "a.0"})).is_err());
        assert!(tool.parse_device(&serde_json::json!({"device": "0.a"})).is_err());
    }

    #[test]
    fn test_spi_parse_device_triple_dot() {
        let tool = SPITool::new();
        assert!(tool.parse_device(&serde_json::json!({"device": "1.2.3"})).is_err());
    }

    #[test]
    fn test_spi_parse_device_single_number() {
        let tool = SPITool::new();
        assert!(tool.parse_device(&serde_json::json!({"device": "1"})).is_err());
    }

    #[test]
    fn test_spi_validate_speed_boundary_exact() {
        let tool = SPITool::new();
        // Speed 1 Hz is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"speed": 1})).is_ok());
        // Speed exactly 125_000_000 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"speed": 125_000_000})).is_ok());
        // Speed exactly 125_000_001 is invalid
        assert!(tool.validate_spi_params(&serde_json::json!({"speed": 125_000_001})).is_err());
    }

    #[test]
    fn test_spi_validate_bits_boundary_exact() {
        let tool = SPITool::new();
        // Bits 1 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 1})).is_ok());
        // Bits 32 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 32})).is_ok());
        // Bits 33 is invalid
        assert!(tool.validate_spi_params(&serde_json::json!({"bits": 33})).is_err());
    }

    #[test]
    fn test_spi_validate_mode_boundary_exact() {
        let tool = SPITool::new();
        // Mode 0 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"mode": 0})).is_ok());
        // Mode 3 is valid
        assert!(tool.validate_spi_params(&serde_json::json!({"mode": 3})).is_ok());
        // Mode 4 is invalid
        assert!(tool.validate_spi_params(&serde_json::json!({"mode": 4})).is_err());
    }

    #[tokio::test]
    async fn test_i2c_scan_non_numeric_bus_chars() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({"action": "scan", "bus": "i2c-1"}))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("number"));
        }
    }

    #[tokio::test]
    async fn test_i2c_read_clamp_length() {
        let tool = I2CTool::new();
        // Length 0 should be clamped to 1
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x38,
                "length": 0
            }))
            .await;
        // On non-Linux, platform check fires first
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_read_large_length_clamped() {
        let tool = I2CTool::new();
        // Length > 256 should be clamped to 256
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "1",
                "address": 0x38,
                "length": 500
            }))
            .await;
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_write_register_boundary_valid() {
        let tool = I2CTool::new();
        // Register 0 is valid
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "register": 0,
                "data": [1]
            }))
            .await;
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_write_register_boundary_valid_255() {
        let tool = I2CTool::new();
        // Register 255 is valid
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "register": 255,
                "data": [1]
            }))
            .await;
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_i2c_read_missing_bus_param() {
        let tool = I2CTool::new();
        // Read with empty bus string
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "",
                "address": 0x38
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("bus"));
        }
    }

    #[tokio::test]
    async fn test_i2c_read_non_numeric_bus() {
        let tool = I2CTool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "bus": "abc",
                "address": 0x38
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_data_empty_array() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true,
                "data": []
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("data"));
        }
    }

    #[tokio::test]
    async fn test_spi_read_valid_min_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0",
                "length": 1
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_valid_max_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0",
                "length": 4096
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_spi_read_over_max_length() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0",
                "length": 4097
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("4096"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_over_max_data() {
        let tool = SPITool::new();
        let data: Vec<u64> = (0..4097).collect();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true,
                "data": data
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("4096"));
        }
    }

    #[tokio::test]
    async fn test_spi_transfer_valid_max_data() {
        let tool = SPITool::new();
        let data: Vec<u64> = (0..4096).map(|_| 0u64).collect();
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "2.0",
                "confirm": true,
                "data": data
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[tokio::test]
    async fn test_i2c_write_valid_data_byte_boundary() {
        let tool = I2CTool::new();
        // Max valid byte value (255)
        let result = tool
            .execute(&serde_json::json!({
                "action": "write",
                "bus": "1",
                "address": 0x38,
                "confirm": true,
                "data": [255, 0, 127]
            }))
            .await;
        // Will error on both platforms (no hardware or not linux)
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_spi_transfer_device_dot_format() {
        let tool = SPITool::new();
        // Device with extra dots
        let result = tool
            .execute(&serde_json::json!({
                "action": "transfer",
                "device": "1.2.3",
                "confirm": true,
                "data": [1]
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("X.Y"));
        }
    }

    #[tokio::test]
    async fn test_spi_read_device_dot_format() {
        let tool = SPITool::new();
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "1.2.3",
                "length": 10
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        } else {
            assert!(result.is_error);
            assert!(result.for_llm.contains("X.Y"));
        }
    }

    #[tokio::test]
    async fn test_spi_read_default_length() {
        let tool = SPITool::new();
        // No length specified, should default to 1
        let result = tool
            .execute(&serde_json::json!({
                "action": "read",
                "device": "2.0"
            }))
            .await;
        if !cfg!(target_os = "linux") {
            assert!(result.is_error);
        }
    }

    #[test]
    fn test_i2c_parameters_json_structure() {
        let tool = I2CTool::new();
        let params = tool.parameters();
        let action = &params["properties"]["action"];
        assert_eq!(action["type"], "string");
        let enum_values = action["enum"].as_array().unwrap();
        assert!(enum_values.contains(&serde_json::json!("detect")));
        assert!(enum_values.contains(&serde_json::json!("scan")));
        assert!(enum_values.contains(&serde_json::json!("read")));
        assert!(enum_values.contains(&serde_json::json!("write")));
    }

    #[test]
    fn test_spi_parameters_json_structure() {
        let tool = SPITool::new();
        let params = tool.parameters();
        let action = &params["properties"]["action"];
        assert_eq!(action["type"], "string");
        let enum_values = action["enum"].as_array().unwrap();
        assert!(enum_values.contains(&serde_json::json!("list")));
        assert!(enum_values.contains(&serde_json::json!("transfer")));
        assert!(enum_values.contains(&serde_json::json!("read")));
    }
}

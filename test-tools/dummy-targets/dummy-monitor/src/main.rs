//! dummy-monitor —— 紫微系统 T0.8 假想目标 B（MonitorState 状态翻转器）。
//!
//! 定位：M1 read_mem + M2 轮询/解码（utf16 消息）+ on_change 变化检测的**预埋靶子**。
//! 全局 `G_STATE`（repr(C)，stdout 打印地址）：
//! `MonitorState{magic:"MON1", status, tick, message:[u16;32]}`；
//! 每 interval-ms 循环 OK(0)→WARN(1)→ALERT(2) 翻转 + utf16 消息同步改写。
//!
//! stdout 契约：`PID=<pid>` / `STATE_ADDR=<hex>` / `INTERVAL_MS=<n>` / 每 tick
//! `tick=<n> status=<0|1|2>`。
//! Windows x86/x64 专用夹具（零依赖）。
#![allow(static_mut_refs)]
#![cfg(windows)]

use std::hint::black_box;

pub const MON_MAGIC: u32 = 0x4D4F_4E31; // "MON1"（小端字节序下内存呈现 31 4E 4F 4D）
pub const MSG_CAP: usize = 32;

/// 状态枚举值（profile / 测试断言按数值读）。
pub const ST_OK: u32 = 0;
pub const ST_WARN: u32 = 1;
pub const ST_ALERT: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MonitorState {
    pub magic: u32,
    pub status: u32,
    pub tick: u64,
    pub message: [u16; MSG_CAP],
}

static mut G_STATE: MonitorState = MonitorState {
    magic: MON_MAGIC,
    status: ST_OK,
    tick: 0,
    message: [0; MSG_CAP],
};

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
compile_error!("dummy-monitor 是紫微系统 Windows x86/x64 测试夹具");

/// 锚点预埋：状态翻转实现入口 8×0x90（byte_pattern 锚点，与 dummy-draw 同约定）。
#[inline(never)]
unsafe fn flip_status(state: *mut MonitorState) -> u32 {
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        options(nomem, nostack, preserves_flags)
    );
    let next = match (*state).status {
        ST_OK => ST_WARN,
        ST_WARN => ST_ALERT,
        _ => ST_OK,
    };
    (*state).status = next;
    next
}

unsafe fn set_message(state: *mut MonitorState, msg: &str) {
    let mut buf = [0u16; MSG_CAP];
    for (i, u) in msg.encode_utf16().enumerate().take(MSG_CAP - 1) {
        buf[i] = u;
    }
    (*state).message = buf;
}

fn main() {
    let mut interval_ms: u64 = 2000;
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix("--interval-ms=") {
            interval_ms = v.parse().unwrap_or(2000);
        }
    }

    let state = std::ptr::addr_of_mut!(G_STATE);
    unsafe {
        (*state).magic = MON_MAGIC;
        (*state).status = ST_OK;
        (*state).tick = 0;
        set_message(state, "MONITOR_OK");
    }

    println!("ZIWEI_DUMMY_MONITOR_V1");
    println!("PID={}", std::process::id());
    println!("STATE_ADDR={:#x}", std::ptr::addr_of!(G_STATE) as usize);
    println!("INTERVAL_MS={interval_ms}");

    loop {
        let (status, tick) = unsafe {
            flip_status(state);
            (*state).tick = (*state).tick.wrapping_add(1);
            let msg = match (*state).status {
                ST_OK => "MONITOR_OK",
                ST_WARN => "MONITOR_WARN",
                _ => "MONITOR_ALERT",
            };
            set_message(state, msg);
            black_box((*state).message);
            ((*state).status, (*state).tick)
        };
        println!("tick={tick} status={status}");
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
}

//! dummy-draw —— 紫微系统 T0.8 假想目标 A（GDI 心跳 + DataRecord 数组）。
//!
//! 定位：M1 注入/管道/read_mem、M2 轮询、M3 锚点扫描、M4 call_fn/hook 捕获、
//! M5 失效演练的**预埋靶子**。预埋要素（全部显式构造，供锚点/断言使用）：
//! - 数据区全局 `G_AREA`：`DataHeader{magic, layout_version, count, capacity}` +
//!   记录数组 union（V1/V2 两布局同址）；`--mutate` 运行期切 V2（字段重排 +
//!   label 16→24 + 尾部加字段 + 魔法值 +1）。
//! - 数据推送函数 `push_record`：入口 8×0x90 nop（byte_pattern 锚点）+
//!   `mov eax, MAGIC_V1` 原始字节（imm_const 锚点）。
//! - 两级调用链 `feed_render → render_record`（feed_render 入口同样 8×0x90）。
//! - M4 靶函数：`slow_op`（自旋 ms 毫秒后返回 ms——call_fn 超时看门狗靶）+
//!   `crash_op`（volatile 空指针写——VEH 救援靶）；两者入口同样 nopsled +
//!   独特 imm_const。
//! - 特征字符串：`ZIWEI_DUMMY_DRAW_V1`（mutate 时打 V2）等（str_ref 锚点）。
//! - GDI 心跳：每 tick CreateCompatibleDC/DeleteDC（gdi32 导入，import 锚点素材）。
//!
//! stdout 契约（供测试驱动解析）：`PID=<pid>` / `LAYOUT=<1|2>` / `MAGIC=<hex>` /
//! `DATA_ADDR=<hex>`（G_AREA 地址）；M4 起追加 `FN_<name>=<hex>`（靶函数
//! 运行时入口地址：push_record / feed_render / render_record / slow_op /
//! crash_op——call_fn 与 hook_set 直用，无需分析推导）。
//! Windows x86/x64 专用夹具（零依赖，`--target i686-pc-windows-msvc` 可编）。
#![allow(static_mut_refs)]
#![cfg(windows)]

use std::hint::black_box;

/// V1 数据区魔法（imm_const 锚点与内存头同值）。
pub const MAGIC_V1: u32 = 0x5A6B_7C8D;
/// V2（--mutate）数据区魔法：仅 +1，探测「软件更新后常量漂移」。
pub const MAGIC_V2: u32 = 0x5A6B_7C8E;
pub const RECORD_CAP: usize = 16;
pub const RECORD_COUNT: usize = 5;

/// V1 记录布局（profile 按 THIS 建模）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DataRecordV1 {
    pub id: u32,
    pub value: i64,
    pub label: [u8; 16],
    pub flags: u32,
}

/// V2（--mutate）记录布局：字段重排 + label 变长 + 尾部加字段。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DataRecordV2 {
    pub id: u32,
    pub flags: u32,
    pub value: i64,
    pub label: [u8; 24],
    pub reserved: u32,
}

/// 布局无关头（两个布局下地址与字段都一致——漂移只发生在记录数组）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DataHeader {
    pub magic: u32,
    pub layout_version: u32,
    pub count: u32,
    pub capacity: u32,
}

/// 记录数组 union：同一地址按 header.layout_version 解释。
#[repr(C)]
#[derive(Clone, Copy)]
pub union Records {
    pub v1: [DataRecordV1; RECORD_CAP],
    pub v2: [DataRecordV2; RECORD_CAP],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DataArea {
    pub header: DataHeader,
    pub records: Records,
}

static mut G_AREA: DataArea = DataArea {
    header: DataHeader {
        magic: MAGIC_V1,
        layout_version: 1,
        count: 0,
        capacity: RECORD_CAP as u32,
    },
    records: Records {
        v1: [DataRecordV1 {
            id: 0,
            value: 0,
            label: [0; 16],
            flags: 0,
        }; RECORD_CAP],
    },
};

/// 特征字符串（str_ref 锚点素材）。V1/V2 值都留在 .rdata；banner 按运行模式取用。
pub const STR_PRODUCT_V1: &str = "ZIWEI_DUMMY_DRAW_V1";
pub const STR_PRODUCT_V2: &str = "ZIWEI_DUMMY_DRAW_V2";
pub const STR_FEATURE: &str = "ZIWEI_DUMMY_DRAW_FEATURE:GDI_HEARTBEAT+DATARECORD";
pub const STR_CHAIN: &str = "feed_render/render_record";

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
compile_error!("dummy-draw 是紫微系统 Windows x86/x64 测试夹具");

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn DeleteDC(hdc: *mut core::ffi::c_void) -> i32;
}

/// GDI 心跳（自绘 workload 的最小替身：建/删兼容 DC）。
unsafe fn gdi_heartbeat() {
    let hdc = CreateCompatibleDC(std::ptr::null_mut());
    if !hdc.is_null() {
        DeleteDC(hdc);
    }
}

/// 数据推送实现 —— **锚点预埋函数**：
/// 入口 8×0x90 nop（byte_pattern `90 90 90 90 90 90 90 90`）+
/// `mov eax, 0x5A6B7C8D` 原始字节（B8 8D 7C 6B 5A，imm_const 锚点）。
/// 两段 .byte 在 x86/x64 都是合法且唯一的编码。
#[inline(never)]
unsafe fn push_record(area: *mut DataArea, idx: usize, delta: i64) {
    // T5.4 no-anchors 变体：sled + imm 锚编译期剥除（锚全 miss → stale 演练）。
    #[cfg(not(feature = "no-anchors"))]
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        ".byte 0xB8, 0x8D, 0x7C, 0x6B, 0x5A",
        out("eax") _,
        options(nomem, nostack, preserves_flags)
    );
    #[cfg(feature = "no-anchors")]
    let _ = 0u8; // 占位：保持语句位（函数体语义不受锚剥除影响）
    match (*area).header.layout_version {
        2 => {
            let rec = &mut (*area).records.v2[idx];
            rec.value = rec.value.wrapping_add(delta);
            rec.flags = rec.flags.wrapping_add(1);
        }
        _ => {
            let rec = &mut (*area).records.v1[idx];
            rec.value = rec.value.wrapping_add(delta);
            rec.flags = rec.flags.wrapping_add(1);
        }
    }
}

/// 两级链顶层（入口 8×0x90 nop）：feed_render → render_record。
#[inline(never)]
unsafe fn feed_render(area: *const DataArea, idx: usize) -> i64 {
    #[cfg(not(feature = "no-anchors"))]
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        options(nomem, nostack, preserves_flags)
    );
    render_record(area, idx)
}

/// 链第二级锚点常量（imm_const：B8 E0 FE 0F 0C——与数据魔法/其余锚不撞）。
pub const RENDER_ANCHOR: u32 = 0x0C0F_FEE0;

/// 两级链第二级：按当前布局读记录并派生一个值。
/// 入口 8×0x90 nop + `mov eax, RENDER_ANCHOR`（v1 hook 只挂 nop 窗口，
/// 第二级也必须自带 sled——M4 链式捕获的前提）。
#[inline(never)]
unsafe fn render_record(area: *const DataArea, idx: usize) -> i64 {
    #[cfg(not(feature = "no-anchors"))]
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        ".byte 0xB8, 0xE0, 0xFE, 0x0F, 0x0C",
        out("eax") _,
        options(nomem, nostack, preserves_flags)
    );
    #[cfg(feature = "no-anchors")]
    let _ = 0u8;
    match (*area).header.layout_version {
        2 => (*area).records.v2[idx].value ^ ((*area).header.magic as i64),
        _ => (*area).records.v1[idx].value + idx as i64,
    }
}

/// M4 超时靶：自旋约 ms 毫秒后返回 ms（call_fn 看门狗 + 单飞拒绝专用）。
/// 入口 8×0x90 + `mov eax, 0x00112233`（imm_const 锚 0x112233）。
#[inline(never)]
unsafe fn slow_op(ms: u32) -> u32 {
    #[cfg(not(feature = "no-anchors"))]
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        ".byte 0xB8, 0x33, 0x22, 0x11, 0x00",
        out("eax") _,
        options(nomem, nostack, preserves_flags)
    );
    #[cfg(feature = "no-anchors")]
    let _ = 0u8;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);
    while std::time::Instant::now() < deadline {
        std::hint::spin_loop();
    }
    ms
}

/// M4 SEH 靶：volatile 空指针写 → ACCESS_VIOLATION（payload VEH 救援专用，
/// 宿主必须存活）。入口 8×0x90 + `mov eax, 0x0BADF00D`（imm_const 锚）。
#[inline(never)]
unsafe fn crash_op(cookie: u32) -> u32 {
    #[cfg(not(feature = "no-anchors"))]
    core::arch::asm!(
        ".byte 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90",
        ".byte 0xB8, 0x0D, 0xF0, 0xAD, 0x0B",
        out("eax") _,
        options(nomem, nostack, preserves_flags)
    );
    #[cfg(feature = "no-anchors")]
    let _ = 0u8;
    let p = cookie as *mut u32;
    p.write_volatile(0xDEAD_BEEFu32);
    1
}

/// 函数地址（stdout 契约打印；call_fn/hook 直用）。
fn fn_addr_1(f: unsafe fn(u32) -> u32) -> usize {
    f as usize
}

fn fill_label(buf: &mut [u8], idx: usize) {
    let tag = format!("REC_{idx:02}");
    for (i, b) in tag.bytes().enumerate() {
        if i < buf.len() {
            buf[i] = b;
        }
    }
}

unsafe fn init_area(mutate: bool) {
    let area = std::ptr::addr_of_mut!(G_AREA);
    (*area).header.count = RECORD_COUNT as u32;
    (*area).header.capacity = RECORD_CAP as u32;
    if mutate {
        (*area).header.magic = MAGIC_V2;
        (*area).header.layout_version = 2;
        for i in 0..RECORD_COUNT {
            let mut rec = DataRecordV2 {
                id: i as u32,
                flags: 0x1000 | i as u32,
                value: 2000 + i as i64,
                label: [0; 24],
                reserved: 0xDEAD_BEEF,
            };
            fill_label(&mut rec.label, i);
            (*area).records.v2[i] = rec;
        }
    } else {
        (*area).header.magic = MAGIC_V1;
        (*area).header.layout_version = 1;
        for i in 0..RECORD_COUNT {
            let mut rec = DataRecordV1 {
                id: i as u32,
                value: 1000 + i as i64,
                label: [0; 16],
                flags: 0x0001 | (i as u32) << 8,
            };
            fill_label(&mut rec.label, i);
            (*area).records.v1[i] = rec;
        }
    }
}

fn main() {
    // T5.4 bake-mutate 变体：出厂即 V2 布局（"软件更新后"的 exe 形态）。
    let mut mutate = cfg!(feature = "bake-mutate");
    let mut tick_ms: u64 = 1000;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--mutate" => mutate = true,
            s => {
                if let Some(v) = s.strip_prefix("--tick-ms=") {
                    tick_ms = v.parse().unwrap_or(1000);
                }
            }
        }
    }

    unsafe { init_area(mutate) };

    // stdout 契约：测试驱动逐行解析（println! = LineWriter，管道下也逐行落）。
    let pid = std::process::id();
    let data_addr = std::ptr::addr_of!(G_AREA) as usize;
    println!("{}", if mutate { STR_PRODUCT_V2 } else { STR_PRODUCT_V1 });
    println!("{STR_FEATURE}");
    println!("{STR_CHAIN}");
    println!("PID={pid}");
    println!("LAYOUT={}", if mutate { 2 } else { 1 });
    println!("MAGIC={:#010x}", if mutate { MAGIC_V2 } else { MAGIC_V1 });
    println!("DATA_ADDR={data_addr:#x}");
    // M4 靶函数运行时入口（stdout 契约；call_fn/hook_set 直用）
    println!(
        "FN_PUSH_RECORD={:#x}",
        push_record as unsafe fn(*mut DataArea, usize, i64) as usize
    );
    println!(
        "FN_FEED_RENDER={:#x}",
        feed_render as unsafe fn(*const DataArea, usize) -> i64 as usize
    );
    println!(
        "FN_RENDER_RECORD={:#x}",
        render_record as unsafe fn(*const DataArea, usize) -> i64 as usize
    );
    println!("FN_SLOW_OP={:#x}", fn_addr_1(slow_op));
    println!("FN_CRASH_OP={:#x}", fn_addr_1(crash_op));

    let mut tick: u64 = 0;
    loop {
        tick += 1;
        unsafe {
            gdi_heartbeat();
            let idx = (tick % (RECORD_COUNT as u64 + 1)) as usize; // 0..=5（5 越界一档，防优化器断言假设）
            let idx = if idx >= RECORD_COUNT { 0 } else { idx };
            let delta = black_box(1 + (tick % 3) as i64);
            push_record(std::ptr::addr_of_mut!(G_AREA), idx, delta);
            let v = feed_render(std::ptr::addr_of!(G_AREA), idx);
            println!("tick={tick} idx={idx} delta={delta} feed={v}");
        }
        std::thread::sleep(std::time::Duration::from_millis(tick_ms));
    }
}

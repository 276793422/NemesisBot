//! ziwei-agent-dll 单测（BUG-199/200 回归）。
//!
//! 方法：**真执行** stub+common 汇编。RWX 页上装配与 `hook_set` 逐字节
//! 同源的 stub（经 [`crate::build_stub_x64`]），test driver 设置完整
//! 寄存器现场（rax/r10/r11 魔数 + rcx/rdx/r8/r9 实参）后调入，断言
//! 返回后寄存器原值回还与 emit 参数捕获。
//!
//! 串行：四测共享全局 RING/HOOK_META/CALL/VEH_HANDLE，用 TEST_LOCK 互斥
//! （项目 background_registry 同款纪律）。

use std::sync::atomic::Ordering;
use std::sync::Mutex;

use crate::{
    build_stub_x64, call_fn, hook_set, poll_events, unload_prepare, FlushInstructionCache, Sleep,
    VirtualAlloc, CALL, CONV_MSX64, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VEH_HANDLE,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(all(test, target_arch = "x86_64"))]
std::arch::global_asm!(
    ".balign 16",
    "ziwei_test_driver_x64:",
    // in（msx64）：rcx=stub rdx=A r8=B r9=C [rsp+0x28]=D（入口时 5 参偏移）
    // 现场：rax/r10/r11 置魔数，rcx/rdx/r8/r9 置 A/B/C/D，call stub。
    // stub 尾部 `ret` 消费 driver 的返回地址回到 `pop rbx`。
    "push rbx",
    "push rbx",                       // 双 push：stub 会写满 [rsp0-8..rsp0-0x30] 六槽
    "mov  rbx, rcx",                  // rbx = stub
    "mov  rcx, rdx",                  // rcx = A
    "mov  rdx, r8",                   // rdx = B
    "mov  r8,  r9",                   // r8  = C
    "mov  r9,  [rsp+0x38]",           // r9  = D（rsp0+0x28 = rsp+0x10+0x28）
    "movabs rax, 0xA5A5A5A5A5A5A5A5", // MAGIC_RAX
    "movabs r10, 0x0F0F0F0F0F0F0F0F",
    "movabs r11, 0x3C3C3C3C3C3C3C3C",
    "call rbx", // 进 stub（stub 视 [rsp]=返回地址）
    "pop  rbx",
    "pop  rbx",
    "ret", // 返回 rax（断言原值回还）
);

#[cfg(all(test, target_arch = "x86_64"))]
unsafe extern "C" {
    fn ziwei_test_driver_x64(stub: usize, a: u64, b: u64, c: u64, d: u64) -> u64;
    fn ziwei_hook_common_x64();
}

const MAGIC_RAX: u64 = 0xA5A5_A5A5_A5A5_A5A5;

/// RWX 页（fill 字节填充），返回基址。
unsafe fn rwx_page(fill: u8) -> usize {
    let p = unsafe {
        VirtualAlloc(
            std::ptr::null_mut(),
            0x10000,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_EXECUTE_READWRITE,
        )
    };
    assert!(!p.is_null(), "RWX 页分配");
    unsafe { std::ptr::write_bytes(p as *mut u8, fill, 0x10000) };
    p as usize
}

/// BUG-199：stub+common 全链真执行——返回后 rax 必须原值回还、
/// rcx/rdx/r8/r9 经 emit 捕获进事件环。
#[cfg(target_arch = "x86_64")]
#[test]
fn bug199_stub_preserves_rax_and_captures_args() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // resume 落点：全 0xC3 页 + 0x40 处（stub 尾跳过去即 ret 回 driver）
    let resume_page = unsafe { rwx_page(0xC3) };
    let common = ziwei_hook_common_x64 as unsafe extern "C" fn() as usize;
    let stub_bytes = build_stub_x64(424242, resume_page + 0x40, common);

    let stub_page = unsafe { rwx_page(0xCC) };
    unsafe {
        std::ptr::copy_nonoverlapping(stub_bytes.as_ptr(), stub_page as *mut u8, 64);
        FlushInstructionCache(std::ptr::null_mut(), stub_page as *const _, 64);
    }

    let rax = unsafe { ziwei_test_driver_x64(stub_page, 0x1111, 0x2222, 0x3333, 0x4444) };
    assert_eq!(
        rax, MAGIC_RAX,
        "BUG-199：stub 返回后 rax 必须原值回还（红=被 resume 指针覆盖）"
    );

    // 参数捕获：poll 出 id=424242 事件，regs == [A,B,C,D]
    let ev = poll_events(4096);
    assert!(ev.contains("\"id\":424242"), "事件在环：{ev}");
    assert!(
        ev.contains("\"regs\":[4369,8738,13107,17476]"),
        "rcx/rdx/r8/r9 原值捕获：{ev}"
    );
}

// ---------------------------------------------------------------------------
// BUG-200：unload 三关闸守门行为测（红=解析性——FreeLibraryAndExitThread
// 进程内不可测；绿=拒绝/还原/幂等/VEH 摘除四断言面）。
// ---------------------------------------------------------------------------

/// 关①：在途 call_fn → 拒绝卸载。
#[cfg(target_arch = "x86_64")]
#[test]
fn bug200_unload_rejects_in_flight_call() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    CALL.active.store(1, Ordering::Release);
    let r = unsafe { unload_prepare() };
    CALL.active.store(0, Ordering::Release); // 复位（测后清场）
    let e = r.expect_err("在途 call_fn 必须拒绝卸载");
    assert!(e.contains("在途"), "拒绝原因指向关①：{e}");
}

/// 关③ + 收尾：在场 hook 入口还原（E9→5×0x90）、二次 prepare 幂等 Ok(0)。
#[cfg(target_arch = "x86_64")]
#[test]
fn bug200_unload_restores_hooks_then_idempotent() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let page = unsafe { rwx_page(0x90) }; // 入口 5×0x90（hook_set 二次实核前提）
    let addr = page as u64 + 0x100;
    unsafe { hook_set(77, addr) }.expect("装 hook");
    assert_eq!(
        unsafe { std::ptr::read(addr as *const u8) },
        0xE9,
        "hook_set 后入口为 E9 蹦床"
    );

    let n = unsafe { unload_prepare() }.expect("unload 放行");
    assert_eq!(n, 1, "解除 1 个 hook");
    let mut cur = [0u8; 5];
    unsafe { std::ptr::copy_nonoverlapping(addr as *const u8, cur.as_mut_ptr(), 5) };
    assert!(
        cur.iter().all(|&b| b == 0x90),
        "入口已还原 5×0x90：{cur:02X?}"
    );

    let n2 = unsafe { unload_prepare() }.expect("二次 unload 放行");
    assert_eq!(n2, 0, "幂等：空表再 prepare = Ok(0)");
}

/// 收尾（VEH 摘除）+ 关②happy path（退出线程句柄收口）：call_fn(Sleep)
/// 注册 VEH → prepare 后 VEH_HANDLE 清零。
#[cfg(target_arch = "x86_64")]
#[test]
fn bug200_unload_removes_veh_after_call() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let sleep = Sleep as unsafe extern "system" fn(u32) as usize;
    let resp = unsafe { call_fn(sleep as u64, CONV_MSX64, [120, 0, 0, 0], 5000) };
    assert!(resp.contains("\"ok\":true"), "Sleep(120) 完成：{resp}");
    assert_ne!(VEH_HANDLE.load(Ordering::Acquire), 0, "call_fn 注册了 VEH");

    unsafe { unload_prepare() }.expect("unload 放行（关②等退出的线程即收口）");
    assert_eq!(VEH_HANDLE.load(Ordering::Acquire), 0, "VEH 已摘除");
}

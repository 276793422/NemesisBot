use super::*;

const OFF_ARCH: usize = 880;

#[test]
fn param_block_offsets_pinned() {
    let p = ZiweiParamBlock {
        magic: 0,
        proto: 0,
        fn_load_library_w: 0,
        fn_get_proc_address: 0,
        dll_path: [0; 260],
        init_name: [0; 16],
        token: [0; 64],
        pipe_name: [0; 128],
        arch: [0; 8],
    };
    let base = &p as *const _ as usize;
    // 同一栈对象内取字段地址差，仅做布局断言（无需 unsafe）。
    let off = |field: *const u8| field as usize - base;
    assert_eq!(off(p.dll_path.as_ptr() as *const u8), OFF_DLL_PATH);
    assert_eq!(off(p.init_name.as_ptr()), OFF_INIT_NAME);
    assert_eq!(off(p.arch.as_ptr()), OFF_ARCH);
    assert_eq!(PARAM_BLOCK_SIZE, 888);
}

#[test]
fn stub_sizes() {
    // BUG-20261103-342：stub 加 NULL 守卫（test/jz）后扩容——LoadLibraryW
    // 失败不再 call NULL 崩目标进程，而是守卫退码 4/5 诚实退出。
    assert_eq!(X64_STUB.len(), 98);
    assert_eq!(X86_STUB.len(), 53);
    // 关键偏移落在缓冲内
    assert!(X64_OFF_PARAM + 8 <= X64_STUB.len());
    assert!(X86_OFF_PARAM + 4 <= X86_STUB.len());
    // 每个立即数前的 opcode 钉死（movabs rcx=48 B9 / rax=48 B8 / rdx=48 BA；
    // push=68 / mov eax=B8）——写错偏移 = 注入野写。
    assert_eq!(
        &X64_STUB[X64_OFF_DLLPATH - 2..X64_OFF_DLLPATH],
        &[0x48, 0xB9]
    );
    assert_eq!(
        &X64_STUB[X64_OFF_LOADLIB - 2..X64_OFF_LOADLIB],
        &[0x48, 0xB8]
    );
    assert_eq!(&X64_STUB[X64_OFF_NAME - 2..X64_OFF_NAME], &[0x48, 0xBA]);
    assert_eq!(&X64_STUB[X64_OFF_GPA - 2..X64_OFF_GPA], &[0x48, 0xB8]);
    assert_eq!(&X64_STUB[X64_OFF_PARAM - 2..X64_OFF_PARAM], &[0x48, 0xB9]);
    assert_eq!(X86_STUB[X86_OFF_DLLPATH - 1], 0x68);
    assert_eq!(X86_STUB[X86_OFF_LOADLIB - 1], 0xB8);
    assert_eq!(X86_STUB[X86_OFF_NAME - 1], 0x68);
    assert_eq!(X86_STUB[X86_OFF_GPA - 1], 0xB8);
    assert_eq!(X86_STUB[X86_OFF_PARAM - 1], 0x68);
    // x86 GPA 参数序回归锁（2026-10-23 实证根因）：GetProcAddress(HMODULE,
    // LPCSTR) 右到左压栈——name（push imm32）先、hmod（push eax）后。写反
    // = GPA 拿参数块当模块句柄 → 恒 NULL → 远线程 call 0 崩。
    assert_eq!(X86_STUB[0x10], 0x68, "GPA 第二参（name）必须先压");
    assert_eq!(X86_STUB[0x15], 0x50, "GPA 第一参（hmod=eax）必须后压");
}

/// BUG-20261103-342 红证：两个 stub 都必须带「call 后 test/jz 守卫」，
/// 守卫失败路径退码 4（LoadLibraryW 失败）/ 5（GetProcAddress 失败）。
/// 旧 stub 无守卫：LoadLibraryW 失败（路径截断/依赖缺失/被拦截）→
/// GPA(NULL) → call NULL → 目标进程 0xC0000005 崩溃（第三方进程连带损毁）。
#[test]
fn stub_null_guards_and_fail_exit_codes_pinned() {
    // x64：LoadLibraryW call @0x18 → test rax,rax (48 85 C0) @0x1A → jz @0x1D
    assert_eq!(
        &X64_STUB[0x1A..0x1D],
        &[0x48, 0x85, 0xC0],
        "x64 load 后必须 test 守卫"
    );
    assert_eq!(
        &X64_STUB[0x1D..0x1F],
        &[0x74, 0x2F],
        "x64 load 失败 jz → load_fail@0x4E"
    );
    // x64：GPA call @0x36 → test @0x38 → jz @0x3B
    assert_eq!(
        &X64_STUB[0x38..0x3B],
        &[0x48, 0x85, 0xC0],
        "x64 GPA 后必须 test 守卫"
    );
    assert_eq!(
        &X64_STUB[0x3B..0x3D],
        &[0x74, 0x1B],
        "x64 GPA 失败 jz → gpa_fail@0x58"
    );
    // 退码尾巴：mov eax,4 @0x4E / mov eax,5 @0x58（B8 imm32 + add rsp,28h + ret）
    assert_eq!(
        &X64_STUB[0x4E..0x53],
        &[0xB8, 0x04, 0x00, 0x00, 0x00],
        "load_fail 退码 4"
    );
    assert_eq!(X64_STUB[0x57], 0xC3, "load_fail ret");
    assert_eq!(
        &X64_STUB[0x58..0x5D],
        &[0xB8, 0x05, 0x00, 0x00, 0x00],
        "gpa_fail 退码 5"
    );
    assert_eq!(X64_STUB[0x61], 0xC3, "gpa_fail ret");
    // jz 位移换算自洽：0x1D jz 终点 0x1F + 0x2F = 0x4E；0x3B jz 终点 0x3D + 0x1B = 0x58
    // x86：LoadLibraryW call @0x0A → test eax,eax (85 C0) @0x0C → jz @0x0E
    assert_eq!(
        &X86_STUB[0x0C..0x0E],
        &[0x85, 0xC0],
        "x86 load 后必须 test 守卫"
    );
    assert_eq!(
        &X86_STUB[0x0E..0x10],
        &[0x74, 0x19],
        "x86 load 失败 jz → load_fail@0x29"
    );
    // x86：GPA call @0x1B → test @0x1D → jz @0x1F
    assert_eq!(
        &X86_STUB[0x1D..0x1F],
        &[0x85, 0xC0],
        "x86 GPA 后必须 test 守卫"
    );
    assert_eq!(
        &X86_STUB[0x1F..0x21],
        &[0x74, 0x0E],
        "x86 GPA 失败 jz → gpa_fail@0x2F"
    );
    assert_eq!(
        &X86_STUB[0x29..0x2E],
        &[0xB8, 0x04, 0x00, 0x00, 0x00],
        "x86 load_fail 退码 4"
    );
    assert_eq!(X86_STUB[0x2E], 0xC3, "x86 load_fail ret");
    assert_eq!(
        &X86_STUB[0x2F..0x34],
        &[0xB8, 0x05, 0x00, 0x00, 0x00],
        "x86 gpa_fail 退码 5"
    );
    assert_eq!(X86_STUB[0x34], 0xC3, "x86 gpa_fail ret");
    // jz 位移换算自洽：0x0E jz 终点 0x10 + 0x19 = 0x29；0x1F jz 终点 0x21 + 0x0E = 0x2F
}

/// stub 退码 → 诚实错误映射（0=成功；4/5=守卫退码；其余≠0=ZiWeiInit 语义）。
#[test]
fn stub_exit_error_mapping() {
    assert!(stub_exit_error(0).is_none());
    assert!(matches!(
        stub_exit_error(4),
        Some(ZiweiAttachError::PayloadLoadFailed)
    ));
    assert!(matches!(
        stub_exit_error(5),
        Some(ZiweiAttachError::PayloadResolveFailed)
    ));
    assert!(matches!(
        stub_exit_error(3),
        Some(ZiweiAttachError::InitFailed(3))
    ));
    assert!(matches!(
        stub_exit_error(1),
        Some(ZiweiAttachError::InitFailed(1))
    ));
}

/// BUG-20261103-342：dll_path UTF-16 单位数 > 259 必须在 attach 前诚实
/// 拒绝——旧实现 take(259) 静默截断 → 目标内 LoadLibraryW 找不到文件 →
/// （旧 stub 无守卫）call NULL 崩目标进程。
#[test]
fn attach_inject_rejects_overlong_payload_path() {
    let base = std::env::temp_dir();
    let overlong = base.join(format!("ziwei_{}", "x".repeat(300)));
    let params = ZiweiAttachParams {
        pid: 0,
        dll_path: overlong,
        token: "t".to_string(),
        pipe_name: r"\\.\pipe\ZiWei_test".to_string(),
    };
    let err = match attach_inject(&params) {
        Err(e) => e,
        Ok(_) => panic!("超长路径必须诚实拒绝"),
    };
    assert!(
        matches!(err, ZiweiAttachError::DllPathTooLong(_)),
        "诚实报超长: {err}"
    );
}

#[test]
fn payload_machine_parse() {
    // 最小 PE 骨架：DOS 头 0x40 + e_lfanew=0x40 + PE 签名 + machine
    let mut pe = vec![0u8; 0x48];
    pe[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    pe[0x40..0x44].copy_from_slice(b"PE\0\0");
    pe[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
    assert_eq!(payload_machine(&pe).unwrap(), 0x8664);
    pe[0x44..0x46].copy_from_slice(&0x014Cu16.to_le_bytes());
    assert_eq!(payload_machine(&pe).unwrap(), 0x014C);
    assert!(payload_machine(&pe[..0x30]).is_err());
    pe[0x40] = b'X';
    assert!(payload_machine(&pe).is_err());
}

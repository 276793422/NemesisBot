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
    assert_eq!(X64_STUB.len(), 68);
    assert_eq!(X86_STUB.len(), 33);
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
    // LPCSTR) 右到左压栈——name（0x0C push imm32）先、hmod（0x11 push eax）
    // 后。写反 = GPA 拿参数块当模块句柄 → 恒 NULL+126 → 远线程 call 0 崩。
    assert_eq!(X86_STUB[0x0C], 0x68, "GPA 第二参（name）必须先压");
    assert_eq!(X86_STUB[0x11], 0x50, "GPA 第一参（hmod=eax）必须后压");
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

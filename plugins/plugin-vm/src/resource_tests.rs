//! [`super::resources`] / `plugin_vm_resource_get` 单测（外部文件——
//! 零内联纪律）。纯内存断言，不落盘、不 dlopen。

use super::*;
use crate::resources as res;

/// 脚本资源恒在场（include_str! 编译期硬依赖）：非空 + shell 脚本形态。
#[test]
fn image_script_always_embedded_and_nonempty() {
    let bytes = res::resource(res::RESOURCE_SCRIPT).expect("脚本资源必须恒在场");
    assert!(!bytes.is_empty(), "脚本资源不许为空");
    assert_eq!(bytes, res::IMAGE_SCRIPT.as_bytes());
    assert!(
        res::IMAGE_SCRIPT.starts_with("#!"),
        "镜像构建脚本必须是 shell 脚本（#! shebang）"
    );
}

/// guest 资源：构建时在场（cfg 发射）→ Some + 非空 ELF 形态；不在场 →
/// None。cfg 双臂都钉——改 build.rs 探测逻辑回归时这里红。
#[test]
fn guest_resource_presence_follows_embed_cfg() {
    #[cfg(embed_guest_x86_64)]
    {
        let bytes = res::resource(res::RESOURCE_GUEST_X86_64).expect("cfg 已发射则资源必须在");
        assert!(!bytes.is_empty());
        // musl 静态 ELF：魔数 0x7f 'E' 'L' 'F'。
        assert_eq!(&bytes[..4], b"\x7fELF", "x86_64 guest 必须是 ELF");
    }
    #[cfg(not(embed_guest_x86_64))]
    assert_eq!(
        res::resource(res::RESOURCE_GUEST_X86_64),
        None,
        "cfg 未发射则资源必须缺席（不许半内嵌）"
    );

    #[cfg(embed_guest_aarch64)]
    {
        let bytes = res::resource(res::RESOURCE_GUEST_AARCH64).expect("cfg 已发射则资源必须在");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[..4], b"\x7fELF", "aarch64 guest 必须是 ELF");
    }
    #[cfg(not(embed_guest_aarch64))]
    assert_eq!(res::resource(res::RESOURCE_GUEST_AARCH64), None);
}

/// C ABI 三返回码：ok 填指针 + 长度；guest 未内嵌 = ABSENT（区别于拼错
/// 名 UNKNOWN）；null 参数 = UNKNOWN（不 panic）。
#[test]
fn resource_get_abi_return_codes() {
    unsafe {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;

        let rc = plugin_vm_resource_get(
            c"build-vm-image.sh".as_ptr(),
            &mut ptr,
            &mut len,
        );
        assert_eq!(rc, RESOURCE_GET_OK);
        assert!(!ptr.is_null() && len > 0, "ok 时必须填 out");

        // 名未知 → UNKNOWN。
        assert_eq!(
            plugin_vm_resource_get(c"no/such/thing".as_ptr(), &mut ptr, &mut len),
            RESOURCE_GET_UNKNOWN
        );

        // ABSENT 臂按 cfg 分派验证（同一构建里两 guest 要么在场要么缺席）。
        let guest_rc = plugin_vm_resource_get(
            c"guest/x86_64/nemesis-guest".as_ptr(),
            &mut ptr,
            &mut len,
        );
        #[cfg(embed_guest_x86_64)]
        assert_eq!(guest_rc, RESOURCE_GET_OK, "cfg 已发射 → ABI 也必须 ok");
        #[cfg(not(embed_guest_x86_64))]
        assert_eq!(guest_rc, RESOURCE_GET_ABSENT, "cfg 未发射 → ABI 必须 ABSENT");

        // null 全家 → UNKNOWN，不 panic。
        assert_eq!(
            plugin_vm_resource_get(std::ptr::null(), &mut ptr, &mut len),
            RESOURCE_GET_UNKNOWN
        );
        assert_eq!(
            plugin_vm_resource_get(c"x".as_ptr(), std::ptr::null_mut(), &mut len),
            RESOURCE_GET_UNKNOWN
        );
    }
}

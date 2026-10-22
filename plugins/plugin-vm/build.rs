//! 构建期资源内嵌探测（2026-10-22 拍板：VM 资源全部内嵌进插件库本体，
//! 分发面只剩单个 plugin_vm.dll / libplugin_vm.so——不再有资源目录）。
//!
//! 职责：
//! 1. 探测 guest 双架构 ELF（`bin/bin_guest/<arch>/nemesis-guest`，由
//!    `scripts/build-guest.sh` 产出——**必须先于本插件构建**）；
//! 2. 在场 → `cargo:rustc-cfg=embed_guest_<arch>`（src/resources.rs 的
//!    include_bytes! 挂同款 cfg）；缺席 → 对应资源编译期不存在；
//! 3. `NMB_VM_REQUIRE_RESOURCES=1`（CI 与本地 release 打包脚本设）时
//!    缺席 = **构建失败**（打包无人值守，缺席资源放出去 = 用户下载后
//!    VM 用不了的残包——fail-closed）；ad-hoc 开发构建不设 env，缺席
//!    只注记（运行期宿主诚实报「资源未内嵌」）。
//!
//! 镜像构建脚本 `build-vm-image.sh` 是仓库内常驻文件，include_str! 直嵌
//! 无需探测（缺席 = 本文件编译不过，天然 fail-closed）。

use std::env;
use std::path::PathBuf;

fn main() {
    // build script cwd = 本包根（plugins/plugin-vm）；仓库根 = ../..。
    let repo_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()))
        .join("..")
        .join("..");

    // ---- guest 双架构探测 ----
    let require = env::var("NMB_VM_REQUIRE_RESOURCES").as_deref() == Ok("1");
    for (arch, cfg_name) in [
        ("x86_64", "embed_guest_x86_64"),
        ("aarch64", "embed_guest_aarch64"),
    ] {
        // 先声明 cfg（unstable 语法闸必须每次无条件发，cfg 未用不告警）。
        println!("cargo:rustc-check-cfg=cfg({cfg_name})");
        let elf = repo_root
            .join("bin")
            .join("bin_guest")
            .join(arch)
            .join("nemesis-guest");
        if elf.is_file() {
            println!("cargo:rustc-cfg={cfg_name}");
            println!("cargo:rerun-if-changed={}", elf.display());
        } else if require {
            panic!(
                "NMB_VM_REQUIRE_RESOURCES=1 但 guest ELF 不在场：{}——先跑 scripts/build-guest.sh \
                 （资源内嵌纪律：guest 构建必须先于 plugin-vm 构建，缺席资源不许出包）",
                elf.display()
            );
        } else {
            println!(
                "cargo:warning=plugin-vm: guest ELF 缺席（{}）——该架构资源不内嵌，运行期诚实报「资源未内嵌」（release 打包请设 NMB_VM_REQUIRE_RESOURCES=1）",
                elf.display()
            );
        }
    }

    // ---- 镜像构建脚本（include_str! 编译期硬依赖，此处只挂 rerun） ----
    println!(
        "cargo:rerun-if-changed={}",
        repo_root
            .join("plugins")
            .join("plugin-vm")
            .join("build-vm-image.sh")
            .display()
    );
}

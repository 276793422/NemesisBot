fn main() {
    #[cfg(target_os = "windows")]
    {
        cc::Build::new()
            .cpp(true)
            .warnings(false)
            .file("cpp/safe_ffi.cpp")
            .compile("safe_ffi");
    }
}

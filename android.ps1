# 1. 启用 CGO
$env:CGO_ENABLED = 1

# 2. 设置目标平台
$env:GOOS = "android"
$env:GOARCH = "arm64"

# 3. 指定 NDK 中的 C 编译器 (路径需根据你的 NDK 版本修改)
# 注意：这里需要指向 NDK 里的 clang，并指定最低 API 版本（如 21）
$NDK_PATH = "C:\Users\Zoo\AppData\Local\Android\Sdk\ndk\26.1.10909125"
$env:CC = "$NDK_PATH\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android21-clang.cmd"

# 4. 编译
.\build.bat android_nemesisbot.exe
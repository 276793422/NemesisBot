# ================================================
# NemesisBot Makefile
# ================================================
# 支持多平台构建、功能开关、清理等功能
#
# 使用示例：
#   make                    # 显示帮助信息
#   make build              # 构建当前平台
#   make build-windows      # 构建 Windows
#   make build-linux        # 构建 Linux
#   make build-all          # 构建所有平台
#   make build-with-desktop # 构建桌面UI版本
#   make rebuild            # 清理并重新构建
#   make clean              # 清理构建文件
#   make test               # 运行测试
#   make run                # 运行应用
#   make release            # 构建发布版本
# ================================================

# ================================================
# 项目配置
# ================================================
PROJECT_NAME := nemesisbot
BINARY_NAME := $(PROJECT_NAME)
BUILDDIR := build
VERSION := $(shell git describe --tags --abbrev=0 2>/dev/null || echo "0.0.0.1")
GIT_COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")
BUILD_TIME := $(shell date -u +"%Y/%m/%d %H:%M:%S UTC" 2>/dev/null || echo "unknown")
GO_VERSION := $(shell go version | awk '{print $$3}' | sed 's/go//' 2>/dev/null || echo "unknown")

# ================================================
# 构建标志
# ================================================
LDFLAGS := -X main.version=$(VERSION) \
           -X main.gitCommit=$(GIT_COMMIT) \
           -X main.buildTime=$(BUILD_TIME) \
           -X main.goVersion=$(GO_VERSION) \
           -s -w

# ================================================
# 平台配置
# ================================================
# 默认构建当前平台
GOOS := $(shell go env GOOS)
GOARCH := $(shell go env GOARCH)

# 平台特定的二进制文件名
ifeq ($(GOOS),windows)
    BINARY_EXT := .exe
else
    BINARY_EXT :=
endif

BINARY_OUTPUT := $(BINARY_NAME)$(BINARY_EXT)

# ================================================
# 支持的平台
# ================================================
PLATFORMS := linux/amd64 linux/arm64 linux/386 linux/arm \
             darwin/amd64 darwin/arm64 \
             windows/amd64 windows/386 windows/arm64 \
             android/arm64 android/arm android/386 android/amd64 \
             freebsd/amd64 openbsd/amd64

# Android NDK 配置
# 默认 NDK 路径（可通过环境变量或命令行覆盖）
NDK_PATH ?= $(ANDROID_NDK_HOME)
NDK_VERSION ?= 26
ANDROID_MIN_API ?= 21

# 根据主机平台确定 NDK 工具链前缀
ifeq ($(GOOS),windows)
    NDK_HOST := windows-x86_64
else ifeq ($(GOOS),darwin)
    NDK_HOST := darwin-x86_64
else
    NDK_HOST := linux-x86_64
endif

# ================================================
# 颜色输出（跨平台）
# ================================================
# 检测是否支持颜色
ifdef NO_COLOR
    COLOR_RESET :=
    COLOR_BOLD :=
    COLOR_GREEN :=
    COLOR_YELLOW :=
    COLOR_RED :=
    COLOR_BLUE :=
    COLOR_CYAN :=
else
    COLOR_RESET := \033[0m
    COLOR_BOLD := \033[1m
    COLOR_GREEN := \033[32m
    COLOR_YELLOW := \033[33m
    COLOR_RED := \033[31m
    COLOR_BLUE := \033[34m
    COLOR_CYAN := \033[36m
endif

# ================================================
# 辅助函数
# ================================================

# 打印信息
define print_info
	@echo "$(COLOR_CYAN)[INFO]$(COLOR_RESET) $(1)"
endef

# 打印成功
define print_success
	@echo "$(COLOR_GREEN)[OK]$(COLOR_RESET) $(1)"
endef

# 打印警告
define print_warning
	@echo "$(COLOR_YELLOW)[WARN]$(COLOR_RESET) $(1)"
endef

# 打印错误
define print_error
	@echo "$(COLOR_RED)[ERROR]$(COLOR_RESET) $(1)"
endef

# 打印章节标题
define print_section
	@echo "$(COLOR_BOLD)$(COLOR_BLUE)========================================$(COLOR_RESET)"
	@echo "$(COLOR_BOLD)$(COLOR_BLUE)  $(1)$(COLOR_RESET)"
	@echo "$(COLOR_BOLD)$(COLOR_BLUE)========================================$(COLOR_RESET)"
endef

# ================================================
# 主要目标
# ================================================

# 默认目标 - 显示帮助信息
.PHONY: all
all:
	@echo ""
	@echo "$(COLOR_BOLD)$(COLOR_CYAN)欢迎使用 NemesisBot 构建系统！$(COLOR_RESET)"
	@echo ""
	@echo "$(COLOR_INFO)快速开始:$(COLOR_RESET)"
	@echo "  $(COLOR_GREEN)make build$(COLOR_RESET)          - 构建当前平台"
	@echo "  $(COLOR_GREEN)make help$(COLOR_RESET)           - 查看所有可用命令"
	@echo ""
	@echo "$(COLOR_INFO)获取帮助:$(COLOR_RESET)"
	@echo "  运行 $(COLOR_YELLOW)make help$(COLOR_RESET) 查看完整的使用说明"
	@echo ""
	@echo "$(COLOR_INFO)常用命令:$(COLOR_RESET)"
	@echo "  make build-windows      - 构建 Windows"
	@echo "  make build-linux        - 构建 Linux"
	@echo "  make build-android      - 构建 Android"
	@echo "  make build-all          - 构建所有平台"
	@echo "  make test               - 运行测试"
	@echo "  make clean              - 清理构建文件"
	@echo ""
	@echo "💡 提示: 使用 $(COLOR_YELLOW)make help$(COLOR_RESET) 查看所有命令"
	@echo ""

# 便捷目标 - 直接构建
.PHONY: quick
quick: build
	@$(call print_success,"构建完成！")

# ================================================
# 构建目标
# ================================================

.PHONY: build
build: check-go
	@$(call print_section,"构建 $(PROJECT_NAME)")
	@$(call print_info,"项目信息：")
	@echo "  名称:    $(PROJECT_NAME)"
	@echo "  版本:    $(VERSION)"
	@echo "  提交:    $(GIT_COMMIT)"
	@echo "  构建时间: $(BUILD_TIME)"
	@echo "  Go版本:  $(GO_VERSION)"
	@echo "  平台:    $(GOOS)/$(GOARCH)"
	@echo ""
	@mkdir -p $(BUILDDIR)
	@$(call print_info,"开始构建...")
	@go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/$(BINARY_OUTPUT) ./nemesisbot/
	@if [ -f $(BUILDDIR)/$(BINARY_OUTPUT) ]; then \
		$(call print_success,"构建成功: $(BUILDDIR)/$(BINARY_OUTPUT)"; \
		ls -lh $(BUILDDIR)/$(BINARY_OUTPUT) | awk '{print "  大小: " $$5}'; \
	else \
		$(call print_error,"构建失败！"; \
		exit 1; \
	fi

.PHONY: build-verbose
build-verbose: check-go
	@$(call print_section,"详细构建模式")
	@$(call print_info,"使用 -v 标志进行详细输出")
	@mkdir -p $(BUILDDIR)
	@go build -v -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/$(BINARY_OUTPUT) ./nemesisbot/
	@$(call print_success,"构建完成: $(BUILDDIR)/$(BINARY_OUTPUT)")

# ================================================
# 带功能的构建
# ================================================

.PHONY: build-with-powershell
build-with-powershell: check-go
	@$(call print_section,"构建（支持 PowerShell）")
	@mkdir -p $(BUILDDIR)
	@$(call print_info,"启用 PowerShell curl 兼容性")
	@go build -tags powershell -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/$(BINARY_OUTPUT) ./nemesisbot/
	@$(call print_success,"构建完成: $(BUILDDIR)/$(BINARY_OUTPUT)")

.PHONY: build-with-desktop
build-with-desktop: check-go
	@$(call print_section,"构建（桌面 UI 版本）")
	@mkdir -p $(BUILDDIR)
	@$(call print_info,"启用桌面 UI")
	@go build -tags desktop -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/$(BINARY_OUTPUT) ./nemesisbot/
	@$(call print_success,"构建完成: $(BUILDDIR)/$(BINARY_OUTPUT)")

.PHONY: build-full-featured
build-full-featured: check-go
	@$(call print_section,"构建（全功能版本）")
	@mkdir -p $(BUILDDIR)
	@$(call print_info,"启用所有功能：PowerShell + Desktop")
	@go build -tags "powershell desktop" -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/$(BINARY_OUTPUT) ./nemesisbot/
	@$(call print_success,"构建完成: $(BUILDDIR)/$(BINARY_OUTPUT)")

# ================================================
# 跨平台构建
# ================================================

.PHONY: build-windows
build-windows: check-go
	@$(call print_section,"构建 Windows (amd64)")
	@mkdir -p $(BUILDDIR)/windows-amd64
	@GOOS=windows GOARCH=amd64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/windows-amd64/$(PROJECT_NAME).exe ./nemesisbot/
	@$(call print_success,"Windows amd64 构建完成")

.PHONY: build-windows-386
build-windows-386: check-go
	@$(call print_section,"构建 Windows (386)")
	@mkdir -p $(BUILDDIR)/windows-386
	@GOOS=windows GOARCH=386 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/windows-386/$(PROJECT_NAME).exe ./nemesisbot/
	@$(call print_success,"Windows 386 构建完成")

.PHONY: build-windows-arm64
build-windows-arm64: check-go
	@$(call print_section,"构建 Windows (ARM64)")
	@mkdir -p $(BUILDDIR)/windows-arm64
	@GOOS=windows GOARCH=arm64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/windows-arm64/$(PROJECT_NAME).exe ./nemesisbot/
	@$(call print_success,"Windows ARM64 构建完成")

.PHONY: build-linux
build-linux: check-go
	@$(call print_section,"构建 Linux (amd64)")
	@mkdir -p $(BUILDDIR)/linux-amd64
	@GOOS=linux GOARCH=amd64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/linux-amd64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Linux amd64 构建完成")

.PHONY: build-linux-arm64
build-linux-arm64: check-go
	@$(call print_section,"构建 Linux (ARM64)")
	@mkdir -p $(BUILDDIR)/linux-arm64
	@GOOS=linux GOARCH=arm64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/linux-arm64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Linux ARM64 构建完成")

.PHONY: build-linux-arm
build-linux-arm: check-go
	@$(call print_section,"构建 Linux (ARM)")
	@mkdir -p $(BUILDDIR)/linux-arm
	@GOOS=linux GOARCH=arm go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/linux-arm/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Linux ARM 构建完成")

.PHONY: build-darwin
build-darwin: check-go
	@$(call print_section,"构建 macOS (amd64)")
	@mkdir -p $(BUILDDIR)/darwin-amd64
	@GOOS=darwin GOARCH=amd64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/darwin-amd64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"macOS amd64 构建完成")

.PHONY: build-darwin-arm64
build-darwin-arm64: check-go
	@$(call print_section,"构建 macOS (ARM64/Apple Silicon)")
	@mkdir -p $(BUILDDIR)/darwin-arm64
	@GOOS=darwin GOARCH=arm64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/darwin-arm64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"macOS ARM64 构建完成")

.PHONY: build-freebsd
build-freebsd: check-go
	@$(call print_section,"构建 FreeBSD (amd64)")
	@mkdir -p $(BUILDDIR)/freebsd-amd64
	@GOOS=freebsd GOARCH=amd64 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/freebsd-amd64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"FreeBSD amd64 构建完成")

# ================================================
# Android 构建
# ================================================

.PHONY: check-android-ndk
check-android-ndk:
	@if [ -z "$(NDK_PATH)" ]; then \
		$(call print_warning,"ANDROID_NDK_HOME 环境变量未设置"; \
		$(call print_info,"使用默认 NDK 路径或通过 NDK_PATH 参数指定"; \
		$(call print_info,"示例: make build-android NDK_PATH=/path/to/ndk"; \
	fi
	@if [ ! -d "$(NDK_PATH)" ]; then \
		$(call print_error,"NDK 路径不存在: $(NDK_PATH)"; \
		$(call print_info,"请安装 Android NDK 或设置正确的路径"; \
		exit 1; \
	fi

.PHONY: build-android
build-android: check-android-ndk
	@$(call print_section,"构建 Android (ARM64)")
	@mkdir -p $(BUILDDIR)/android-arm64
	@$(call print_info,"NDK 路径: $(NDK_PATH)")
	@$(call print_info,"最低 API 版本: android$(ANDROID_MIN_API)")
	@$(call print_info,"工具链: $(NDK_HOST)")
	@CGO_ENABLED=1 \
	 GOOS=android \
	 GOARCH=arm64 \
	 CC="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/aarch64-linux-android$(ANDROID_MIN_API)-clang" \
	 CXX="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/aarch64-linux-android$(ANDROID_MIN_API)-clang++" \
	 AR="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/llvm-ar" \
	 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/android-arm64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Android ARM64 构建完成"

.PHONY: build-android-arm64
build-android-arm64: check-android-ndk
	@$(call print_section,"构建 Android (ARM64)")
	@mkdir -p $(BUILDDIR)/android-arm64
	@$(call print_info,"NDK 路径: $(NDK_PATH)")
	@CGO_ENABLED=1 \
	 GOOS=android \
	 GOARCH=arm64 \
	 CC="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/aarch64-linux-android$(ANDROID_MIN_API)-clang" \
	 CXX="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/aarch64-linux-android$(ANDROID_MIN_API)-clang++" \
	 AR="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/llvm-ar" \
	 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/android-arm64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Android ARM64 构建完成"

.PHONY: build-android-arm
build-android-arm: check-android-ndk
	@$(call print_section,"构建 Android (ARM)")
	@mkdir -p $(BUILDDIR)/android-arm
	@$(call print_info,"NDK 路径: $(NDK_PATH)")
	@CGO_ENABLED=1 \
	 GOOS=android \
	 GOARCH=arm \
	 CC="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/armv7a-linux-androideabi$(ANDROID_MIN_API)-clang" \
	 CXX="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/armv7a-linux-androideabi$(ANDROID_MIN_API)-clang++" \
	 AR="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/llvm-ar" \
	 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/android-arm/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Android ARM 构建完成"

.PHONY: build-android-386
build-android-386: check-android-ndk
	@$(call print_section,"构建 Android (x86)")
	@mkdir -p $(BUILDDIR)/android-386
	@$(call print_info,"NDK 路径: $(NDK_PATH)")
	@CGO_ENABLED=1 \
	 GOOS=android \
	 GOARCH=386 \
	 CC="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/i686-linux-android$(ANDROID_MIN_API)-clang" \
	 CXX="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/i686-linux-android$(ANDROID_MIN_API)-clang++" \
	 AR="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/llvm-ar" \
	 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/android-386/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Android x86 构建完成"

.PHONY: build-android-amd64
build-android-amd64: check-android-ndk
	@$(call print_section,"构建 Android (x86_64)")
	@mkdir -p $(BUILDDIR)/android-amd64
	@$(call print_info,"NDK 路径: $(NDK_PATH)")
	@CGO_ENABLED=1 \
	 GOOS=android \
	 GOARCH=amd64 \
	 CC="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/x86_64-linux-android$(ANDROID_MIN_API)-clang" \
	 CXX="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/x86_64-linux-android$(ANDROID_MIN_API)-clang++" \
	 AR="$(NDK_PATH)/toolchains/llvm/prebuilt/$(NDK_HOST)/bin/llvm-ar" \
	 go build -ldflags "$(LDFLAGS)" -o $(BUILDDIR)/android-amd64/$(PROJECT_NAME) ./nemesisbot/
	@$(call print_success,"Android x86_64 构建完成"

# ================================================
# 批量构建
# ================================================

.PHONY: build-all
build-all: check-go
	@$(call print_section,"构建所有平台")
	@$(call print_info,"这将需要一些时间..."
	@echo ""
	@$(MAKE) --no-print-directory build-windows
	@$(MAKE) --no-print-directory build-linux
	@$(MAKE) --no-print-directory build-darwin
	@$(MAKE) --no-print-directory build-freebsd
	@$(call print_info,"Android 构建需要 NDK，跳过..."
	@echo ""
	@$(call print_success,"所有平台构建完成！"
	@$(call print_info,"查看 $(BUILDDIR)/ 目录"
	@ls -lh $(BUILDDIR)/*/*/ 2>/dev/null || true

.PHONY: build-all-with-android
build-all-with-android: check-go check-android-ndk
	@$(call print_section,"构建所有平台（包括 Android）"
	@$(call print_info,"这将需要一些时间..."
	@echo ""
	@$(MAKE) --no-print-directory build-windows
	@$(MAKE) --no-print-directory build-linux
	@$(MAKE) --no-print-directory build-darwin
	@$(MAKE) --no-print-directory build-freebsd
	@$(MAKE) --no-print-directory build-android-all
	@echo ""
	@$(call print_success,"所有平台构建完成！"
	@$(call print_info,"查看 $(BUILDDIR)/ 目录"
	@ls -lh $(BUILDDIR)/*/*/ 2>/dev/null || true

.PHONY: build-windows-all
build-windows-all: check-go
	@$(call print_section,"构建所有 Windows 平台")
	@$(MAKE) --no-print-directory build-windows
	@$(MAKE) --no-print-directory build-windows-386
	@$(MAKE) --no-print-directory build-windows-arm64
	@$(call print_success,"所有 Windows 平台构建完成"

.PHONY: build-linux-all
build-linux-all: check-go
	@$(call print_section,"构建所有 Linux 平台")
	@$(MAKE) --no-print-directory build-linux
	@$(MAKE) --no-print-directory build-linux-arm64
	@$(MAKE) --no-print-directory build-linux-arm
	@$(call print_success,"所有 Linux 平台构建完成"

.PHONY: build-darwin-all
build-darwin-all: check-go
	@$(call print_section,"构建所有 macOS 平台")
	@$(MAKE) --no-print-directory build-darwin
	@$(MAKE) --no-print-directory build-darwin-arm64
	@$(call print_success,"所有 macOS 平台构建完成"

.PHONY: build-android-all
build-android-all: check-android-ndk
	@$(call print_section,"构建所有 Android 平台")
	@$(call print_info,"NDK 路径: $(NDK_PATH)"
	@$(MAKE) --no-print-directory build-android-arm64
	@$(MAKE) --no-print-directory build-android-arm
	@$(MAKE) --no-print-directory build-android-386
	@$(MAKE) --no-print-directory build-android-amd64
	@$(call print_success,"所有 Android 平台构建完成"

# ================================================
# 重新构建
# ================================================

.PHONY: rebuild
rebuild: clean build
	@$(call print_success,"重新构建完成"

.PHONY: rebuild-windows
rebuild-windows: clean build-windows
	@$(call print_success,"Windows 版本重新构建完成"

.PHONY: rebuild-linux
rebuild-linux: clean build-linux
	@$(call print_success,"Linux 版本重新构建完成"

# ================================================
# 清理目标
# ================================================

.PHONY: clean
clean:
	@$(call print_section,"清理构建文件")
	@if [ -d "$(BUILDDIR)" ]; then \
		rm -rf $(BUILDDIR); \
		$(call print_success,"已删除 $(BUILDDIR)/ 目录"; \
	else \
		$(call print_info,"没有需要清理的构建文件"; \
	fi
	@rm -f $(PROJECT_NAME) $(PROJECT_NAME).exe
	@$(call print_success,"清理完成"

.PHONY: clean-all
clean-all:
	@$(call print_section,"清理所有生成文件")
	@rm -rf $(BUILDDIR)
	@rm -f $(PROJECT_NAME) $(PROJECT_NAME).exe
	@find . -name "*_test.go" -type f -exec dirname {} \; | xargs -I {} sh -c 'rm -f {}/*.test 2>/dev/null || true'
	@find . -name "*.out" -delete 2>/dev/null || true
	@$(call print_success,"所有生成文件已清理"

.PHONY: clean-cache
clean-cache:
	@$(call print_section,"清理 Go 缓存")
	@go clean -cache -testcache 2>/dev/null || true
	@$(call print_success,"Go 缓存已清理"

# ================================================
# 测试目标
# ================================================

.PHONY: test
test: check-go
	@$(call print_section,"运行测试")
	@go test -v ./... 2>&1 | tee test_output.log
	@$(call print_success,"测试完成"

.PHONY: test-short
test-short: check-go
	@$(call print_section,"运行快速测试")
	@go test -short -v ./...
	@$(call print_success,"快速测试完成"

.PHONY: test-cover
test-cover: check-go
	@$(call print_section,"运行测试并生成覆盖率报告")
	@go test -coverprofile=coverage.out -covermode=atomic ./...
	@go tool cover -html=coverage.out -o coverage.html
	@$(call print_success,"覆盖率报告已生成: coverage.html"

.PHONY: test-race
test-race: check-go
	@$(call print_section,"运行竞态检测测试")
	@go test -race -v ./...
	@$(call print_success,"竞态检测测试完成"

.PHONY: test-module
test-module: check-go
	@$(call print_section,"测试特定模块")
	@$(call print_info,"用法: make test-module MODULE=./module/agent"
	@if [ -z "$(MODULE)" ]; then \
		$(call print_error,"请指定 MODULE 参数"; \
		echo "示例: make test-module MODULE=./module/agent"; \
		exit 1; \
	fi
	@go test -v $(MODULE)
	@$(call print_success,"模块 $(MODULE) 测试完成"

# ================================================
# 运行目标
# ================================================

.PHONY: run
run: build
	@$(call print_section,"运行 $(PROJECT_NAME)"
	@$(call print_info,"启动网关服务..."
	@./$(BUILDDIR)/$(BINARY_OUTPUT) gateway

.PHONY: run-dev
run-dev: check-go
	@$(call print_section,"开发模式运行")
	@$(call print_info,"使用 'go run' 直接运行（无需构建）"
	@go run ./nemesisbot/ gateway

.PHONY: run-local
run-local: build
	@$(call print_section,"本地模式运行")
	@$(call print_info,"使用 --local 参数运行"
	@./$(BUILDDIR)/$(BINARY_OUTPUT) --local gateway

# ================================================
# 安装目标
# ================================================

.PHONY: install
install: check-go
	@$(call print_section,"安装 $(PROJECT_NAME)")
	@go install -ldflags "$(LDFLAGS)" ./nemesisbot/
	@$(call print_success,"安装完成: $(shell go env GOPATH)/bin/$(PROJECT_NAME)$(BINARY_EXT)"

.PHONY: uninstall
uninstall:
	@$(call print_section,"卸载 $(PROJECT_NAME)")
	@rm -f $$(go env GOPATH)/bin/$(PROJECT_NAME) $$(go env GOPATH)/bin/$(PROJECT_NAME).exe
	@$(call print_success,"卸载完成"

# ================================================
# 发布目标
# ================================================

.PHONY: release
release: clean build-all
	@$(call print_section,"创建发布包")
	@mkdir -p release
	@cd $(BUILDDIR)/windows-amd64 && zip -q -r ../../release/$(PROJECT_NAME)-$(VERSION)-windows-amd64.zip $(PROJECT_NAME).exe
	@cd $(BUILDDIR)/linux-amd64 && tar -czf ../../release/$(PROJECT_NAME)-$(VERSION)-linux-amd64.tar.gz $(PROJECT_NAME)
	@cd $(BUILDDIR)/darwin-amd64 && tar -czf ../../release/$(PROJECT_NAME)-$(VERSION)-darwin-amd64.tar.gz $(PROJECT_NAME)
	@$(call print_success,"发布包已创建: release/"
	@ls -lh release/

.PHONY: release-checksums
release-checksums: release
	@$(call print_section,"生成校验和"
	@cd release && \
		sha256sum *.zip *.tar.gz > SHA256SUMS.txt && \
		md5sum *.zip *.tar.gz > MD5SUMS.txt
	@$(call print_success,"校验和已生成: release/SHA256SUMS.txt"

# ================================================
# 依赖管理
# ================================================

.PHONY: deps
deps: check-go
	@$(call print_section,"安装依赖")
	@go mod download
	@go mod tidy
	@$(call print_success,"依赖安装完成"

.PHONY: deps-update
deps-update: check-go
	@$(call print_section,"更新依赖")
	@go get -u ./...
	@go mod tidy
	@$(call print_success,"依赖更新完成"

.PHONY: deps-verify
deps-verify: check-go
	@$(call print_section,"验证依赖"
	@go mod verify
	@$(call print_success,"依赖验证完成"

# ================================================
# 代码质量
# ================================================

.PHONY: fmt
fmt: check-go
	@$(call print_section,"格式化代码")
	@go fmt ./...
	@$(call print_success,"代码格式化完成"

.PHONY: vet
vet: check-go
	@$(call print_section,"代码检查")
	@go vet ./...
	@$(call print_success,"代码检查完成"

.PHONY: lint
lint: check-go
	@$(call print_section,"代码 Lint 检查")
	@if command -v golangci-lint >/dev/null 2>&1; then \
		golangci-lint run ./...; \
		$(call print_success,"Lint 检查完成"; \
	else \
		$(call print_warning,"golangci-lint 未安装，跳过 Lint 检查"; \
		$(call print_info,"安装: go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest"; \
	fi

.PHONY: fix
fix: check-go
	@$(call print_section,"自动修复代码问题"
	@go fix ./...
	@$(call print_success,"代码修复完成"

# ================================================
# 工具链
# ================================================

.PHONY: tools
tools: check-go
	@$(call print_section,"安装开发工具"
	@go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
	@go install golang.org/x/tools/cmd/goimports@latest
	@$(call print_success,"开发工具安装完成"

.PHONY: check-go
check-go:
	@command -v go >/dev/null 2>&1 || { \
		$(call print_error,"Go 未安装或不在 PATH 中"; \
		echo "请安装 Go: https://golang.org/dl/"; \
		exit 1; \
	}
	@$(eval GO_VERSION_CHECK=$(shell go version | awk '{print $$3}' | sed 's/go/'))
	@$(call print_info,"Go 版本: $(GO_VERSION_CHECK)")

# ================================================
# 信息显示
# ================================================

.PHONY: info
info:
	@$(call print_section,"项目信息"
	@echo "项目名称:   $(PROJECT_NAME)"
	@echo "当前版本:   $(VERSION)"
	@echo "Git 提交:   $(GIT_COMMIT)"
	@echo "构建时间:   $(BUILD_TIME)"
	@echo "Go 版本:    $(GO_VERSION)"
	@echo "构建目录:   $(BUILDDIR)"
	@echo "当前平台:   $(GOOS)/$(GOARCH)"
	@echo ""
	@echo "支持的平台:"
	@for platform in $(PLATFORMS); do \
		echo "  - $$platform"; \
	done

.PHONY: version
version:
	@echo "$(VERSION)"

.PHONY: help
help:
	@echo "$(COLOR_BOLD)$(COLOR_CYAN)NemesisBot Makefile 使用说明$(COLOR_RESET)"
	@echo ""
	@echo "$(COLOR_GREEN)主要目标:$(COLOR_RESET)"
	@echo "  make                  - 构建当前平台"
	@echo "  make build            - 构建当前平台"
	@echo "  make rebuild          - 清理并重新构建"
	@echo "  make clean            - 清理构建文件"
	@echo ""
	@echo "$(COLOR_GREEN)功能构建:$(COLOR_RESET)"
	@echo "  make build-with-powershell  - 构建支持 PowerShell 的版本"
	@echo "  make build-with-desktop     - 构建桌面 UI 版本"
	@echo "  make build-full-featured    - 构建全功能版本"
	@echo ""
	@echo "$(COLOR_GREEN)跨平台构建:$(COLOR_RESET)"
	@echo "  make build-windows          - 构建 Windows (amd64)"
	@echo "  make build-windows-386      - 构建 Windows (32位)"
	@echo "  make build-windows-arm64    - 构建 Windows (ARM64)"
	@echo "  make build-linux            - 构建 Linux (amd64)"
	@echo "  make build-linux-arm64      - 构建 Linux (ARM64)"
	@echo "  make build-darwin            - 构建 macOS (Intel)"
	@echo "  make build-darwin-arm64      - 构建 macOS (Apple Silicon)"
	@echo "  make build-android          - 构建 Android (ARM64, 需要 NDK)"
	@echo "  make build-all              - 构建所有平台（不含 Android）"
	@echo "  make build-all-with-android - 构建所有平台（含 Android）"
	@echo ""
	@echo "$(COLOR_CYAN)Android 构建:$(COLOR_RESET)"
	@echo "  make build-android-arm64    - Android ARM64"
	@echo "  make build-android-arm      - Android ARM (32位)"
	@echo "  make build-android-386      - Android x86 (32位)"
	@echo "  make build-android-amd64    - Android x86_64"
	@echo "  make build-android-all      - 所有 Android 平台"
	@echo "  环境变量: NDK_PATH=/path/to/ndk  ANDROID_MIN_API=21"
	@echo ""
	@echo "$(COLOR_GREEN)测试:$(COLOR_RESET)"
	@echo "  make test                   - 运行所有测试"
	@echo "  make test-short             - 运行快速测试"
	@echo "  make test-cover             - 生成覆盖率报告"
	@echo "  make test-race              - 竞态检测"
	@echo "  make test-module MODULE=... - 测试特定模块"
	@echo ""
	@echo "$(COLOR_GREEN)运行:$(COLOR_RESET)"
	@echo "  make run                    - 构建并运行"
	@echo "  make run-dev                - 开发模式运行"
	@echo "  make run-local              - 本地模式运行"
	@echo ""
	@echo "$(COLOR_GREEN)依赖管理:$(COLOR_RESET)"
	@echo "  make deps                   - 安装依赖"
	@echo "  make deps-update            - 更新依赖"
	@echo "  make deps-verify            - 验证依赖"
	@echo ""
	@echo "$(COLOR_GREEN)代码质量:$(COLOR_RESET)"
	@echo "  make fmt                    - 格式化代码"
	@echo "  make vet                    - 代码检查"
	@echo "  make lint                   - Lint 检查"
	@echo "  make fix                    - 自动修复问题"
	@echo ""
	@echo "$(COLOR_GREEN)发布:$(COLOR_RESET)"
	@echo "  make release                - 创建发布包"
	@echo "  make release-checksums      - 生成校验和"
	@echo ""
	@echo "$(COLOR_GREEN)其他:$(COLOR_RESET)"
	@echo "  make install                - 安装到 GOPATH/bin"
	@echo "  make uninstall              - 卸载"
	@echo "  make tools                  - 安装开发工具"
	@echo "  make info                   - 显示项目信息"
	@echo "  make version                - 显示版本号"
	@echo "  make help                   - 显示此帮助"
	@echo ""
	@echo "$(COLOR_YELLOW)示例:$(COLOR_RESET)"
	@echo "  make build-windows && make build-linux"
	@echo "  make test MODULE=./module/agent"
	@echo "  make build-with-powershell GOOS=linux GOARCH=amd64"
	@echo ""

# ================================================
# Phony 声明
# ================================================
.PHONY: all build rebuild clean clean-all clean-cache
.PHONY: test test-short test-cover test-race test-module
.PHONY: run run-dev run-local install uninstall
.PHONY: fmt vet lint fix
.PHONY: deps deps-update deps-verify
.PHONY: tools check-go info version help
.PHONY: check-android-ndk
.PHONY: build-android build-android-arm64 build-android-arm build-android-386 build-android-amd64 build-android-all

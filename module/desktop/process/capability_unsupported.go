//go:build !cross_compile && !production

package process

// PopupSupported 编译期标记：非 production 构建时弹窗不可用
const PopupSupported = false

//go:build !cross_compile && production

package process

// PopupSupported 编译期标记：仅当 production tag 存在时为 true
const PopupSupported = true

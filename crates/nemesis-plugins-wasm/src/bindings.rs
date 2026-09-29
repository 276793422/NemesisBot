//! 宿主侧 WIT bindgen（两个 world 各自独立模块，避免生成模块树冲突）。

/// 工具插件 world 绑定（`plugin-tool`：导入 host，导出 tool）。
pub mod tool {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin-tool",
    });
}

/// 观察者插件 world 绑定（`plugin-observer`：导入 host，导出 observer）。
pub mod observer {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin-observer",
    });
}

//! 太微装配槽占位（stub）。
//!
//! 开源构建（feature `taiwei` 关）永不编译本 crate——它只满足 cargo 对
//! optional path 依赖的解析。若在未装配实现的情况下开启 feature `taiwei`，
//! 下面的编译错误就是给你的诚实提示：完整实现由商业装配器克隆进本槽位
//! （覆盖本 stub），单独启用 feature 没有实现可编。

compile_error!(
    "taiwei 槽位是装配槽：未检测到装配实现。请勿手动开启 feature \"taiwei\"\
     （完整实现由装配器注入本槽位后编译；开源构建保持 feature 关闭）。"
);

//! 测试夹具 re-export 壳（原 lib.rs 内联 mod body；BUG-20261106-014 迁移——
//! 「测试代码放独立文件」纪律要求带 body 的 cfg(test) 系 mod 走声明形态）。
pub use crate::vector::test_fixture::*;

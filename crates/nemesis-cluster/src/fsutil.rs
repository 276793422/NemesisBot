//! 文件系统韧性小工具（crate 内共享）。
//!
//! 实现已提升到 `nemesis_utils::fsutil`（全仓共享单一真相源——board/sandbox/
//! skills 等不依赖 cluster 的 crate 同样需要韧性删除）；本模块保留 re-export，
//! crate 内既有调用点零改动。

pub(crate) use nemesis_utils::fsutil::remove_dir_all_resilient;

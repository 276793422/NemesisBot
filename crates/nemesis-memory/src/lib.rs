//! NemesisBot - Memory Management Crate
//!
//! Provides a unified memory system with multiple storage backends:
//! - **LocalStore**: In-memory vector store with TF-IDF-like text matching
//! - **EpisodicStore**: Session-based conversation episode persistence (JSONL)
//! - **GraphStore**: Knowledge graph with entity-relation triples and BFS query
//! - **VectorStore**: Semantic vector search with local n-gram hash embeddings
//! - **MemoryManager**: Unified facade combining all stores

pub mod dreaming;
pub mod episodic;
pub mod graph;
pub mod local_store;
pub mod manager;
pub mod memory_tools;
pub mod retrieval;
pub mod store;
pub mod types;
pub mod vector;

#[cfg(test)]
mod dreaming_tests;

#[cfg(test)]
mod contract_tests;

// W4（2026-10-25 加固）：memory_forget 谱系传导（merge 派生连带归档）测试。
#[cfg(test)]
mod w4_cascade_tests;

// W5（2026-10-25 加固）：dreaming 近期召回硬保护（expire 排除）测试。
#[cfg(test)]
mod w5_recency_protect_tests;

// re-export 壳放独立文件（BUG-20261106-014：inline mod { body } 形态违反
// 「测试代码放独立文件」纪律，check-inline-tests.sh 形态 E 补齐后必拦）。
#[cfg(any(test, feature = "test-fixture"))]
#[doc(hidden)]
pub mod __test_fixture;

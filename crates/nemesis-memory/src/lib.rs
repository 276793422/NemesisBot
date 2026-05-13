//! NemesisBot - Memory Management Crate
//!
//! Provides a unified memory system with multiple storage backends:
//! - **LocalStore**: In-memory vector store with TF-IDF-like text matching
//! - **EpisodicStore**: Session-based conversation episode persistence (JSONL)
//! - **GraphStore**: Knowledge graph with entity-relation triples and BFS query
//! - **VectorStore**: Semantic vector search with local n-gram hash embeddings
//! - **MemoryManager**: Unified facade combining all stores

pub mod types;
pub mod store;
pub mod local_store;
pub mod episodic;
pub mod graph;
pub mod manager;
pub mod memory_tools;
pub mod vector;

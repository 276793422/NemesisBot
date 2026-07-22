//! Vector sub-module for semantic embedding and vector search.

mod embedding;
pub mod embedding_config;
mod plugin_loader;
mod store;

pub use embedding::EmbeddingFunc;
pub use embedding::new_embedding_func;
pub use plugin_loader::{EmbeddingPlugin, NativePlugin, PluginError, load_plugin};
pub use store::{QueryResult, StoreConfig, VectorEntry, VectorStore, cosine_similarity};

#[cfg(any(test, feature = "test-fixture"))]
#[doc(hidden)]
pub mod test_fixture;

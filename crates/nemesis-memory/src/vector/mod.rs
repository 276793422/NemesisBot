//! Vector sub-module for semantic embedding and vector search.

mod embedding;
pub mod embedding_config;
mod plugin_loader;
mod store;

pub use embedding::new_embedding_func;
pub use embedding::EmbeddingFunc;
pub use plugin_loader::{
    load_plugin, EmbeddingPlugin, NativePlugin, PluginError,
};
pub use store::{VectorStore, StoreConfig, VectorEntry, QueryResult, cosine_similarity};

#[cfg(any(test, feature = "test-fixture"))]
#[doc(hidden)]
pub mod test_fixture;

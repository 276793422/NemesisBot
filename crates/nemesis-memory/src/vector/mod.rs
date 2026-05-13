//! Vector sub-module for semantic embedding and vector search.

mod embedding;
mod embedding_api;
mod embedding_local;
mod plugin_loader;
mod store;

pub use embedding::new_embedding_func;
pub use embedding_api::{api_embedding_func, api_embedding_func_simple, ApiEmbeddingConfig};
pub use embedding_local::local_embedding_func;
pub use plugin_loader::{
    load_plugin, EmbeddingPlugin, NativePlugin, PluginError,
};
pub use store::{VectorStore, StoreConfig, VectorEntry, QueryResult};

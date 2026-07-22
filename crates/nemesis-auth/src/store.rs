//! Auth credential storage.

use crate::token::AuthCredential;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

/// On-disk auth credential store.
pub struct AuthStore {
    path: String,
    creds: RwLock<HashMap<String, AuthCredential>>,
}

impl AuthStore {
    /// Create a new store backed by a file.
    pub fn new(path: &str) -> Self {
        let store = Self {
            path: path.to_string(),
            creds: RwLock::new(HashMap::new()),
        };
        let _ = store.load();
        store
    }

    /// Save a credential.
    pub fn save(&self, provider: &str, cred: AuthCredential) -> Result<(), String> {
        self.creds
            .write()
            .unwrap()
            .insert(provider.to_string(), cred);
        self.persist()
    }

    /// Get a credential.
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.creds.read().unwrap().get(provider).cloned()
    }

    /// Remove a credential.
    pub fn remove(&self, provider: &str) -> Result<(), String> {
        self.creds.write().unwrap().remove(provider);
        self.persist()
    }

    /// List all providers.
    pub fn list_providers(&self) -> Vec<String> {
        self.creds.read().unwrap().keys().cloned().collect()
    }

    /// Delete all credentials by removing the store file.
    /// Mirrors Go DeleteAllCredentials.
    pub fn delete_all(&self) -> Result<(), String> {
        let path = Path::new(&self.path);
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| format!("delete auth file: {}", e))?;
        }
        self.creds.write().unwrap().clear();
        Ok(())
    }

    fn load(&self) -> Result<(), String> {
        if !Path::new(&self.path).exists() {
            return Ok(());
        }
        let data =
            std::fs::read_to_string(&self.path).map_err(|e| format!("read auth store: {}", e))?;
        let creds: HashMap<String, AuthCredential> =
            serde_json::from_str(&data).map_err(|e| format!("parse auth store: {}", e))?;
        *self.creds.write().unwrap() = creds;
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        if let Some(parent) = Path::new(&self.path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
        let data = serde_json::to_string_pretty(&*self.creds.read().unwrap())
            .map_err(|e| format!("serialize: {}", e))?;
        std::fs::write(&self.path, data).map_err(|e| format!("write: {}", e))
    }
}

#[cfg(test)]
mod tests;

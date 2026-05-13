//! Auth credential storage.

use crate::token::AuthCredential;
use std::path::Path;
use std::sync::RwLock;
use std::collections::HashMap;

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
        self.creds.write().unwrap().insert(provider.to_string(), cred);
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
        let data = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("read auth store: {}", e))?;
        let creds: HashMap<String, AuthCredential> = serde_json::from_str(&data)
            .map_err(|e| format!("parse auth store: {}", e))?;
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
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_cred() -> AuthCredential {
        AuthCredential {
            access_token: "at_123".to_string(),
            refresh_token: Some("rt_456".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            provider: "test".to_string(),
            auth_method: "oauth".to_string(),
            account_id: Some("acct_1".to_string()),
        }
    }

    #[test]
    fn test_save_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("test", test_cred()).unwrap();
        let cred = store.get("test").unwrap();
        assert_eq!(cred.access_token, "at_123");
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("test", test_cred()).unwrap();
        store.remove("test").unwrap();
        assert!(store.get("test").is_none());
    }

    #[test]
    fn test_list_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("p1", test_cred()).unwrap();
        store.save("p2", test_cred()).unwrap();
        let mut providers = store.list_providers();
        providers.sort();
        assert_eq!(providers, vec!["p1", "p2"]);
    }

    #[test]
    fn test_new_with_nonexistent_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent").join("auth.json").to_string_lossy().to_string();
        // Should succeed even when path doesn't exist
        let store = AuthStore::new(&path);
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_new_with_empty_path() {
        let store = AuthStore::new("");
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_new_with_corrupted_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "{invalid json!!!}").unwrap();
        let path_str = path.to_string_lossy().to_string();
        // Load should fail silently (error is discarded in new())
        let store = AuthStore::new(&path_str);
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_delete_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("p1", test_cred()).unwrap();
        store.save("p2", test_cred()).unwrap();
        assert_eq!(store.list_providers().len(), 2);
        store.delete_all().unwrap();
        assert!(store.list_providers().is_empty());
        assert!(store.get("p1").is_none());
        // File should be deleted
        assert!(!std::path::Path::new(&path).exists());
    }

    #[test]
    fn test_delete_all_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        // Should succeed even when file doesn't exist
        store.delete_all().unwrap();
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();

        // Create store, save data
        let store1 = AuthStore::new(&path);
        store1.save("myprovider", test_cred()).unwrap();

        // Create new store from same file
        let store2 = AuthStore::new(&path);
        let cred = store2.get("myprovider").unwrap();
        assert_eq!(cred.access_token, "at_123");
        assert_eq!(cred.provider, "test");
    }

    #[test]
    fn test_get_removed_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("provider1", test_cred()).unwrap();
        assert!(store.get("provider1").is_some());
        store.remove("provider1").unwrap();
        assert!(store.get("provider1").is_none());
    }

    #[test]
    fn test_remove_nonexistent_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        // Should succeed even for nonexistent provider
        store.remove("nonexistent").unwrap();
    }

    #[test]
    fn test_save_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);

        let mut cred1 = test_cred();
        cred1.access_token = "token_v1".to_string();
        store.save("provider", cred1).unwrap();

        let mut cred2 = test_cred();
        cred2.access_token = "token_v2".to_string();
        store.save("provider", cred2).unwrap();

        let loaded = store.get("provider").unwrap();
        assert_eq!(loaded.access_token, "token_v2");
    }

    #[test]
    fn test_get_nonexistent_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_from_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        assert!(store.get("any").is_none());
    }

    #[test]
    fn test_list_providers_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_list_providers_after_partial_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("p1", test_cred()).unwrap();
        store.save("p2", test_cred()).unwrap();
        store.save("p3", test_cred()).unwrap();
        store.remove("p2").unwrap();
        let mut providers = store.list_providers();
        providers.sort();
        assert_eq!(providers, vec!["p1", "p3"]);
    }

    #[test]
    fn test_save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subdir").join("nested").join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("test", test_cred()).unwrap();
        assert!(store.get("test").is_some());
        // File should exist in nested dir
        assert!(std::path::Path::new(&path).exists());
    }

    #[test]
    fn test_persistence_multiple_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();

        let store1 = AuthStore::new(&path);
        let mut cred_a = test_cred();
        cred_a.access_token = "token_a".to_string();
        let mut cred_b = test_cred();
        cred_b.access_token = "token_b".to_string();
        store1.save("provider_a", cred_a).unwrap();
        store1.save("provider_b", cred_b).unwrap();

        // Reload from file
        let store2 = AuthStore::new(&path);
        let mut providers = store2.list_providers();
        providers.sort();
        assert_eq!(providers, vec!["provider_a", "provider_b"]);
        assert_eq!(store2.get("provider_a").unwrap().access_token, "token_a");
        assert_eq!(store2.get("provider_b").unwrap().access_token, "token_b");
    }

    #[test]
    fn test_persistence_preserves_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();

        let store1 = AuthStore::new(&path);
        let cred = AuthCredential {
            access_token: "at_abc".to_string(),
            refresh_token: Some("rt_def".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(2)),
            provider: "myprov".to_string(),
            auth_method: "oauth".to_string(),
            account_id: Some("acct_123".to_string()),
        };
        store1.save("myprov", cred).unwrap();

        let store2 = AuthStore::new(&path);
        let loaded = store2.get("myprov").unwrap();
        assert_eq!(loaded.access_token, "at_abc");
        assert_eq!(loaded.refresh_token.unwrap(), "rt_def");
        assert!(loaded.expires_at.is_some());
        assert_eq!(loaded.provider, "myprov");
        assert_eq!(loaded.auth_method, "oauth");
        assert_eq!(loaded.account_id.unwrap(), "acct_123");
    }

    #[test]
    fn test_credential_without_optional_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();

        let store = AuthStore::new(&path);
        let cred = AuthCredential {
            access_token: "simple_token".to_string(),
            refresh_token: None,
            expires_at: None,
            provider: "simple".to_string(),
            auth_method: "api_key".to_string(),
            account_id: None,
        };
        store.save("simple", cred).unwrap();

        let loaded = store.get("simple").unwrap();
        assert_eq!(loaded.access_token, "simple_token");
        assert!(loaded.refresh_token.is_none());
        assert!(loaded.expires_at.is_none());
        assert!(loaded.account_id.is_none());
    }

    #[test]
    fn test_delete_all_clears_memory_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("a", test_cred()).unwrap();
        store.save("b", test_cred()).unwrap();
        assert!(std::path::Path::new(&path).exists());

        store.delete_all().unwrap();

        assert!(store.list_providers().is_empty());
        assert!(store.get("a").is_none());
        assert!(store.get("b").is_none());
        assert!(!std::path::Path::new(&path).exists());
    }

    #[test]
    fn test_delete_all_then_save_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("old", test_cred()).unwrap();
        store.delete_all().unwrap();

        // Should be able to save again after delete_all
        store.save("new", test_cred()).unwrap();
        assert!(store.get("new").is_some());
        assert!(store.get("old").is_none());
    }

    #[test]
    fn test_remove_last_provider_leaves_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("only_one", test_cred()).unwrap();
        store.remove("only_one").unwrap();
        assert!(store.list_providers().is_empty());
    }

    #[test]
    fn test_persistence_file_format_is_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json").to_string_lossy().to_string();
        let store = AuthStore::new(&path);
        store.save("prov", test_cred()).unwrap();

        // Read the raw file and verify it's valid JSON
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.get("prov").is_some());
    }
}

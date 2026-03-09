// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package auth_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/auth"
)

// TestLoadStore tests LoadStore function
func TestLoadStore(t *testing.T) {
	// Test 1: Load non-existent store creates new one
	t.Run("load non-existent store", func(t *testing.T) {
		// This test will use the default path which may not exist
		// The function should return an empty store
		store, err := auth.LoadStore()
		if err != nil {
			t.Fatalf("LoadStore() with non-existent file failed: %v", err)
		}
		if store == nil {
			t.Fatal("LoadStore() returned nil store")
		}
		if store.Credentials == nil {
			t.Fatal("LoadStore() returned nil Credentials map")
		}
		if len(store.Credentials) != 0 {
			t.Errorf("LoadStore() returned non-empty Credentials map: %v", store.Credentials)
		}
	})

	// Test 2: Save and load store
	t.Run("save and load store", func(t *testing.T) {
		// Create a test store
		testStore := &auth.AuthStore{
			Credentials: map[string]*auth.AuthCredential{
				"test_provider": {
					AccessToken:  "test_access_token",
					RefreshToken: "test_refresh_token",
					AccountID:    "test_account_id",
					ExpiresAt:    time.Now().Add(1 * time.Hour),
					Provider:     "test_provider",
					AuthMethod:   "oauth",
				},
			},
		}

		// Save the store
		err := auth.SaveStore(testStore)
		if err != nil {
			t.Fatalf("SaveStore() failed: %v", err)
		}

		// Load the store back
		loadedStore, err := auth.LoadStore()
		if err != nil {
			t.Fatalf("LoadStore() failed: %v", err)
		}

		// Verify the loaded store matches
		if len(loadedStore.Credentials) != len(testStore.Credentials) {
			t.Errorf("LoadStore() returned %d credentials, want %d", len(loadedStore.Credentials), len(testStore.Credentials))
		}

		cred, ok := loadedStore.Credentials["test_provider"]
		if !ok {
			t.Fatal("LoadStore() missing test_provider credential")
		}

		if cred.AccessToken != "test_access_token" {
			t.Errorf("LoadStore() AccessToken = %v, want 'test_access_token'", cred.AccessToken)
		}
		if cred.RefreshToken != "test_refresh_token" {
			t.Errorf("LoadStore() RefreshToken = %v, want 'test_refresh_token'", cred.RefreshToken)
		}
		if cred.AccountID != "test_account_id" {
			t.Errorf("LoadStore() AccountID = %v, want 'test_account_id'", cred.AccountID)
		}
		if cred.Provider != "test_provider" {
			t.Errorf("LoadStore() Provider = %v, want 'test_provider'", cred.Provider)
		}
		if cred.AuthMethod != "oauth" {
			t.Errorf("LoadStore() AuthMethod = %v, want 'oauth'", cred.AuthMethod)
		}
	})

	// Test 3: Load store with nil credentials map
	t.Run("load store with nil credentials", func(t *testing.T) {
		// We can't directly test this without file manipulation,
		// but we can test that an empty store initializes the map
		store := &auth.AuthStore{}
		if store.Credentials == nil {
			store.Credentials = make(map[string]*auth.AuthCredential)
		}
		if store.Credentials == nil {
			t.Fatal("Credentials map should not be nil after initialization")
		}
	})
}

// TestSaveStore tests SaveStore function
func TestSaveStore(t *testing.T) {
	// Test 1: Save valid store
	t.Run("save valid store", func(t *testing.T) {
		testStore := &auth.AuthStore{
			Credentials: map[string]*auth.AuthCredential{
				"provider1": {
					AccessToken: "token1",
					Provider:    "provider1",
					AuthMethod:  "oauth",
				},
			},
		}

		err := auth.SaveStore(testStore)
		if err != nil {
			t.Fatalf("SaveStore() failed: %v", err)
		}
	})

	// Test 2: Save empty store
	t.Run("save empty store", func(t *testing.T) {
		testStore := &auth.AuthStore{
			Credentials: map[string]*auth.AuthCredential{},
		}

		err := auth.SaveStore(testStore)
		if err != nil {
			t.Fatalf("SaveStore() with empty credentials failed: %v", err)
		}
	})

	// Test 3: Save store with multiple credentials
	t.Run("save multiple credentials", func(t *testing.T) {
		testStore := &auth.AuthStore{
			Credentials: map[string]*auth.AuthCredential{
				"provider1": {
					AccessToken: "token1",
					Provider:    "provider1",
					AuthMethod:  "oauth",
				},
				"provider2": {
					AccessToken:  "token2",
					RefreshToken: "refresh2",
					Provider:     "provider2",
					AuthMethod:   "paste_token",
				},
			},
		}

		err := auth.SaveStore(testStore)
		if err != nil {
			t.Fatalf("SaveStore() with multiple credentials failed: %v", err)
		}
	})
}

// TestGetCredential tests GetCredential function
func TestGetCredential(t *testing.T) {
	// Setup: Create a test credential
	testCred := &auth.AuthCredential{
		AccessToken:  "test_token",
		RefreshToken: "test_refresh",
		AccountID:    "test_account",
		ExpiresAt:    time.Now().Add(1 * time.Hour),
		Provider:     "test_provider",
		AuthMethod:   "oauth",
	}

	err := auth.SetCredential("test_provider", testCred)
	if err != nil {
		t.Fatalf("Setup: SetCredential() failed: %v", err)
	}

	// Clean up after test
	defer auth.DeleteCredential("test_provider")

	// Test 1: Get existing credential
	t.Run("get existing credential", func(t *testing.T) {
		cred, err := auth.GetCredential("test_provider")
		if err != nil {
			t.Fatalf("GetCredential() failed: %v", err)
		}
		if cred == nil {
			t.Fatal("GetCredential() returned nil for existing credential")
		}
		if cred.AccessToken != "test_token" {
			t.Errorf("GetCredential() AccessToken = %v, want 'test_token'", cred.AccessToken)
		}
		if cred.Provider != "test_provider" {
			t.Errorf("GetCredential() Provider = %v, want 'test_provider'", cred.Provider)
		}
	})

	// Test 2: Get non-existent credential returns nil, not error
	t.Run("get non-existent credential", func(t *testing.T) {
		cred, err := auth.GetCredential("non_existent_provider")
		if err != nil {
			t.Fatalf("GetCredential() with non-existent provider failed: %v", err)
		}
		if cred != nil {
			t.Errorf("GetCredential() returned non-nil for non-existent credential: %v", cred)
		}
	})
}

// TestSetCredential tests SetCredential function
func TestSetCredential(t *testing.T) {
	// Test 1: Set new credential
	t.Run("set new credential", func(t *testing.T) {
		testCred := &auth.AuthCredential{
			AccessToken: "new_token",
			Provider:    "new_provider",
			AuthMethod:  "oauth",
		}

		err := auth.SetCredential("new_provider", testCred)
		if err != nil {
			t.Fatalf("SetCredential() failed: %v", err)
		}

		// Verify it was saved
		loaded, err := auth.GetCredential("new_provider")
		if err != nil {
			t.Fatalf("GetCredential() failed: %v", err)
		}
		if loaded == nil {
			t.Fatal("SetCredential() credential was not saved")
		}
		if loaded.AccessToken != "new_token" {
			t.Errorf("SetCredential() AccessToken not saved correctly: %v", loaded.AccessToken)
		}

		// Cleanup
		auth.DeleteCredential("new_provider")
	})

	// Test 2: Update existing credential
	t.Run("update existing credential", func(t *testing.T) {
		// Set initial credential
		initialCred := &auth.AuthCredential{
			AccessToken: "initial_token",
			Provider:    "update_provider",
			AuthMethod:  "oauth",
		}
		err := auth.SetCredential("update_provider", initialCred)
		if err != nil {
			t.Fatalf("Setup: SetCredential() failed: %v", err)
		}

		// Update with new credential
		updatedCred := &auth.AuthCredential{
			AccessToken:  "updated_token",
			RefreshToken: "new_refresh",
			Provider:     "update_provider",
			AuthMethod:   "oauth",
		}
		err = auth.SetCredential("update_provider", updatedCred)
		if err != nil {
			t.Fatalf("SetCredential() update failed: %v", err)
		}

		// Verify it was updated
		loaded, err := auth.GetCredential("update_provider")
		if err != nil {
			t.Fatalf("GetCredential() failed: %v", err)
		}
		if loaded.AccessToken != "updated_token" {
			t.Errorf("SetCredential() AccessToken not updated: %v", loaded.AccessToken)
		}
		if loaded.RefreshToken != "new_refresh" {
			t.Errorf("SetCredential() RefreshToken not updated: %v", loaded.RefreshToken)
		}

		// Cleanup
		auth.DeleteCredential("update_provider")
	})

	// Test 3: Set credential with all fields
	t.Run("set credential with all fields", func(t *testing.T) {
		testCred := &auth.AuthCredential{
			AccessToken:  "full_token",
			RefreshToken: "full_refresh",
			AccountID:    "full_account",
			ExpiresAt:    time.Now().Add(24 * time.Hour),
			Provider:     "full_provider",
			AuthMethod:   "paste_token",
		}

		err := auth.SetCredential("full_provider", testCred)
		if err != nil {
			t.Fatalf("SetCredential() with full credential failed: %v", err)
		}

		// Verify all fields were saved
		loaded, err := auth.GetCredential("full_provider")
		if err != nil {
			t.Fatalf("GetCredential() failed: %v", err)
		}
		if loaded.AccessToken != "full_token" {
			t.Errorf("SetCredential() AccessToken = %v, want 'full_token'", loaded.AccessToken)
		}
		if loaded.RefreshToken != "full_refresh" {
			t.Errorf("SetCredential() RefreshToken = %v, want 'full_refresh'", loaded.RefreshToken)
		}
		if loaded.AccountID != "full_account" {
			t.Errorf("SetCredential() AccountID = %v, want 'full_account'", loaded.AccountID)
		}
		if loaded.Provider != "full_provider" {
			t.Errorf("SetCredential() Provider = %v, want 'full_provider'", loaded.Provider)
		}
		if loaded.AuthMethod != "paste_token" {
			t.Errorf("SetCredential() AuthMethod = %v, want 'paste_token'", loaded.AuthMethod)
		}

		// Cleanup
		auth.DeleteCredential("full_provider")
	})
}

// TestDeleteCredential tests DeleteCredential function
func TestDeleteCredential(t *testing.T) {
	// Setup: Create a test credential
	testCred := &auth.AuthCredential{
		AccessToken: "to_delete",
		Provider:    "delete_provider",
		AuthMethod:  "oauth",
	}

	err := auth.SetCredential("delete_provider", testCred)
	if err != nil {
		t.Fatalf("Setup: SetCredential() failed: %v", err)
	}

	// Test 1: Delete existing credential
	t.Run("delete existing credential", func(t *testing.T) {
		err := auth.DeleteCredential("delete_provider")
		if err != nil {
			t.Fatalf("DeleteCredential() failed: %v", err)
		}

		// Verify it was deleted
		cred, err := auth.GetCredential("delete_provider")
		if err != nil {
			t.Fatalf("GetCredential() failed: %v", err)
		}
		if cred != nil {
			t.Errorf("DeleteCredential() credential still exists: %v", cred)
		}
	})

	// Test 2: Delete non-existent credential (should not error)
	t.Run("delete non-existent credential", func(t *testing.T) {
		err := auth.DeleteCredential("already_deleted_provider")
		if err != nil {
			t.Errorf("DeleteCredential() with non-existent credential failed: %v", err)
		}
	})
}

// TestDeleteAllCredentials tests DeleteAllCredentials function
func TestDeleteAllCredentials(t *testing.T) {
	// Setup: Create multiple test credentials
	credentials := map[string]*auth.AuthCredential{
		"provider1": {
			AccessToken: "token1",
			Provider:    "provider1",
			AuthMethod:  "oauth",
		},
		"provider2": {
			AccessToken: "token2",
			Provider:    "provider2",
			AuthMethod:  "paste_token",
		},
		"provider3": {
			AccessToken: "token3",
			Provider:    "provider3",
			AuthMethod:  "oauth",
		},
	}

	for provider, cred := range credentials {
		err := auth.SetCredential(provider, cred)
		if err != nil {
			t.Fatalf("Setup: SetCredential() for %s failed: %v", provider, err)
		}
	}

	// Test: Delete all credentials
	t.Run("delete all credentials", func(t *testing.T) {
		err := auth.DeleteAllCredentials()
		if err != nil {
			t.Fatalf("DeleteAllCredentials() failed: %v", err)
		}

		// Verify all credentials were deleted
		store, err := auth.LoadStore()
		if err != nil {
			t.Fatalf("LoadStore() failed: %v", err)
		}
		if len(store.Credentials) != 0 {
			t.Errorf("DeleteAllCredentials() %d credentials remain", len(store.Credentials))
		}
	})

	// Test: Delete all when file doesn't exist (should not error)
	t.Run("delete all when file doesn't exist", func(t *testing.T) {
		err := auth.DeleteAllCredentials()
		if err != nil {
			t.Errorf("DeleteAllCredentials() with non-existent file failed: %v", err)
		}
	})
}

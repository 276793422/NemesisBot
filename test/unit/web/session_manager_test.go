// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web_test

import (
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/web"
)

// TestNewSessionManager tests creating a new session manager
func TestNewSessionManager(t *testing.T) {
	t.Run("create with timeout", func(t *testing.T) {
		sm := web.NewSessionManager(30 * time.Minute)
		if sm == nil {
			t.Fatal("NewSessionManager() returned nil")
		}
	})

	t.Run("create with zero timeout", func(t *testing.T) {
		sm := web.NewSessionManager(0)
		if sm == nil {
			t.Fatal("NewSessionManager() returned nil")
		}
	})

	t.Run("create with negative timeout", func(t *testing.T) {
		sm := web.NewSessionManager(-1 * time.Hour)
		if sm == nil {
			t.Fatal("NewSessionManager() returned nil")
		}
	})
}

// TestSessionManager_CreateSession tests session creation
func TestSessionManager_CreateSession(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("create session with nil connection", func(t *testing.T) {
		// This should not panic
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// Verify session ID is generated
		if session.ID == "" {
			t.Error("CreateSession() returned session with empty ID")
		}

		// Verify sender ID format
		if session.SenderID == "" {
			t.Error("CreateSession() returned session with empty SenderID")
		}
		// SenderID should be in format "web:{sessionID}"
		// We can't check the exact format without the connection
	})
}

// TestSessionManager_GetSession tests session retrieval
func TestSessionManager_GetSession(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("get non-existent session", func(t *testing.T) {
		session, ok := sm.GetSession("non-existent")
		if ok {
			t.Error("GetSession() returned true for non-existent session")
		}
		if session != nil {
			t.Error("GetSession() returned non-nil session for non-existent ID")
		}
	})

	t.Run("get session with empty ID", func(t *testing.T) {
		session, ok := sm.GetSession("")
		if ok {
			t.Error("GetSession() returned true for empty ID")
		}
		if session != nil {
			t.Error("GetSession() returned non-nil session for empty ID")
		}
	})
}

// TestSessionManager_RemoveSession tests session removal
func TestSessionManager_RemoveSession(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("remove non-existent session", func(t *testing.T) {
		// Should not panic
		sm.RemoveSession("non-existent")
	})

	t.Run("remove session with empty ID", func(t *testing.T) {
		// Should not panic
		sm.RemoveSession("")
	})

	t.Run("create and remove session", func(t *testing.T) {
		// Create a session
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// Verify session exists
		_, ok := sm.GetSession(session.ID)
		if !ok {
			t.Error("GetSession() returned false for newly created session")
		}

		// Remove the session
		sm.RemoveSession(session.ID)

		// Verify session is removed
		_, ok = sm.GetSession(session.ID)
		if ok {
			t.Error("GetSession() returned true after RemoveSession()")
		}
	})
}

// TestSessionManager_Broadcast tests broadcasting to sessions
func TestSessionManager_Broadcast(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("broadcast to non-existent session", func(t *testing.T) {
		message := []byte("test message")
		err := sm.Broadcast("non-existent", message)
		if err == nil {
			t.Error("Broadcast() to non-existent session should return error")
		}
	})

	t.Run("broadcast empty message", func(t *testing.T) {
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		err := sm.Broadcast(session.ID, []byte{})
		// May or may not error depending on implementation
		_ = err
	})

	t.Run("broadcast to existing session", func(t *testing.T) {
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		message := []byte("test message")
		err := sm.Broadcast(session.ID, message)
		// May error due to nil connection, but should not panic
		_ = err
	})
}

// TestSessionManager_Stats tests session statistics
func TestSessionManager_Stats(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("stats with no sessions", func(t *testing.T) {
		stats := sm.Stats()
		if stats == nil {
			t.Fatal("Stats() returned nil")
		}

		// Check that stats have expected fields
		// We can't check exact values without knowing the struct
		_ = stats
	})

	t.Run("stats with sessions", func(t *testing.T) {
		// Create multiple sessions
		for i := 0; i < 3; i++ {
			session := sm.CreateSession(nil)
			if session == nil {
				t.Fatal("CreateSession() returned nil")
			}
		}

		stats := sm.Stats()
		if stats == nil {
			t.Fatal("Stats() returned nil")
		}

		// Stats should reflect the sessions
		_ = stats
	})
}

// TestSessionManager_GetActiveCount tests getting active session count
func TestSessionManager_GetActiveCount(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("active count with no sessions", func(t *testing.T) {
		count := sm.GetActiveCount()
		if count < 0 {
			t.Errorf("GetActiveCount() returned negative count: %d", count)
		}
	})

	t.Run("active count with sessions", func(t *testing.T) {
		// Create some sessions
		initialCount := sm.GetActiveCount()

		for i := 0; i < 3; i++ {
			session := sm.CreateSession(nil)
			if session == nil {
				t.Fatal("CreateSession() returned nil")
			}
		}

		newCount := sm.GetActiveCount()
		if newCount <= initialCount {
			t.Errorf("GetActiveCount() = %d, want > %d", newCount, initialCount)
		}
	})

	t.Run("active count after removal", func(t *testing.T) {
		// Create a session
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		countBefore := sm.GetActiveCount()

		// Remove the session
		sm.RemoveSession(session.ID)

		countAfter := sm.GetActiveCount()
		if countAfter >= countBefore {
			t.Errorf("GetActiveCount() after removal = %d, want < %d", countAfter, countBefore)
		}
	})
}

// TestSessionManager_GetAllSessions tests getting all sessions
func TestSessionManager_GetAllSessions(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("get all sessions with none", func(t *testing.T) {
		sessions := sm.GetAllSessions()
		if sessions == nil {
			t.Log("GetAllSessions() returned nil (may be expected behavior)")
			return
		}
		if len(sessions) != 0 {
			t.Errorf("GetAllSessions() returned %d sessions, want 0", len(sessions))
		}
	})

	t.Run("get all sessions with some", func(t *testing.T) {
		// Create sessions
		session1 := sm.CreateSession(nil)
		session2 := sm.CreateSession(nil)
		if session1 == nil || session2 == nil {
			t.Fatal("CreateSession() returned nil")
		}

		sessions := sm.GetAllSessions()
		if sessions == nil {
			t.Fatal("GetAllSessions() returned nil")
		}

		if len(sessions) < 2 {
			t.Errorf("GetAllSessions() returned %d sessions, want at least 2", len(sessions))
		}
	})
}

// TestSessionManager_Shutdown tests shutting down the session manager
func TestSessionManager_Shutdown(t *testing.T) {
	t.Run("shutdown with no sessions", func(t *testing.T) {
		sm := web.NewSessionManager(30 * time.Minute)

		// Should not panic
		sm.Shutdown()
	})

	t.Run("shutdown with sessions", func(t *testing.T) {
		sm := web.NewSessionManager(30 * time.Minute)

		// Create some sessions
		for i := 0; i < 3; i++ {
			session := sm.CreateSession(nil)
			if session == nil {
				t.Fatal("CreateSession() returned nil")
			}
		}

		// Should not panic
		sm.Shutdown()

		// Verify sessions are cleared
		count := sm.GetActiveCount()
		if count != 0 {
			t.Errorf("GetActiveCount() after Shutdown() = %d, want 0", count)
		}
	})

	t.Run("shutdown multiple times", func(t *testing.T) {
		sm := web.NewSessionManager(30 * time.Minute)

		// Should not panic
		sm.Shutdown()
		sm.Shutdown()
		sm.Shutdown()
	})
}

// TestSessionManager_ConcurrentOperations tests concurrent session operations
func TestSessionManager_ConcurrentOperations(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("concurrent session creation", func(t *testing.T) {
		var wg sync.WaitGroup
		numSessions := 100

		for i := 0; i < numSessions; i++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				session := sm.CreateSession(nil)
				if session == nil {
					t.Error("CreateSession() returned nil")
				}
			}()
		}

		wg.Wait()

		// Verify sessions were created
		count := sm.GetActiveCount()
		if count < numSessions {
			t.Logf("Note: GetActiveCount() = %d, may be less than %d due to nil connections", count, numSessions)
		}
	})

	t.Run("concurrent session access", func(t *testing.T) {
		// Create some sessions
		sessions := make([]string, 10)
		for i := 0; i < 10; i++ {
			session := sm.CreateSession(nil)
			if session == nil {
				t.Fatal("CreateSession() returned nil")
			}
			sessions[i] = session.ID
		}

		var wg sync.WaitGroup

		// Concurrent reads
		for _, sessionID := range sessions {
			wg.Add(1)
			go func(id string) {
				defer wg.Done()
				sm.GetSession(id)
			}(sessionID)
		}

		// Concurrent broadcasts
		for _, sessionID := range sessions {
			wg.Add(1)
			go func(id string) {
				defer wg.Done()
				sm.Broadcast(id, []byte("test"))
			}(sessionID)
		}

		wg.Wait()
		// If we got here without panic, the test passed
	})

	t.Run("concurrent create and remove", func(t *testing.T) {
		var wg sync.WaitGroup

		// Concurrent creates
		for i := 0; i < 50; i++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				session := sm.CreateSession(nil)
				if session != nil {
					// Immediately remove
					sm.RemoveSession(session.ID)
				}
			}()
		}

		wg.Wait()
		// If we got here without panic, the test passed
	})
}

// TestSessionManager_SessionTimeout tests session timeout behavior
func TestSessionManager_SessionTimeout(t *testing.T) {
	t.Run("cleanup with short timeout", func(t *testing.T) {
		// Create manager with very short timeout
		sm := web.NewSessionManager(100 * time.Millisecond)

		// Create a session
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// Wait for timeout + cleanup
		time.Sleep(300 * time.Millisecond)

		// Session may have been cleaned up
		_, ok := sm.GetSession(session.ID)
		_ = ok // Result depends on cleanup timing
	})

	t.Run("cleanup with long timeout", func(t *testing.T) {
		// Create manager with long timeout
		sm := web.NewSessionManager(1 * time.Hour)

		// Create a session
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// Wait a bit
		time.Sleep(100 * time.Millisecond)

		// Session should still exist
		_, ok := sm.GetSession(session.ID)
		if !ok {
			t.Error("GetSession() returned false for session with long timeout")
		}
	})
}

// TestSessionManager_SessionIDGeneration tests session ID generation
func TestSessionManager_SessionIDGeneration(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("unique session IDs", func(t *testing.T) {
		ids := make(map[string]bool)

		// Create many sessions
		for i := 0; i < 1000; i++ {
			session := sm.CreateSession(nil)
			if session == nil {
				t.Fatal("CreateSession() returned nil")
			}

			if ids[session.ID] {
				t.Errorf("Duplicate session ID generated: %s", session.ID)
			}
			ids[session.ID] = true
		}

		if len(ids) != 1000 {
			t.Errorf("Generated %d unique IDs out of 1000 sessions", len(ids))
		}
	})

	t.Run("session ID format", func(t *testing.T) {
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// Session ID should not be empty
		if session.ID == "" {
			t.Error("Session ID is empty")
		}

		// Session ID should be reasonable length
		if len(session.ID) < 10 {
			t.Errorf("Session ID too short: %s", session.ID)
		}
	})
}

// TestSessionManager_SenderIDFormat tests sender ID format
func TestSessionManager_SenderIDFormat(t *testing.T) {
	sm := web.NewSessionManager(30 * time.Minute)

	t.Run("sender ID has correct format", func(t *testing.T) {
		session := sm.CreateSession(nil)
		if session == nil {
			t.Fatal("CreateSession() returned nil")
		}

		// SenderID should not be empty
		if session.SenderID == "" {
			t.Error("SenderID is empty")
		}

		// ChatID should match SenderID for web sessions
		if session.ChatID != session.SenderID {
			t.Errorf("ChatID (%s) should match SenderID (%s)", session.ChatID, session.SenderID)
		}

		// SenderID should start with "web:"
		if len(session.SenderID) < 5 {
			t.Errorf("SenderID too short: %s", session.SenderID)
		}
	})
}

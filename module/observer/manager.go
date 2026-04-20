package observer

import (
	"context"
	"sync"

	"github.com/276793422/NemesisBot/module/logger"
)

// Manager manages multiple conversation event observers.
type Manager struct {
	observers []Observer
	mu        sync.RWMutex
}

// NewManager creates a new Observer Manager.
func NewManager() *Manager {
	return &Manager{}
}

// Register adds an observer.
func (m *Manager) Register(obs Observer) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.observers = append(m.observers, obs)
}

// Unregister removes an observer by name.
func (m *Manager) Unregister(name string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for i, obs := range m.observers {
		if obs.Name() == name {
			m.observers = append(m.observers[:i], m.observers[i+1:]...)
			return
		}
	}
}

// Emit sends an event to all observers asynchronously.
// Each observer runs in its own goroutine so that slow observers don't block others.
func (m *Manager) Emit(ctx context.Context, event ConversationEvent) {
	m.mu.RLock()
	observers := make([]Observer, len(m.observers))
	copy(observers, m.observers)
	m.mu.RUnlock()

	for _, obs := range observers {
		go func(o Observer) {
			defer func() {
				if r := recover(); r != nil {
					logger.WarnCF("observer", "Observer panicked",
						map[string]interface{}{
							"observer": o.Name(),
							"event":    string(event.Type),
							"panic":    r,
						})
				}
			}()
			o.OnEvent(ctx, event)
		}(obs)
	}
}

// EmitSync sends an event to all observers synchronously.
// Use for events where all observers must complete before proceeding (e.g. conversation_end).
func (m *Manager) EmitSync(ctx context.Context, event ConversationEvent) {
	m.mu.RLock()
	observers := make([]Observer, len(m.observers))
	copy(observers, m.observers)
	m.mu.RUnlock()

	for _, obs := range observers {
		func(o Observer) {
			defer func() {
				if r := recover(); r != nil {
					logger.WarnCF("observer", "Observer panicked (sync)",
						map[string]interface{}{
							"observer": o.Name(),
							"event":    string(event.Type),
							"panic":    r,
						})
				}
			}()
			o.OnEvent(ctx, event)
		}(obs)
	}
}

// HasObservers returns true if at least one observer is registered.
func (m *Manager) HasObservers() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return len(m.observers) > 0
}

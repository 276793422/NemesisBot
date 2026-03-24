//go:build !cross_compile

package websocket

import (
	"sync"
	"time"

	"github.com/google/uuid"
)

// WebSocketKey WebSocket 密钥
type WebSocketKey struct {
	Key       string    // UUID
	Port      int       // WebSocket 端口
	Path      string    // WebSocket 路径
	CreatedAt time.Time // 创建时间
	UsedAt    time.Time // 使用时间
	ChildPID  int       // 子进程 PID
	ChildID   string    // 子进程 ID
	Valid     bool      // 是否有效
}

// KeyGenerator 密钥生成器
type KeyGenerator struct {
	mu    sync.Mutex
	keys  map[string]*WebSocketKey
	nextPort int
}

// NewKeyGenerator 创建密钥生成器
func NewKeyGenerator() *KeyGenerator {
	return &KeyGenerator{
		keys:     make(map[string]*WebSocketKey),
		nextPort: 40000, // 起始端口
	}
}

// Generate 生成新密钥
func (k *KeyGenerator) Generate(childID string, childPID int) (*WebSocketKey, error) {
	k.mu.Lock()
	defer k.mu.Unlock()

	// 生成 UUID
	key := uuid.New().String()

	// 分配端口
	port := k.nextPort
	k.nextPort++

	wsKey := &WebSocketKey{
		Key:       key,
		Port:      port,
		Path:      "/child/" + key,
		CreatedAt: time.Now(),
		ChildPID:  childPID,
		ChildID:   childID,
		Valid:     true,
	}

	k.keys[key] = wsKey

	return wsKey, nil
}

// Validate 验证密钥
func (k *KeyGenerator) Validate(key string) (*WebSocketKey, error) {
	k.mu.Lock()
	defer k.mu.Unlock()

	wsKey, ok := k.keys[key]
	if !ok {
		return nil, ErrKeyNotFound
	}

	if !wsKey.Valid {
		return nil, ErrKeyInvalid
	}

	// 标记为已使用
	wsKey.UsedAt = time.Now()

	return wsKey, nil
}

// Revoke 撤销密钥
func (k *KeyGenerator) Revoke(key string) error {
	k.mu.Lock()
	defer k.mu.Unlock()

	wsKey, ok := k.keys[key]
	if !ok {
		return ErrKeyNotFound
	}

	wsKey.Valid = false
	return nil
}

// Cleanup 清理过期密钥
func (k *KeyGenerator) Cleanup(maxAge time.Duration) {
	k.mu.Lock()
	defer k.mu.Unlock()

	now := time.Now()
	for key, wsKey := range k.keys {
		if now.Sub(wsKey.CreatedAt) > maxAge {
			delete(k.keys, key)
		}
	}
}

// Errors
var (
	ErrKeyNotFound = &WebSocketError{Code: "KEY_NOT_FOUND", Message: "Key not found"}
	ErrKeyInvalid = &WebSocketError{Code: "KEY_INVALID", Message: "Key is invalid or expired"}
)

// WebSocketError WebSocket 错误
type WebSocketError struct {
	Code    string
	Message string
}

func (e *WebSocketError) Error() string {
	return e.Message
}

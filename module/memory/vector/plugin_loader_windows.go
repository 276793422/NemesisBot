//go:build windows

package vector

import (
	"fmt"
	"sync"
	"syscall"
	"unsafe"
)

// nativePlugin implements EmbeddingPlugin using syscall.LoadDLL (Windows).
type nativePlugin struct {
	mu        sync.Mutex
	dll       *syscall.DLL
	initProc  *syscall.Proc
	embedProc *syscall.Proc
	freeProc  *syscall.Proc
	dim       int
}

// LoadPlugin loads a DLL and returns an EmbeddingPlugin.
func LoadPlugin(path string) (EmbeddingPlugin, error) {
	dll, err := syscall.LoadDLL(path)
	if err != nil {
		return nil, fmt.Errorf("vector: load DLL %s: %w", path, err)
	}

	initProc, err := dll.FindProc("embed_init")
	if err != nil {
		dll.Release()
		return nil, fmt.Errorf("vector: find embed_init: %w", err)
	}

	embedProc, err := dll.FindProc("embed")
	if err != nil {
		dll.Release()
		return nil, fmt.Errorf("vector: find embed: %w", err)
	}

	freeProc, err := dll.FindProc("embed_free")
	if err != nil {
		dll.Release()
		return nil, fmt.Errorf("vector: find embed_free: %w", err)
	}

	return &nativePlugin{
		dll:       dll,
		initProc:  initProc,
		embedProc: embedProc,
		freeProc:  freeProc,
	}, nil
}

func (p *nativePlugin) Init(modelPath string, dim int) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	mp, err := syscall.BytePtrFromString(modelPath)
	if err != nil {
		return err
	}

	ret, _, _ := p.initProc.Call(uintptr(unsafe.Pointer(mp)), uintptr(dim))
	if ret != 0 {
		return fmt.Errorf("vector: embed_init returned %d", int32(ret))
	}
	p.dim = dim
	return nil
}

func (p *nativePlugin) Embed(text string) ([]float32, error) {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.dim <= 0 {
		return nil, fmt.Errorf("vector: plugin not initialized (dim=%d)", p.dim)
	}

	ctext, err := syscall.BytePtrFromString(text)
	if err != nil {
		return nil, err
	}

	buf := make([]float32, p.dim)

	ret, _, _ := p.embedProc.Call(
		uintptr(unsafe.Pointer(ctext)),
		uintptr(unsafe.Pointer(&buf[0])),
		uintptr(p.dim),
	)
	if ret != 0 {
		return nil, fmt.Errorf("vector: embed returned %d", int32(ret))
	}

	return buf, nil
}

func (p *nativePlugin) Close() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.freeProc != nil {
		p.freeProc.Call()
	}
	if p.dll != nil {
		p.dll.Release()
	}
}

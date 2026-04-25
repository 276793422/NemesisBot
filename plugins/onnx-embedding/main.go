package main

/*
#include <stdlib.h>
*/
import "C"
import (
	"context"
	"fmt"
	"sync"
	"unsafe"
)

var (
	engine   *embeddingEngine
	engineMu sync.Mutex
)

//export embed_init
func embed_init(modelPath *C.char, dim C.int) C.int {
	engineMu.Lock()
	defer engineMu.Unlock()

	path := C.GoString(modelPath)

	e, err := newEmbeddingEngine(path, int(dim))
	if err != nil {
		return -1
	}
	engine = e
	return 0
}

//export embed
func embed(text *C.char, output *C.float, dim C.int) C.int {
	engineMu.Lock()
	defer engineMu.Unlock()

	if engine == nil {
		return -1
	}

	goText := C.GoString(text)

	vec, err := engine.embed(context.Background(), goText)
	if err != nil {
		return -2
	}

	if len(vec) != int(dim) {
		return -3
	}

	outSlice := unsafe.Slice(output, dim)
	for i, v := range vec {
		outSlice[i] = C.float(v)
	}

	return 0
}

//export embed_free
func embed_free() {
	engineMu.Lock()
	defer engineMu.Unlock()

	if engine != nil {
		engine.close()
		engine = nil
	}
}

func main() {
	fmt.Println("ONNX Embedding Plugin - build as shared library with: go build -buildmode=c-shared")
}

//go:build cgo

package main

import (
	"C"
	"log"
	"sync"
	"unsafe"

	"websocket-client/src/client"
	"websocket-client/src/config"
)

var (
	globalMu  sync.Mutex
	globalCli *client.WebSocketClient
)

//export WSC_Init
func WSC_Init(url *C.char, token *C.char) C.int {
	goURL := C.GoString(url)
	goToken := C.GoString(token)

	globalMu.Lock()
	defer globalMu.Unlock()

	if globalCli != nil {
		globalCli.Destroy()
		globalCli = nil
	}

	cfg := config.NewConfig(goURL, goToken)
	cli := client.New(cfg)

	if err := cli.Start(); err != nil {
		log.Printf("WSC_Init: %v", err)
		return -1
	}

	globalCli = cli
	return 0
}

//export WSC_Send
func WSC_Send(content *C.char) C.int {
	globalMu.Lock()
	cli := globalCli
	globalMu.Unlock()

	if cli == nil {
		return -1
	}
	if !cli.IsConnected() {
		return -2
	}
	if err := cli.Send(C.GoString(content)); err != nil {
		return -3
	}
	return 0
}

//export WSC_Recv
func WSC_Recv(buf *C.char, bufSize C.int, timeoutMs C.int) C.int {
	globalMu.Lock()
	cli := globalCli
	globalMu.Unlock()

	if cli == nil {
		return -1
	}

	data := cli.Recv(int(timeoutMs))
	if data == nil {
		return 0
	}

	n := len(data)
	if n >= int(bufSize) {
		n = int(bufSize) - 1
	}
	if n <= 0 {
		return 0
	}

	dest := unsafe.Slice((*byte)(unsafe.Pointer(buf)), int(bufSize))
	copy(dest, data[:n])
	dest[n] = 0

	return C.int(n)
}

//export WSC_Destroy
func WSC_Destroy() {
	globalMu.Lock()
	cli := globalCli
	globalCli = nil
	globalMu.Unlock()

	if cli != nil {
		cli.Destroy()
	}
}

func main() {}

.PHONY: dll so clean deps

# Windows (requires MinGW-w64 + CGO_ENABLED=1)
dll:
	CGO_ENABLED=1 GOOS=windows GOARCH=amd64 CC=x86_64-w64-mingw32-gcc \
	go build -buildmode=c-shared -o onnx_embedding.dll .

# Linux
so:
	CGO_ENABLED=1 go build -buildmode=c-shared -o onnx_embedding.so .

deps:
	go mod tidy

clean:
	rm -f onnx_embedding.dll onnx_embedding.so onnx_embedding.h

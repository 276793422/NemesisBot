// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport_test

import (
	"bytes"
	"encoding/binary"
	"io"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

func TestEncodeFrame(t *testing.T) {
	tests := []struct {
		name    string
		data    []byte
		wantErr bool
	}{
		{
			name:    "small data",
			data:    []byte("hello"),
			wantErr: false,
		},
		{
			name:    "empty data",
			data:    []byte{},
			wantErr: false,
		},
		{
			name:    "medium data",
			data:    bytes.Repeat([]byte("abc"), 1000),
			wantErr: false,
		},
		{
			name:    "large but within limit",
			data:    bytes.Repeat([]byte("x"), 1024*1024),
			wantErr: false,
		},
		{
			name:    "too large",
			data:    bytes.Repeat([]byte("x"), transport.MaxFrameSize+1),
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			frame, err := transport.EncodeFrame(tt.data)
			if (err != nil) != tt.wantErr {
				t.Errorf("EncodeFrame() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if tt.wantErr {
				return
			}

			// Verify frame structure
			if len(frame) < transport.FrameHeaderSize {
				t.Errorf("EncodeFrame() frame too short: %d", len(frame))
				return
			}

			// Verify length matches
			expectedLen := uint32(len(tt.data))
			actualLen := binary.BigEndian.Uint32(frame[:transport.FrameHeaderSize])
			if actualLen != expectedLen {
				t.Errorf("EncodeFrame() length = %d, want %d", actualLen, expectedLen)
			}

			// Verify data matches
			if !bytes.Equal(frame[transport.FrameHeaderSize:], tt.data) {
				t.Errorf("EncodeFrame() data mismatch")
			}
		})
	}
}

func TestDecodeFrame(t *testing.T) {
	tests := []struct {
		name    string
		input   []byte
		want    []byte
		wantErr bool
	}{
		{
			name:    "valid small frame",
			input:   append([]byte{0, 0, 0, 5}, []byte("hello")...),
			want:    []byte("hello"),
			wantErr: false,
		},
		{
			name:    "valid empty frame",
			input:   []byte{0, 0, 0, 0},
			want:    []byte{},
			wantErr: false,
		},
		{
			name:    "incomplete header",
			input:   []byte{0, 0, 0},
			want:    nil,
			wantErr: true,
		},
		{
			name:    "incomplete data",
			input:   append([]byte{0, 0, 0, 10}, []byte("partial")...),
			want:    nil,
			wantErr: true,
		},
		{
			name:    "frame too large - header only",
			input:   []byte{0x01, 0, 0, 0},
			want:    nil,
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reader := bytes.NewReader(tt.input)
			data, err := transport.DecodeFrame(reader)

			if (err != nil) != tt.wantErr {
				t.Errorf("DecodeFrame() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if tt.wantErr {
				return
			}

			if !bytes.Equal(data, tt.want) {
				t.Errorf("DecodeFrame() got = %v, want %v", data, tt.want)
			}
		})
	}
}

func TestEncodeDecodeRoundTrip(t *testing.T) {
	testCases := []struct {
		name string
		data string
	}{
		{
			name: "simple JSON",
			data: `{"version":"1.0","id":"msg-123","type":"request"}`,
		},
		{
			name: "ping message",
			data: `{"action":"ping","payload":{}}`,
		},
		{
			name: "large message",
			data: string(bytes.Repeat([]byte("x"), 10000)), // 10KB
		},
		{
			name: "very large message",
			data: string(bytes.Repeat([]byte("a"), 100000)), // 100KB
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Encode
			frame, err := transport.EncodeFrame([]byte(tc.data))
			if err != nil {
				t.Fatalf("EncodeFrame() failed: %v", err)
			}

			// Decode
			reader := bytes.NewReader(frame)
			decoded, err := transport.DecodeFrame(reader)
			if err != nil {
				t.Fatalf("DecodeFrame() failed: %v", err)
			}

			// Verify
			if string(decoded) != tc.data {
				t.Errorf("Round trip failed: got %d bytes, want %d bytes", len(decoded), len(tc.data))
			}
		})
	}
}

func TestFrameReader(t *testing.T) {
	// Create multiple frames
	frames := []string{
		"frame1",
		"frame2 is longer",
		"frame3",
	}

	// Encode all frames
	var buf bytes.Buffer
	for _, frame := range frames {
		data, err := transport.EncodeFrame([]byte(frame))
		if err != nil {
			t.Fatalf("EncodeFrame() failed: %v", err)
		}
		buf.Write(data)
	}

	// Read back using FrameReader
	fr := transport.NewFrameReader(&buf)
	for i, want := range frames {
		data, err := fr.ReadFrame()
		if err != nil {
			t.Fatalf("ReadFrame()[%d] failed: %v", i, err)
		}
		if string(data) != want {
			t.Errorf("ReadFrame()[%d] = %s, want %s", i, data, want)
		}
	}

	// Should get EOF on next read
	_, err := fr.ReadFrame()
	if err != io.EOF {
		t.Errorf("ReadFrame() after all frames = %v, want EOF", err)
	}
}

func TestWriteFrame(t *testing.T) {
	testData := []byte(`{"test":"data"}`)

	var buf bytes.Buffer
	err := transport.WriteFrame(&buf, testData)
	if err != nil {
		t.Fatalf("WriteFrame() failed: %v", err)
	}

	// Read back
	data, err := transport.DecodeFrame(&buf)
	if err != nil {
		t.Fatalf("DecodeFrame() failed: %v", err)
	}

	if !bytes.Equal(data, testData) {
		t.Errorf("WriteFrame/DecodeFrame round trip failed")
	}
}

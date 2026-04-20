// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport

import (
	"bufio"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
)

const (
	// MaxFrameSize is the maximum allowed frame size (16MB)
	MaxFrameSize = 16 * 1024 * 1024
	// FrameHeaderSize is the size of the frame header (4 bytes for length)
	FrameHeaderSize = 4
)

var (
	// ErrFrameTooLarge is returned when a frame exceeds MaxFrameSize
	ErrFrameTooLarge = errors.New("frame size exceeds maximum allowed size")
	// ErrInvalidFrameSize is returned when the frame size is invalid
	ErrInvalidFrameSize = errors.New("invalid frame size")
)

// EncodeFrame encodes data into a frame with length prefix
// Frame format: [4 bytes length (big endian)] + [data]
func EncodeFrame(data []byte) ([]byte, error) {
	dataLen := uint32(len(data))

	// Check frame size limit
	if dataLen > MaxFrameSize {
		return nil, ErrFrameTooLarge
	}

	// Allocate buffer: 4 bytes header + data
	frame := make([]byte, FrameHeaderSize+dataLen)

	// Write length in big endian (network byte order)
	binary.BigEndian.PutUint32(frame[:FrameHeaderSize], dataLen)

	// Copy data
	copy(frame[FrameHeaderSize:], data)

	return frame, nil
}

// DecodeFrame reads and decodes a frame from reader
// Frame format: [4 bytes length (big endian)] + [data]
func DecodeFrame(r io.Reader) ([]byte, error) {
	// Read header (4 bytes length)
	header := make([]byte, FrameHeaderSize)
	n, err := io.ReadFull(r, header)
	if err != nil {
		if err == io.EOF {
			return nil, io.EOF // Clean EOF
		}
		if n == 0 && err == io.ErrUnexpectedEOF {
			return nil, io.EOF // Incomplete read at start is EOF
		}
		return nil, fmt.Errorf("failed to read frame header: %w", err)
	}

	// Parse length
	dataLen := binary.BigEndian.Uint32(header)

	// Validate frame size
	if dataLen > MaxFrameSize {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameTooLarge, dataLen)
	}
	// Zero length is allowed (empty message)
	// if dataLen == 0 {
	// 	return []byte{}, nil
	// }

	// Read data
	data := make([]byte, dataLen)
	if _, err := io.ReadFull(r, data); err != nil {
		if err == io.ErrUnexpectedEOF {
			return nil, io.ErrUnexpectedEOF
		}
		return nil, fmt.Errorf("failed to read frame data: %w", err)
	}

	return data, nil
}

// FrameReader is a buffered reader for efficient frame reading
type FrameReader struct {
	reader *bufio.Reader
}

// NewFrameReader creates a new frame reader with buffered reading
func NewFrameReader(r io.Reader) *FrameReader {
	// Buffer size 4KB for efficient reading
	return &FrameReader{
		reader: bufio.NewReaderSize(r, 4096),
	}
}

// NewFrameReaderWithBufio creates a frame reader from an existing bufio.Reader.
// This is used when a bufio.Reader has already read some data (e.g., auth token)
// and must be reused to avoid data loss.
func NewFrameReaderWithBufio(r *bufio.Reader) *FrameReader {
	return &FrameReader{reader: r}
}

// ReadFrame reads a single frame from the reader
func (fr *FrameReader) ReadFrame() ([]byte, error) {
	return DecodeFrame(fr.reader)
}

// WriteFrame writes a frame to the writer
func WriteFrame(w io.Writer, data []byte) error {
	frame, err := EncodeFrame(data)
	if err != nil {
		return err
	}

	_, err = w.Write(frame)
	return err
}

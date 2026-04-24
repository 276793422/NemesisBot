package agent

import (
	"fmt"
	"sync"
	"testing"
)

func TestRingBuffer_NewRingBuffer(t *testing.T) {
	rb := NewRingBuffer[int](5)
	if rb.Len() != 0 {
		t.Fatalf("expected len 0, got %d", rb.Len())
	}
}

func TestRingBuffer_NewRingBufferZeroSize(t *testing.T) {
	rb := NewRingBuffer[int](0)
	if rb == nil {
		t.Fatal("expected non-nil ring buffer")
	}
	// Should be clamped to 1
	rb.Push(1)
	rb.Push(2)
	items := rb.GetAll()
	if len(items) != 1 || items[0] != 2 {
		t.Fatalf("expected [2] with size clamped to 1, got %v", items)
	}
}

func TestRingBuffer_PushAndGetAll(t *testing.T) {
	rb := NewRingBuffer[int](4)
	rb.Push(1)
	rb.Push(2)
	rb.Push(3)

	items := rb.GetAll()
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	expected := []int{1, 2, 3}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_Overwrite(t *testing.T) {
	rb := NewRingBuffer[int](3)
	rb.Push(1)
	rb.Push(2)
	rb.Push(3)
	rb.Push(4) // overwrites 1
	rb.Push(5) // overwrites 2

	items := rb.GetAll()
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	expected := []int{3, 4, 5}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_GetAllEmpty(t *testing.T) {
	rb := NewRingBuffer[int](5)
	items := rb.GetAll()
	if items != nil {
		t.Fatalf("expected nil for empty buffer, got %v", items)
	}
}

func TestRingBuffer_GetLast(t *testing.T) {
	rb := NewRingBuffer[int](5)
	for i := 1; i <= 5; i++ {
		rb.Push(i)
	}

	// Get last 3
	items := rb.GetLast(3)
	expected := []int{3, 4, 5}
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_GetLastMoreThanCount(t *testing.T) {
	rb := NewRingBuffer[int](5)
	rb.Push(1)
	rb.Push(2)

	// Request more than available
	items := rb.GetLast(10)
	if len(items) != 2 {
		t.Fatalf("expected 2 items (clamped), got %d", len(items))
	}
	expected := []int{1, 2}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_GetLastZeroOrNegative(t *testing.T) {
	rb := NewRingBuffer[int](5)
	rb.Push(1)

	if items := rb.GetLast(0); items != nil {
		t.Fatalf("expected nil for n=0, got %v", items)
	}
	if items := rb.GetLast(-1); items != nil {
		t.Fatalf("expected nil for n=-1, got %v", items)
	}
}

func TestRingBuffer_Clear(t *testing.T) {
	rb := NewRingBuffer[int](5)
	rb.Push(1)
	rb.Push(2)
	rb.Push(3)

	rb.Clear()

	if rb.Len() != 0 {
		t.Fatalf("expected len 0 after clear, got %d", rb.Len())
	}
	if items := rb.GetAll(); items != nil {
		t.Fatalf("expected nil after clear, got %v", items)
	}
}

func TestRingBuffer_ClearThenPush(t *testing.T) {
	rb := NewRingBuffer[int](3)
	rb.Push(1)
	rb.Push(2)
	rb.Clear()
	rb.Push(10)
	rb.Push(20)

	items := rb.GetAll()
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	expected := []int{10, 20}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_GenericString(t *testing.T) {
	rb := NewRingBuffer[string](3)
	rb.Push("hello")
	rb.Push("world")

	items := rb.GetAll()
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	if items[0] != "hello" || items[1] != "world" {
		t.Fatalf("expected [hello, world], got %v", items)
	}
}

func TestRingBuffer_ConcurrentPush(t *testing.T) {
	rb := NewRingBuffer[int](1000)
	var wg sync.WaitGroup

	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(val int) {
			defer wg.Done()
			rb.Push(val)
		}(i)
	}

	wg.Wait()

	// Should have exactly 100 items (100 < capacity of 1000)
	if rb.Len() != 100 {
		t.Fatalf("expected len 100, got %d", rb.Len())
	}
}

func TestRingBuffer_ConcurrentReadWrite(t *testing.T) {
	rb := NewRingBuffer[int](50)
	var wg sync.WaitGroup

	// Concurrent writers
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(val int) {
			defer wg.Done()
			rb.Push(val)
		}(i)
	}

	// Concurrent readers
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = rb.GetAll()
			_ = rb.GetLast(5)
			_ = rb.Len()
		}()
	}

	wg.Wait()
}

func TestRingBuffer_GetLastAfterOverwrite(t *testing.T) {
	rb := NewRingBuffer[int](3)
	rb.Push(1)
	rb.Push(2)
	rb.Push(3)
	rb.Push(4) // overwrites 1

	// Buffer now contains [2, 3, 4]
	last2 := rb.GetLast(2)
	if len(last2) != 2 {
		t.Fatalf("expected 2 items, got %d", len(last2))
	}
	if last2[0] != 3 || last2[1] != 4 {
		t.Fatalf("expected [3, 4], got %v", last2)
	}
}

func TestRingBuffer_FullCycle(t *testing.T) {
	// Test multiple overwrite cycles
	rb := NewRingBuffer[int](3)
	for i := 0; i < 10; i++ {
		rb.Push(i)
	}

	items := rb.GetAll()
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	// Last 3 items: 7, 8, 9
	expected := []int{7, 8, 9}
	for i, v := range items {
		if v != expected[i] {
			t.Fatalf("item %d: expected %d, got %d", i, expected[i], v)
		}
	}
}

func TestRingBuffer_GetAllReturnsNewSlice(t *testing.T) {
	rb := NewRingBuffer[int](5)
	rb.Push(1)
	rb.Push(2)

	items := rb.GetAll()
	// The returned slice header is independent; appending does not affect the buffer
	items = append(items, 3)

	original := rb.GetAll()
	if len(original) != 2 {
		t.Fatalf("expected 2 items in original after append to returned slice, got %d", len(original))
	}
}

func BenchmarkRingBuffer_Push(b *testing.B) {
	rb := NewRingBuffer[int](1000)
	for i := 0; i < b.N; i++ {
		rb.Push(i)
	}
}

func BenchmarkRingBuffer_GetAll(b *testing.B) {
	rb := NewRingBuffer[int](1000)
	for i := 0; i < 1000; i++ {
		rb.Push(i)
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = rb.GetAll()
	}
}

func BenchmarkRingBuffer_GetLast(b *testing.B) {
	rb := NewRingBuffer[int](1000)
	for i := 0; i < 1000; i++ {
		rb.Push(i)
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = rb.GetLast(100)
	}
}

func BenchmarkRingBuffer_ConcurrentPush(b *testing.B) {
	rb := NewRingBuffer[int](10000)
	b.RunParallel(func(pb *testing.PB) {
		i := 0
		for pb.Next() {
			rb.Push(i)
			i++
		}
	})
}

func ExampleRingBuffer() {
	rb := NewRingBuffer[string](3)
	rb.Push("first")
	rb.Push("second")
	rb.Push("third")
	rb.Push("fourth") // overwrites "first"

	fmt.Println(rb.GetAll())
	fmt.Println(rb.GetLast(2))
	// Output:
	// [second third fourth]
	// [third fourth]
}

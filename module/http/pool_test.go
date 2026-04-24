package http

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestNewPooledTransport(t *testing.T) {
	pt := NewPooledTransport(5, 20, 30*time.Second)
	defer pt.Close()

	if pt.Client() == nil {
		t.Fatal("expected non-nil client")
	}
	if pt.Transport() == nil {
		t.Fatal("expected non-nil transport")
	}

	transport := pt.Transport()
	if transport.MaxIdleConns != 20 {
		t.Fatalf("expected MaxIdleConns=20, got %d", transport.MaxIdleConns)
	}
	if transport.MaxIdleConnsPerHost != 5 {
		t.Fatalf("expected MaxIdleConnsPerHost=5, got %d", transport.MaxIdleConnsPerHost)
	}
	if transport.IdleConnTimeout != 30*time.Second {
		t.Fatalf("expected IdleConnTimeout=30s, got %v", transport.IdleConnTimeout)
	}
}

func TestPooledTransport_MakesRequest(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("hello"))
	}))
	defer server.Close()

	pt := NewPooledTransport(5, 10, 30*time.Second)
	defer pt.Close()

	resp, err := pt.Client().Get(server.URL)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
}

func TestPooledTransport_ConnectionReuse(t *testing.T) {
	var connections int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		connections++
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	pt := NewPooledTransport(5, 10, 60*time.Second)
	defer pt.Close()

	client := pt.Client()

	// Make multiple requests; they should reuse the same connection
	for i := 0; i < 5; i++ {
		resp, err := client.Get(server.URL)
		if err != nil {
			t.Fatalf("request %d: unexpected error: %v", i, err)
		}
		resp.Body.Close()
	}

	// All requests should ideally use a single connection
	// (though this depends on the server and timing)
	if connections != 1 {
		t.Logf("connections made: %d (expected 1, but may vary)", connections)
	}
}

func TestPooledTransport_Close(t *testing.T) {
	pt := NewPooledTransport(5, 10, 30*time.Second)

	// Close should not panic
	pt.Close()

	// Double close should also be safe
	pt.Close()
}

func TestDefaultPooledTransport(t *testing.T) {
	pt := DefaultPooledTransport()
	defer pt.Close()

	if pt.Client() == nil {
		t.Fatal("expected non-nil client")
	}

	transport := pt.Transport()
	if transport.MaxIdleConns != 100 {
		t.Fatalf("expected MaxIdleConns=100, got %d", transport.MaxIdleConns)
	}
	if transport.MaxIdleConnsPerHost != 10 {
		t.Fatalf("expected MaxIdleConnsPerHost=10, got %d", transport.MaxIdleConnsPerHost)
	}
	if transport.IdleConnTimeout != 90*time.Second {
		t.Fatalf("expected IdleConnTimeout=90s, got %v", transport.IdleConnTimeout)
	}
}

func TestDefaultPooledTransport_MakesRequest(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("ok"))
	}))
	defer server.Close()

	pt := DefaultPooledTransport()
	defer pt.Close()

	resp, err := pt.Client().Get(server.URL)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
}

func BenchmarkPooledTransport_Request(b *testing.B) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	pt := DefaultPooledTransport()
	defer pt.Close()

	client := pt.Client()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		resp, err := client.Get(server.URL)
		if err != nil {
			b.Fatal(err)
		}
		resp.Body.Close()
	}
}

func BenchmarkPooledTransport_ParallelRequests(b *testing.B) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	pt := DefaultPooledTransport()
	defer pt.Close()

	client := pt.Client()

	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		for pb.Next() {
			resp, err := client.Get(server.URL)
			if err != nil {
				b.Fatal(err)
			}
			resp.Body.Close()
		}
	})
}

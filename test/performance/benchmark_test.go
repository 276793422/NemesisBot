// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package performance

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/cron"
)

// BenchmarkMessageBusPublish benchmarks message publishing performance
func BenchmarkMessageBusPublish(b *testing.B) {
	msgBus := bus.NewMessageBus()

	// Start a subscriber to prevent blocking
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	go func() {
		for {
			_, ok := msgBus.SubscribeOutbound(ctx)
			if !ok {
				break
			}
		}
	}()

	// Warm-up
	msgBus.PublishOutbound(bus.OutboundMessage{})
	time.Sleep(10 * time.Millisecond)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		msgBus.PublishOutbound(bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat",
			Content: "test",
		})
	}
}

// BenchmarkCronJobCreation benchmarks cron job creation
func BenchmarkCronJobCreation(b *testing.B) {
	tempDir := b.TempDir()
	storePath := tempDir + "/cron.json"
	cs := cron.NewCronService(storePath, nil)

	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := cron.CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	// Warm-up
	cs.AddJob("warmup", schedule, "test", false, "", "")

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		cs.AddJob("bench_job", schedule, "test", false, "", "")
	}
}

// BenchmarkCronJobListing benchmarks job listing performance
func BenchmarkCronJobListing(b *testing.B) {
	tempDir := b.TempDir()
	storePath := tempDir + "/cron.json"
	cs := cron.NewCronService(storePath, nil)

	// Create some jobs
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := cron.CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	for i := 0; i < 100; i++ {
		cs.AddJob("job", schedule, "test", false, "", "")
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		cs.ListJobs(true)
	}
}

// BenchmarkInboundMessageCreation benchmarks inbound message creation
func BenchmarkInboundMessageCreation(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = bus.InboundMessage{
			Channel:       "test",
			SenderID:      "user",
			ChatID:        "chat",
			Content:       "test message",
			SessionKey:    "session",
			CorrelationID: "",
		}
	}
}

// BenchmarkOutboundMessageCreation benchmarks outbound message creation
func BenchmarkOutboundMessageCreation(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = bus.OutboundMessage{
			Channel: "test",
			ChatID:  "chat",
			Content: "test message",
		}
	}
}

// BenchmarkMessageCreationParallel benchmarks parallel message creation
func BenchmarkMessageCreationParallel(b *testing.B) {
	b.RunParallel(func(pb *testing.PB) {
		for pb.Next() {
			_ = bus.OutboundMessage{
				Channel: "test",
				ChatID:  "chat",
				Content: "test",
			}
		}
	})
}

// BenchmarkCronScheduleEvery benchmarks "every" schedule computation
func BenchmarkCronScheduleEvery(b *testing.B) {
	everyMS := int64(1000)
	schedule := cron.CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}

	now := time.Now().UnixMilli()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		// Compute next run for "every" schedule
		next := now + *schedule.EveryMS
		_ = next
	}
}

// BenchmarkCronScheduleAt benchmarks "at" schedule computation
func BenchmarkCronScheduleAt(b *testing.B) {
	now := time.Now().UnixMilli()
	atMS := now + 3600000 // 1 hour in future

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		// Check if at schedule is due
		if atMS <= now {
			_ = atMS
		}
	}
}

// BenchmarkStringOperations benchmarks common string operations in message handling
func BenchmarkStringOperations(b *testing.B) {
	content := "This is a test message with some content"

	b.Run("Concatenation", func(b *testing.B) {
		for i := 0; i < b.N; i++ {
			_ = "Prefix: " + content
		}
	})

	b.Run("Formatting", func(b *testing.B) {
		for i := 0; i < b.N; i++ {
			_ = fmt.Sprintf("Message: %s", content)
		}
	})

	b.Run("Contains", func(b *testing.B) {
		for i := 0; i < b.N; i++ {
			_ = contains(content, "test")
		}
	})
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && indexOf(s, substr) >= 0
}

func indexOf(s, substr string) int {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return i
		}
	}
	return -1
}

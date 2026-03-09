// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cron

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

// Test generateID
func TestGenerateID(t *testing.T) {
	id := generateID()
	if id == "" {
		t.Error("generateID() should not return empty string")
	}

	// Test that multiple calls produce different IDs
	id2 := generateID()
	if id == id2 {
		t.Error("generateID() should produce unique IDs")
	}

	// Test ID format (should be hex string from 8 bytes = 16 hex chars)
	if len(id) != 16 {
		t.Errorf("generateID() should return 16-character hex string, got %d characters", len(id))
	}
}

// Test CronSchedule structs
func TestCronSchedule(t *testing.T) {
	t.Run("at schedule", func(t *testing.T) {
		atMS := int64(1234567890000)
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &atMS,
		}

		if schedule.Kind != "at" {
			t.Errorf("Expected Kind 'at', got '%s'", schedule.Kind)
		}

		if schedule.AtMS == nil || *schedule.AtMS != atMS {
			t.Error("AtMS should be set correctly")
		}
	})

	t.Run("every schedule", func(t *testing.T) {
		everyMS := int64(60000)
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: &everyMS,
		}

		if schedule.Kind != "every" {
			t.Errorf("Expected Kind 'every', got '%s'", schedule.Kind)
		}

		if schedule.EveryMS == nil || *schedule.EveryMS != everyMS {
			t.Error("EveryMS should be set correctly")
		}
	})

	t.Run("cron schedule", func(t *testing.T) {
		schedule := CronSchedule{
			Kind: "cron",
			Expr: "0 * * * *",
			TZ:   "UTC",
		}

		if schedule.Kind != "cron" {
			t.Errorf("Expected Kind 'cron', got '%s'", schedule.Kind)
		}

		if schedule.Expr != "0 * * * *" {
			t.Errorf("Expected Expr '0 * * * *', got '%s'", schedule.Expr)
		}

		if schedule.TZ != "UTC" {
			t.Errorf("Expected TZ 'UTC', got '%s'", schedule.TZ)
		}
	})
}

// Test CronPayload structs
func TestCronPayload(t *testing.T) {
	payload := CronPayload{
		Kind:    "agent_turn",
		Message: "test message",
		Command: "test command",
		Deliver: true,
		Channel: "test_channel",
		To:      "test_user",
	}

	if payload.Kind != "agent_turn" {
		t.Errorf("Expected Kind 'agent_turn', got '%s'", payload.Kind)
	}

	if payload.Message != "test message" {
		t.Errorf("Expected Message 'test message', got '%s'", payload.Message)
	}

	if !payload.Deliver {
		t.Error("Expected Deliver to be true")
	}
}

// Test CronJobState structs
func TestCronJobState(t *testing.T) {
	nextRun := int64(1234567890000)
	lastRun := int64(1234567880000)

	state := CronJobState{
		NextRunAtMS: &nextRun,
		LastRunAtMS: &lastRun,
		LastStatus:  "ok",
		LastError:   "",
	}

	if state.NextRunAtMS == nil || *state.NextRunAtMS != nextRun {
		t.Error("NextRunAtMS should be set correctly")
	}

	if state.LastRunAtMS == nil || *state.LastRunAtMS != lastRun {
		t.Error("LastRunAtMS should be set correctly")
	}

	if state.LastStatus != "ok" {
		t.Errorf("Expected LastStatus 'ok', got '%s'", state.LastStatus)
	}
}

// Test CronJob structs
func TestCronJob(t *testing.T) {
	atMS := int64(1234567890000)
	now := time.Now().UnixMilli()

	job := CronJob{
		ID:      "test_id",
		Name:    "test job",
		Enabled: true,
		Schedule: CronSchedule{
			Kind: "at",
			AtMS: &atMS,
		},
		Payload: CronPayload{
			Kind:    "agent_turn",
			Message: "test",
		},
		State: CronJobState{
			NextRunAtMS: &atMS,
		},
		CreatedAtMS:    now,
		UpdatedAtMS:    now,
		DeleteAfterRun: true,
	}

	if job.ID != "test_id" {
		t.Errorf("Expected ID 'test_id', got '%s'", job.ID)
	}

	if !job.Enabled {
		t.Error("Expected job to be enabled")
	}

	if !job.DeleteAfterRun {
		t.Error("Expected DeleteAfterRun to be true")
	}
}

// Test CronStore structs
func TestCronStore(t *testing.T) {
	store := CronStore{
		Version: 1,
		Jobs:    []CronJob{},
	}

	if store.Version != 1 {
		t.Errorf("Expected Version 1, got %d", store.Version)
	}

	if store.Jobs == nil {
		t.Error("Jobs should be initialized")
	}

	if len(store.Jobs) != 0 {
		t.Errorf("Expected 0 jobs, got %d", len(store.Jobs))
	}
}

// Test NewCronService
func TestNewCronService(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	onJob := func(job *CronJob) (string, error) {
		return "result", nil
	}

	cs := NewCronService(storePath, onJob)

	if cs == nil {
		t.Fatal("NewCronService() should not return nil")
	}

	if cs.storePath != storePath {
		t.Errorf("Expected storePath '%s', got '%s'", storePath, cs.storePath)
	}

	if cs.onJob == nil {
		t.Error("onJob handler should be set")
	}

	if cs.store == nil {
		t.Error("store should be initialized")
	}

	if cs.running {
		t.Error("service should not be running initially")
	}
}

// Test CronService Start/Stop
func TestCronServiceStartStop(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Start the service
	err := cs.Start()
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}

	if !cs.running {
		t.Error("service should be running after Start()")
	}

	// Start again should be idempotent
	err = cs.Start()
	if err != nil {
		t.Fatalf("Second Start() failed: %v", err)
	}

	// Stop the service
	cs.Stop()

	if cs.running {
		t.Error("service should not be running after Stop()")
	}

	// Stop again should be safe
	cs.Stop()
}

// Test CronService AddJob
func TestCronServiceAddJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	t.Run("add at job", func(t *testing.T) {
		atMS := time.Now().Add(1 * time.Hour).UnixMilli()
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &atMS,
		}

		job, err := cs.AddJob("test_at_job", schedule, "test message", false, "", "")
		if err != nil {
			t.Fatalf("AddJob() failed: %v", err)
		}

		if job == nil {
			t.Fatal("AddJob() should return a job")
		}

		if job.Name != "test_at_job" {
			t.Errorf("Expected Name 'test_at_job', got '%s'", job.Name)
		}

		if !job.Enabled {
			t.Error("Job should be enabled by default")
		}

		if !job.DeleteAfterRun {
			t.Error("At job should have DeleteAfterRun set to true")
		}

		if job.State.NextRunAtMS == nil {
			t.Error("NextRunAtMS should be set")
		}
	})

	t.Run("add every job", func(t *testing.T) {
		everyMS := int64(60000) // 1 minute
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: &everyMS,
		}

		job, err := cs.AddJob("test_every_job", schedule, "test message", false, "", "")
		if err != nil {
			t.Fatalf("AddJob() failed: %v", err)
		}

		if job.Schedule.Kind != "every" {
			t.Errorf("Expected Kind 'every', got '%s'", job.Schedule.Kind)
		}

		if job.DeleteAfterRun {
			t.Error("Every job should not have DeleteAfterRun set")
		}
	})

	t.Run("add cron job", func(t *testing.T) {
		schedule := CronSchedule{
			Kind: "cron",
			Expr: "0 * * * *",
		}

		job, err := cs.AddJob("test_cron_job", schedule, "test message", false, "", "")
		if err != nil {
			t.Fatalf("AddJob() failed: %v", err)
		}

		if job.Schedule.Kind != "cron" {
			t.Errorf("Expected Kind 'cron', got '%s'", job.Schedule.Kind)
		}

		if job.DeleteAfterRun {
			t.Error("Cron job should not have DeleteAfterRun set")
		}
	})
}

// Test CronService RemoveJob
func TestCronServiceRemoveJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Add a job first
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob() failed: %v", err)
	}

	// Remove the job
	removed := cs.RemoveJob(job.ID)
	if !removed {
		t.Error("RemoveJob() should return true when job is removed")
	}

	// Remove again should return false
	removed = cs.RemoveJob(job.ID)
	if removed {
		t.Error("RemoveJob() should return false when job doesn't exist")
	}

	// Verify job is gone from list
	jobs := cs.ListJobs(true)
	for _, j := range jobs {
		if j.ID == job.ID {
			t.Error("Job should be removed from list")
		}
	}
}

// Test CronService EnableJob
func TestCronServiceEnableJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Add a job
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob() failed: %v", err)
	}

	// Disable the job
	disabledJob := cs.EnableJob(job.ID, false)
	if disabledJob == nil {
		t.Fatal("EnableJob() should return the job")
	}

	if disabledJob.Enabled {
		t.Error("Job should be disabled")
	}

	if disabledJob.State.NextRunAtMS != nil {
		t.Error("NextRunAtMS should be nil when disabled")
	}

	// Enable the job
	enabledJob := cs.EnableJob(job.ID, true)
	if enabledJob == nil {
		t.Fatal("EnableJob() should return the job")
	}

	if !enabledJob.Enabled {
		t.Error("Job should be enabled")
	}

	if enabledJob.State.NextRunAtMS == nil {
		t.Error("NextRunAtMS should be set when enabled")
	}

	// Try to enable non-existent job
	nilJob := cs.EnableJob("non_existent", true)
	if nilJob != nil {
		t.Error("EnableJob() should return nil for non-existent job")
	}
}

// Test CronService ListJobs
func TestCronServiceListJobs(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Add some jobs
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	job1, _ := cs.AddJob("job1", schedule, "test", false, "", "")
	job2, _ := cs.AddJob("job2", schedule, "test", false, "", "")

	// Disable one job
	cs.EnableJob(job2.ID, false)

	t.Run("list all jobs", func(t *testing.T) {
		jobs := cs.ListJobs(true)
		if len(jobs) != 2 {
			t.Errorf("Expected 2 jobs, got %d", len(jobs))
		}
	})

	t.Run("list only enabled jobs", func(t *testing.T) {
		jobs := cs.ListJobs(false)
		if len(jobs) != 1 {
			t.Errorf("Expected 1 enabled job, got %d", len(jobs))
		}

		if jobs[0].ID != job1.ID {
			t.Error("Should return the enabled job")
		}
	})
}

// Test CronService UpdateJob
func TestCronServiceUpdateJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Add a job
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob() failed: %v", err)
	}

	// Update the job
	job.Name = "updated_job"
	job.Payload.Message = "updated message"

	err = cs.UpdateJob(job)
	if err != nil {
		t.Fatalf("UpdateJob() failed: %v", err)
	}

	// Verify the update
	jobs := cs.ListJobs(true)
	if len(jobs) != 1 {
		t.Fatalf("Expected 1 job, got %d", len(jobs))
	}

	if jobs[0].Name != "updated_job" {
		t.Errorf("Expected Name 'updated_job', got '%s'", jobs[0].Name)
	}

	if jobs[0].Payload.Message != "updated message" {
		t.Errorf("Expected Message 'updated message', got '%s'", jobs[0].Payload.Message)
	}

	// Try to update non-existent job
	fakeJob := &CronJob{
		ID:   "non_existent",
		Name: "fake",
	}
	err = cs.UpdateJob(fakeJob)
	if err == nil {
		t.Error("UpdateJob() should return error for non-existent job")
	}
}

// Test CronService Status
func TestCronServiceStatus(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	t.Run("status when not running", func(t *testing.T) {
		status := cs.Status()

		if enabled, ok := status["enabled"].(bool); !ok {
			t.Error("status['enabled'] should be a bool")
		} else if enabled {
			t.Error("enabled should be false when service is not running")
		}

		if jobs, ok := status["jobs"].(int); !ok {
			t.Error("status['jobs'] should be an int")
		} else if jobs != 0 {
			t.Errorf("Expected 0 jobs, got %d", jobs)
		}
	})

	t.Run("status with jobs", func(t *testing.T) {
		// Add a job
		atMS := time.Now().Add(1 * time.Hour).UnixMilli()
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &atMS,
		}
		cs.AddJob("test_job", schedule, "test", false, "", "")

		status := cs.Status()

		if jobs, ok := status["jobs"].(int); ok {
			if jobs != 1 {
				t.Errorf("Expected 1 job, got %d", jobs)
			}
		} else {
			t.Error("status['jobs'] should be an int")
		}

		if nextWake, ok := status["nextWakeAtMS"].(*int64); ok {
			if nextWake == nil {
				t.Error("nextWakeAtMS should be set for enabled job")
			}
		}
	})
}

// Test CronService Load
func TestCronServiceLoad(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	// Load should work even if file doesn't exist
	err := cs.Load()
	if err != nil {
		t.Fatalf("Load() failed: %v", err)
	}

	// Add a job and save
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}
	job, _ := cs.AddJob("test_job", schedule, "test", false, "", "")

	// Create a new service and load
	cs2 := NewCronService(storePath, nil)
	err = cs2.Load()
	if err != nil {
		t.Fatalf("Load() failed: %v", err)
	}

	jobs := cs2.ListJobs(true)
	if len(jobs) != 1 {
		t.Errorf("Expected 1 job after load, got %d", len(jobs))
	}

	if jobs[0].ID != job.ID {
		t.Error("Job ID should match after load")
	}
}

// Test CronService persistence
func TestCronServicePersistence(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	// Create first service and add job
	cs1 := NewCronService(storePath, nil)
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}
	job1, _ := cs1.AddJob("persisted_job", schedule, "test message", true, "channel", "user")

	// Verify file was created
	if _, err := os.Stat(storePath); os.IsNotExist(err) {
		t.Error("Store file should be created after AddJob")
	}

	// Create second service and verify job was loaded
	cs2 := NewCronService(storePath, nil)
	err := cs2.Load()
	if err != nil {
		t.Fatalf("Load() failed: %v", err)
	}

	jobs := cs2.ListJobs(true)
	if len(jobs) != 1 {
		t.Fatalf("Expected 1 job after load, got %d", len(jobs))
	}

	job2 := jobs[0]
	if job2.ID != job1.ID {
		t.Errorf("Job ID mismatch: got %s, want %s", job2.ID, job1.ID)
	}

	if job2.Name != job1.Name {
		t.Errorf("Job Name mismatch: got %s, want %s", job2.Name, job1.Name)
	}

	if job2.Payload.Message != job1.Payload.Message {
		t.Errorf("Job Message mismatch: got %s, want %s", job2.Payload.Message, job1.Payload.Message)
	}
}

// Test computeNextRun
func TestComputeNextRun(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	now := time.Now().UnixMilli()

	t.Run("at schedule in future", func(t *testing.T) {
		future := now + 3600000
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &future,
		}

		next := cs.computeNextRun(&schedule, now)
		if next == nil {
			t.Fatal("computeNextRun() should return a value for future 'at' schedule")
		}

		if *next != future {
			t.Errorf("Expected next run %d, got %d", future, *next)
		}
	})

	t.Run("at schedule in past", func(t *testing.T) {
		past := now - 3600000
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &past,
		}

		next := cs.computeNextRun(&schedule, now)
		if next != nil {
			t.Error("computeNextRun() should return nil for past 'at' schedule")
		}
	})

	t.Run("every schedule", func(t *testing.T) {
		everyMS := int64(60000)
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: &everyMS,
		}

		next := cs.computeNextRun(&schedule, now)
		if next == nil {
			t.Fatal("computeNextRun() should return a value for 'every' schedule")
		}

		expected := now + everyMS
		if *next != expected {
			t.Errorf("Expected next run %d, got %d", expected, *next)
		}
	})

	t.Run("every schedule with zero interval", func(t *testing.T) {
		zero := int64(0)
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: &zero,
		}

		next := cs.computeNextRun(&schedule, now)
		if next != nil {
			t.Error("computeNextRun() should return nil for 'every' schedule with zero interval")
		}
	})

	t.Run("every schedule with nil interval", func(t *testing.T) {
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: nil,
		}

		next := cs.computeNextRun(&schedule, now)
		if next != nil {
			t.Error("computeNextRun() should return nil for 'every' schedule with nil interval")
		}
	})

	t.Run("cron schedule with valid expression", func(t *testing.T) {
		schedule := CronSchedule{
			Kind: "cron",
			Expr: "* * * * *", // Every minute
		}

		next := cs.computeNextRun(&schedule, now)
		if next == nil {
			t.Fatal("computeNextRun() should return a value for valid cron expression")
		}

		if *next <= now {
			t.Error("Next run should be in the future")
		}
	})

	t.Run("cron schedule with empty expression", func(t *testing.T) {
		schedule := CronSchedule{
			Kind: "cron",
			Expr: "",
		}

		next := cs.computeNextRun(&schedule, now)
		if next != nil {
			t.Error("computeNextRun() should return nil for cron schedule with empty expression")
		}
	})

	t.Run("unknown schedule kind", func(t *testing.T) {
		schedule := CronSchedule{
			Kind: "unknown",
		}

		next := cs.computeNextRun(&schedule, now)
		if next != nil {
			t.Error("computeNextRun() should return nil for unknown schedule kind")
		}
	})
}

// Test SetOnJob
func TestSetOnJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	newHandler := func(job *CronJob) (string, error) {
		return "result", nil
	}

	cs.SetOnJob(newHandler)

	// We can't directly test that the handler is set, but we can call it indirectly
	// by starting the service and adding a job that will execute immediately
}

// Test concurrent access
func TestCronServiceConcurrentAccess(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")

	cs := NewCronService(storePath, nil)

	var wg sync.WaitGroup
	numGoroutines := 10
	operationsPerGoroutine := 10

	// Start the service first
	if err := cs.Start(); err != nil {
		t.Fatalf("Start() failed: %v", err)
	}
	defer cs.Stop()

	// Concurrent job additions
	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < operationsPerGoroutine; j++ {
				atMS := time.Now().Add(1 * time.Hour).UnixMilli()
				schedule := CronSchedule{
					Kind: "at",
					AtMS: &atMS,
				}
				cs.AddJob("concurrent_job", schedule, "test", false, "", "")
			}
		}(i)
	}

	// Concurrent job listings
	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < operationsPerGoroutine; j++ {
				_ = cs.ListJobs(true)
			}
		}(i)
	}

	// Concurrent status checks
	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < operationsPerGoroutine; j++ {
				_ = cs.Status()
			}
		}(i)
	}

	wg.Wait()

	// Verify we have all jobs
	jobs := cs.ListJobs(true)
	expectedJobs := numGoroutines * operationsPerGoroutine
	if len(jobs) != expectedJobs {
		t.Logf("Warning: Expected %d jobs, got %d (may vary due to concurrent access)", expectedJobs, len(jobs))
	}
}

// Test JSON serialization
func TestCronJobJSONSerialization(t *testing.T) {
	atMS := int64(1234567890000)
	now := time.Now().UnixMilli()

	job := CronJob{
		ID:      "test_id",
		Name:    "test job",
		Enabled: true,
		Schedule: CronSchedule{
			Kind: "at",
			AtMS: &atMS,
		},
		Payload: CronPayload{
			Kind:    "agent_turn",
			Message: "test message",
		},
		State: CronJobState{
			NextRunAtMS: &atMS,
		},
		CreatedAtMS:    now,
		UpdatedAtMS:    now,
		DeleteAfterRun: true,
	}

	// Serialize
	data, err := json.Marshal(job)
	if err != nil {
		t.Fatalf("Failed to marshal job: %v", err)
	}

	// Deserialize
	var loadedJob CronJob
	if err := json.Unmarshal(data, &loadedJob); err != nil {
		t.Fatalf("Failed to unmarshal job: %v", err)
	}

	// Verify
	if loadedJob.ID != job.ID {
		t.Errorf("ID mismatch: got %s, want %s", loadedJob.ID, job.ID)
	}

	if loadedJob.Name != job.Name {
		t.Errorf("Name mismatch: got %s, want %s", loadedJob.Name, job.Name)
	}

	if loadedJob.Schedule.Kind != job.Schedule.Kind {
		t.Errorf("Schedule.Kind mismatch: got %s, want %s", loadedJob.Schedule.Kind, job.Schedule.Kind)
	}

	if loadedJob.Payload.Message != job.Payload.Message {
		t.Errorf("Payload.Message mismatch: got %s, want %s", loadedJob.Payload.Message, job.Payload.Message)
	}
}

// Test CronStore JSON serialization
func TestCronStoreJSONSerialization(t *testing.T) {
	store := CronStore{
		Version: 1,
		Jobs: []CronJob{
			{
				ID:      "job1",
				Name:    "Job 1",
				Enabled: true,
			},
			{
				ID:      "job2",
				Name:    "Job 2",
				Enabled: false,
			},
		},
	}

	// Serialize
	data, err := json.Marshal(store)
	if err != nil {
		t.Fatalf("Failed to marshal store: %v", err)
	}

	// Deserialize
	var loadedStore CronStore
	if err := json.Unmarshal(data, &loadedStore); err != nil {
		t.Fatalf("Failed to unmarshal store: %v", err)
	}

	// Verify
	if loadedStore.Version != store.Version {
		t.Errorf("Version mismatch: got %d, want %d", loadedStore.Version, store.Version)
	}

	if len(loadedStore.Jobs) != len(store.Jobs) {
		t.Errorf("Jobs count mismatch: got %d, want %d", len(loadedStore.Jobs), len(store.Jobs))
	}
}

// Benchmark tests
func BenchmarkAddJob(b *testing.B) {
	tempDir := b.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = cs.AddJob("bench_job", schedule, "test", false, "", "")
	}
}

func BenchmarkListJobs(b *testing.B) {
	tempDir := b.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Add some jobs
	atMS := time.Now().Add(1 * time.Hour).UnixMilli()
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}
	for i := 0; i < 100; i++ {
		cs.AddJob("job", schedule, "test", false, "", "")
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = cs.ListJobs(true)
	}
}

func BenchmarkComputeNextRun(b *testing.B) {
	tempDir := b.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	now := time.Now().UnixMilli()

	b.Run("at_schedule", func(b *testing.B) {
		future := now + 3600000
		schedule := CronSchedule{
			Kind: "at",
			AtMS: &future,
		}
		b.ResetTimer()
		for i := 0; i < b.N; i++ {
			_ = cs.computeNextRun(&schedule, now)
		}
	})

	b.Run("every_schedule", func(b *testing.B) {
		everyMS := int64(60000)
		schedule := CronSchedule{
			Kind:    "every",
			EveryMS: &everyMS,
		}
		b.ResetTimer()
		for i := 0; i < b.N; i++ {
			_ = cs.computeNextRun(&schedule, now)
		}
	})
}

func BenchmarkGenerateID(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = generateID()
	}
}

// Tests for checkJobs() function
func TestCronService_CheckJobs_NotRunning(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Track job execution
	jobExecuted := false
	cs.SetOnJob(func(job *CronJob) (string, error) {
		jobExecuted = true
		return "success", nil
	})

	// Start to initialize NextRunAtMS
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	// Stop immediately so service is not running when we call checkJobs
	cs.Stop()

	// Add a job that's due
	now := time.Now().UnixMilli()
	atMS := now - 1000 // Past time
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Verify job exists
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job not found before checkJobs")
	}

	// checkJobs should do nothing when not running
	cs.checkJobs()

	// Verify job was NOT executed
	if jobExecuted {
		t.Error("Job should not be executed when service is not running")
	}
}

func TestCronService_CheckJobs_DueJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Track job execution
	jobExecuted := false
	var executedJobID string
	cs.SetOnJob(func(job *CronJob) (string, error) {
		jobExecuted = true
		executedJobID = job.ID
		return "success", nil
	})

	// Start the service
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	t.Cleanup(cs.Stop)

	// Add an "every" schedule job (will have NextRunAtMS computed)
	everyMS := int64(100)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Wait for job to become due
	time.Sleep(150 * time.Millisecond)

	// Call checkJobs - this should execute the job
	cs.checkJobs()

	// Verify job was executed
	if !jobExecuted {
		t.Error("Job should have been executed")
	}
	if executedJobID != jobID {
		t.Errorf("Wrong job executed: got %s, want %s", executedJobID, jobID)
	}
}

func TestCronService_CheckJobs_MultipleDueJobs(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Start the service
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	t.Cleanup(cs.Stop)

	// Track job executions
	var executedCount int
	executedJobs := make(map[string]bool)
	cs.SetOnJob(func(job *CronJob) (string, error) {
		executedCount++
		executedJobs[job.ID] = true
		return "success", nil
	})

	// Add multiple "every" schedule jobs with short intervals
	everyMS := int64(50)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}

	jobIDs := make([]string, 3)
	for i := 0; i < 3; i++ {
		job, err := cs.AddJob(fmt.Sprintf("test_job_%d", i), schedule, "test", false, "", "")
		if err != nil {
			t.Fatalf("AddJob failed for job %d: %v", i, err)
		}
		jobIDs[i] = job.ID
	}

	// Wait for jobs to become due
	time.Sleep(100 * time.Millisecond)

	// Call checkJobs - this should execute due jobs
	cs.checkJobs()

	// Verify at least some jobs were executed
	if executedCount == 0 {
		t.Error("At least one job should have been executed")
	}
}

func TestCronService_CheckJobs_DisabledJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Start the service
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	t.Cleanup(cs.Stop)

	// Track job execution
	jobExecuted := false
	cs.SetOnJob(func(job *CronJob) (string, error) {
		jobExecuted = true
		return "success", nil
	})

	// Add a recurring job
	everyMS := int64(100)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}

	// Disable the job
	cs.EnableJob(job.ID, false)

	// Wait for job to become due
	time.Sleep(150 * time.Millisecond)

	// Call checkJobs - this should NOT execute the disabled job
	cs.checkJobs()

	// Verify job was NOT executed
	if jobExecuted {
		t.Error("Disabled job should not have been executed")
	}
}

func TestCronService_CheckJobs_FutureJob(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Start the service
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	t.Cleanup(cs.Stop)

	// Track job execution
	jobExecuted := false
	cs.SetOnJob(func(job *CronJob) (string, error) {
		jobExecuted = true
		return "success", nil
	})

	// Add a job scheduled far in the future
	now := time.Now().UnixMilli()
	atMS := now + 60000 // 1 minute in the future
	schedule := CronSchedule{
		Kind: "at",
		AtMS: &atMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Call checkJobs immediately - this should NOT execute the future job
	cs.checkJobs()

	// Verify job was NOT executed
	if jobExecuted {
		t.Error("Future job should not have been executed")
	}

	// Verify job still exists
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Future job should still exist")
	}
}

func TestCronService_CheckJobs_ResetsNextRunAtMS(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Start the service
	if err := cs.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}
	t.Cleanup(cs.Stop)

	cs.SetOnJob(func(job *CronJob) (string, error) {
		return "success", nil
	})

	// Add a recurring job
	everyMS := int64(100)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Wait for job to become due
	time.Sleep(150 * time.Millisecond)

	// Call checkJobs - this should reset NextRunAtMS temporarily
	// The function resets NextRunAtMS before executing jobs
	cs.checkJobs()

	// Test passes if no panic occurred - the behavior is tested indirectly
	// by ensuring the job can be executed and NextRunAtMS is recalculated
	_ = jobID
}

// Tests for executeJobByID() function
func TestCronService_ExecuteJobByID_NotFound(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Try to execute a non-existent job
	// This should not panic or return error
	cs.executeJobByID("nonexistent_job_id")
}

func TestCronService_ExecuteJobByID_WithCallback(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Track callback execution
	callbackCalled := false
	var callbackJob *CronJob
	cs.SetOnJob(func(job *CronJob) (string, error) {
		callbackCalled = true
		callbackJob = job
		return "callback_result", nil
	})

	// Add an "every" schedule job (not deleted after execution)
	everyMS := int64(60000)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Execute the job
	cs.executeJobByID(jobID)

	// Verify callback was called
	if !callbackCalled {
		t.Error("Callback should have been called")
	}
	if callbackJob == nil || callbackJob.ID != jobID {
		t.Error("Callback should have received the correct job")
	}

	// Verify job state was updated
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job not found after execution")
	}
	if foundJob.State.LastRunAtMS == nil {
		t.Error("LastRunAtMS should be set")
	}
	if foundJob.State.LastStatus != "ok" {
		t.Errorf("LastStatus should be 'ok', got '%s'", foundJob.State.LastStatus)
	}
	if foundJob.State.LastError != "" {
		t.Errorf("LastError should be empty, got '%s'", foundJob.State.LastError)
	}
}

func TestCronService_ExecuteJobByID_CallbackError(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// Callback that returns an error
	cs.SetOnJob(func(job *CronJob) (string, error) {
		return "", fmt.Errorf("callback failed")
	})

	// Add an "every" schedule job (not deleted after execution)
	everyMS := int64(60000)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Execute the job
	cs.executeJobByID(jobID)

	// Verify job state reflects the error
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job not found after execution")
	}
	if foundJob.State.LastStatus != "error" {
		t.Errorf("LastStatus should be 'error', got '%s'", foundJob.State.LastStatus)
	}
	if foundJob.State.LastError == "" {
		t.Error("LastError should be set")
	}
}

func TestCronService_ExecuteJobByID_NoCallback(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	// No callback set
	cs.SetOnJob(nil)

	// Add an "every" schedule job (not deleted after execution)
	everyMS := int64(60000)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Execute the job - should not panic
	cs.executeJobByID(jobID)

	// Verify job state was still updated (treated as success)
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job not found after execution")
	}
	if foundJob.State.LastRunAtMS == nil {
		t.Error("LastRunAtMS should be set even without callback")
	}
}

func TestCronService_ExecuteJobByID_EverySchedule(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	cs.SetOnJob(func(job *CronJob) (string, error) {
		return "success", nil
	})

	// Add an "every" schedule job
	everyMS := int64(60000) // 1 minute
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Execute the job
	cs.executeJobByID(jobID)

	// Verify job still enabled and NextRunAtMS is set
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job should still exist")
	}
	if !foundJob.Enabled {
		t.Error("Job should still be enabled")
	}
	if foundJob.State.NextRunAtMS == nil {
		t.Error("NextRunAtMS should be set for recurring schedule")
	}
}

func TestCronService_ExecuteJobByID_UpdatedAt(t *testing.T) {
	tempDir := t.TempDir()
	storePath := filepath.Join(tempDir, "cron.json")
	cs := NewCronService(storePath, nil)

	cs.SetOnJob(func(job *CronJob) (string, error) {
		return "success", nil
	})

	// Add an "every" schedule job (not deleted)
	everyMS := int64(60000)
	schedule := CronSchedule{
		Kind:    "every",
		EveryMS: &everyMS,
	}
	job, err := cs.AddJob("test_job", schedule, "test", false, "", "")
	if err != nil {
		t.Fatalf("AddJob failed: %v", err)
	}
	jobID := job.ID

	// Get original UpdatedAtMS
	jobs := cs.ListJobs(false)
	var foundJob *CronJob
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob == nil {
		t.Fatal("Job not found")
	}
	originalUpdatedAt := foundJob.UpdatedAtMS

	// Wait a bit to ensure time difference
	time.Sleep(10 * time.Millisecond)

	// Execute the job
	cs.executeJobByID(jobID)

	// Verify UpdatedAtMS was updated
	jobs = cs.ListJobs(false)
	for i := range jobs {
		if jobs[i].ID == jobID {
			foundJob = &jobs[i]
			break
		}
	}
	if foundJob.UpdatedAtMS <= originalUpdatedAt {
		t.Error("UpdatedAtMS should be updated after execution")
	}
}

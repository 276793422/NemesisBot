package workflow_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestNewTriggerManager(t *testing.T) {
	tm := workflow.NewTriggerManager()
	if tm == nil {
		t.Fatal("NewTriggerManager() returned nil")
	}
}

func TestTriggerManager_RegisterTrigger(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tests := []struct {
		name    string
		trigger workflow.TriggerConfig
		wantErr bool
	}{
		{
			name: "cron trigger",
			trigger: workflow.TriggerConfig{
				Type:   "cron",
				Config: map[string]interface{}{"expression": "*/5 * * * *"},
			},
			wantErr: false,
		},
		{
			name: "webhook trigger",
			trigger: workflow.TriggerConfig{
				Type:   "webhook",
				Config: map[string]interface{}{"path": "/hook"},
			},
			wantErr: false,
		},
		{
			name: "event trigger",
			trigger: workflow.TriggerConfig{
				Type:   "event",
				Config: map[string]interface{}{"event_type": "deploy"},
			},
			wantErr: false,
		},
		{
			name: "message trigger",
			trigger: workflow.TriggerConfig{
				Type:   "message",
				Config: map[string]interface{}{"pattern": "hello*"},
			},
			wantErr: false,
		},
		{
			name: "unknown trigger type",
			trigger: workflow.TriggerConfig{
				Type:   "invalid",
				Config: map[string]interface{}{},
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tm.RegisterTrigger("test-wf", tt.trigger)
			if (err != nil) != tt.wantErr {
				t.Errorf("RegisterTrigger() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

func TestTriggerManager_RemoveTrigger(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "*/5 * * * *"},
	})

	err := tm.RemoveTrigger("wf1")
	if err != nil {
		t.Fatalf("RemoveTrigger() error: %v", err)
	}

	// Verify triggers are removed
	triggers := tm.ListTriggers("wf1")
	if len(triggers) != 0 {
		t.Errorf("ListTriggers(wf1) = %d, want 0 after removal", len(triggers))
	}
}

func TestTriggerManager_RemoveTrigger_Nonexistent(t *testing.T) {
	tm := workflow.NewTriggerManager()

	// Should not error on nonexistent workflow
	err := tm.RemoveTrigger("nonexistent")
	if err != nil {
		t.Fatalf("RemoveTrigger() error: %v", err)
	}
}

func TestTriggerManager_RegisterWorkflowTriggers(t *testing.T) {
	tm := workflow.NewTriggerManager()

	wf := &workflow.Workflow{
		Name: "triggered-wf",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "llm", Config: map[string]interface{}{"prompt": "test"}},
		},
		Triggers: []workflow.TriggerConfig{
			{Type: "cron", Config: map[string]interface{}{"expression": "0 * * * *"}},
			{Type: "webhook", Config: map[string]interface{}{"path": "/api/trigger"}},
			{Type: "message", Config: map[string]interface{}{"pattern": "start*"}},
		},
	}

	err := tm.RegisterWorkflowTriggers(wf)
	if err != nil {
		t.Fatalf("RegisterWorkflowTriggers() error: %v", err)
	}

	triggers := tm.ListTriggers("triggered-wf")
	if len(triggers) != 3 {
		t.Errorf("ListTriggers() = %d, want 3", len(triggers))
	}
}

func TestTriggerManager_RegisterWorkflowTriggers_InvalidType(t *testing.T) {
	tm := workflow.NewTriggerManager()

	wf := &workflow.Workflow{
		Name: "bad-triggers",
		Nodes: []workflow.NodeDef{
			{ID: "n1", Type: "llm", Config: map[string]interface{}{"prompt": "test"}},
		},
		Triggers: []workflow.TriggerConfig{
			{Type: "cron", Config: map[string]interface{}{"expression": "0 * * * *"}},
			{Type: "invalid_type", Config: map[string]interface{}{}},
		},
	}

	err := tm.RegisterWorkflowTriggers(wf)
	if err == nil {
		t.Fatal("expected error for invalid trigger type")
	}
}

func TestTriggerManager_MatchEvent_NoFilter(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{}, // no filter = match all events
	})

	matched := tm.MatchEvent("event", map[string]interface{}{"anything": "goes"})
	if len(matched) != 1 || matched[0] != "wf1" {
		t.Errorf("MatchEvent() = %v, want [wf1]", matched)
	}
}

func TestTriggerManager_MatchEvent_WithFilter(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{"event_type": "deploy"},
	})

	tests := []struct {
		name     string
		data     map[string]interface{}
		expected int
	}{
		{"matching event", map[string]interface{}{"event_type": "deploy"}, 1},
		{"non-matching event", map[string]interface{}{"event_type": "build"}, 0},
		{"missing key", map[string]interface{}{"other": "value"}, 0},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			matched := tm.MatchEvent("event", tt.data)
			if len(matched) != tt.expected {
				t.Errorf("MatchEvent() = %v, expected %d matches", matched, tt.expected)
			}
		})
	}
}

func TestTriggerManager_MatchEvent_WrongType(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{},
	})

	// Looking for "message" triggers when only "event" is registered
	matched := tm.MatchEvent("message", map[string]interface{}{})
	if len(matched) != 0 {
		t.Errorf("MatchEvent(message) = %v, want empty", matched)
	}
}

func TestTriggerManager_MatchEvent_MessagePattern(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("chat-bot", workflow.TriggerConfig{
		Type:   "message",
		Config: map[string]interface{}{"pattern": "hello*"},
	})

	tests := []struct {
		name     string
		data     map[string]interface{}
		expected int
	}{
		{"glob match", map[string]interface{}{"pattern": "hello world"}, 1},
		{"glob no match", map[string]interface{}{"pattern": "goodbye"}, 0},
		{"exact match", map[string]interface{}{"pattern": "hello*"}, 1},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			matched := tm.MatchEvent("message", tt.data)
			if len(matched) != tt.expected {
				t.Errorf("MatchEvent() = %v, expected %d matches", matched, tt.expected)
			}
		})
	}
}

func TestTriggerManager_MatchEvent_MultipleWorkflows(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{"event_type": "deploy"},
	})
	tm.RegisterTrigger("wf2", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{"event_type": "deploy"},
	})

	matched := tm.MatchEvent("event", map[string]interface{}{"event_type": "deploy"})
	if len(matched) != 2 {
		t.Errorf("MatchEvent() = %v, expected 2 matches", matched)
	}
}

func TestTriggerManager_MatchEvent_OneMatchPerWorkflow(t *testing.T) {
	tm := workflow.NewTriggerManager()

	// Register two triggers for the same workflow
	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{"event_type": "deploy"},
	})
	tm.RegisterTrigger("wf1", workflow.TriggerConfig{
		Type:   "event",
		Config: map[string]interface{}{"event_type": "deploy"},
	})

	// Should only get one match for wf1
	matched := tm.MatchEvent("event", map[string]interface{}{"event_type": "deploy"})
	if len(matched) != 1 {
		t.Errorf("MatchEvent() = %v, expected 1 match (one per workflow)", matched)
	}
}

func TestTriggerManager_GetCronWorkflows(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("cron-wf1", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "*/5 * * * *"},
	})
	tm.RegisterTrigger("cron-wf2", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "0 * * * *"},
	})
	tm.RegisterTrigger("webhook-wf", workflow.TriggerConfig{
		Type:   "webhook",
		Config: map[string]interface{}{"path": "/hook"},
	})

	cronWFs := tm.GetCronWorkflows()
	if len(cronWFs) != 2 {
		t.Fatalf("GetCronWorkflows() = %d entries, want 2", len(cronWFs))
	}
	if len(cronWFs["cron-wf1"]) != 1 || cronWFs["cron-wf1"][0] != "*/5 * * * *" {
		t.Errorf("cron-wf1 expressions = %v, want [*/5 * * * *]", cronWFs["cron-wf1"])
	}
	if len(cronWFs["cron-wf2"]) != 1 || cronWFs["cron-wf2"][0] != "0 * * * *" {
		t.Errorf("cron-wf2 expressions = %v, want [0 * * * *]", cronWFs["cron-wf2"])
	}

	// Verify copy (not a reference)
	cronWFs["cron-wf1"] = nil
	original := tm.GetCronWorkflows()
	if original["cron-wf1"] == nil {
		t.Error("GetCronWorkflows() returned a reference, not a copy")
	}
}

func TestTriggerManager_GetWebhookWorkflows(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("hook-wf", workflow.TriggerConfig{
		Type:   "webhook",
		Config: map[string]interface{}{"path": "/hook"},
	})
	tm.RegisterTrigger("cron-wf", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "*/5 * * * *"},
	})

	webhookWFs := tm.GetWebhookWorkflows()
	if len(webhookWFs) != 1 {
		t.Fatalf("GetWebhookWorkflows() = %d, want 1", len(webhookWFs))
	}
	if webhookWFs[0] != "hook-wf" {
		t.Errorf("webhook workflow = %q, want %q", webhookWFs[0], "hook-wf")
	}
}

func TestTriggerManager_ListTriggers(t *testing.T) {
	tm := workflow.NewTriggerManager()

	trigger1 := workflow.TriggerConfig{Type: "cron", Config: map[string]interface{}{"expression": "0 * * * *"}}
	trigger2 := workflow.TriggerConfig{Type: "webhook", Config: map[string]interface{}{"path": "/hook"}}

	tm.RegisterTrigger("wf1", trigger1)
	tm.RegisterTrigger("wf1", trigger2)

	triggers := tm.ListTriggers("wf1")
	if len(triggers) != 2 {
		t.Fatalf("ListTriggers() = %d, want 2", len(triggers))
	}

	// Verify it's a copy
	triggers[0].Type = "modified"
	original := tm.ListTriggers("wf1")
	if original[0].Type == "modified" {
		t.Error("ListTriggers() returned a reference, not a copy")
	}
}

func TestTriggerManager_ListTriggers_NoTriggers(t *testing.T) {
	tm := workflow.NewTriggerManager()

	triggers := tm.ListTriggers("nonexistent")
	if len(triggers) != 0 {
		t.Errorf("ListTriggers(nonexistent) = %d, want 0", len(triggers))
	}
}

func TestTriggerManager_ListAllTriggers(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("wf1", workflow.TriggerConfig{Type: "cron", Config: map[string]interface{}{"expression": "0 * * * *"}})
	tm.RegisterTrigger("wf2", workflow.TriggerConfig{Type: "webhook", Config: map[string]interface{}{"path": "/hook"}})

	all := tm.ListAllTriggers()
	if len(all) != 2 {
		t.Fatalf("ListAllTriggers() = %d entries, want 2", len(all))
	}
	if len(all["wf1"]) != 1 {
		t.Errorf("wf1 triggers = %d, want 1", len(all["wf1"]))
	}
	if len(all["wf2"]) != 1 {
		t.Errorf("wf2 triggers = %d, want 1", len(all["wf2"]))
	}
}

func TestTriggerManager_CronNoExpression(t *testing.T) {
	tm := workflow.NewTriggerManager()

	// Register a cron trigger without expression
	err := tm.RegisterTrigger("no-expr", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{},
	})
	if err != nil {
		t.Fatalf("RegisterTrigger() error: %v", err)
	}

	// Should not appear in cron workflows (no expression tracked)
	cronWFs := tm.GetCronWorkflows()
	if len(cronWFs) != 0 {
		t.Errorf("GetCronWorkflows() = %d, want 0 (no expression)", len(cronWFs))
	}
}

func TestTriggerManager_MatchEvent_GlobMatching(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("glob-wf", workflow.TriggerConfig{
		Type: "message",
		Config: map[string]interface{}{
			"channel": "chat-*",
		},
	})

	tests := []struct {
		name     string
		data     map[string]interface{}
		expected int
	}{
		{"glob match prefix", map[string]interface{}{"channel": "chat-general"}, 1},
		{"glob match different suffix", map[string]interface{}{"channel": "chat-random"}, 1},
		{"no glob match", map[string]interface{}{"channel": "voice-general"}, 0},
		{"glob match empty suffix", map[string]interface{}{"channel": "chat-"}, 1},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			matched := tm.MatchEvent("message", tt.data)
			if len(matched) != tt.expected {
				t.Errorf("MatchEvent() = %v, expected %d matches", matched, tt.expected)
			}
		})
	}
}

func TestTriggerManager_MultipleTriggersForWorkflow(t *testing.T) {
	tm := workflow.NewTriggerManager()

	tm.RegisterTrigger("multi-wf", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "*/5 * * * *"},
	})
	tm.RegisterTrigger("multi-wf", workflow.TriggerConfig{
		Type:   "cron",
		Config: map[string]interface{}{"expression": "0 * * * *"},
	})

	triggers := tm.ListTriggers("multi-wf")
	if len(triggers) != 2 {
		t.Errorf("ListTriggers() = %d, want 2", len(triggers))
	}

	cronWFs := tm.GetCronWorkflows()
	if len(cronWFs["multi-wf"]) != 2 {
		t.Errorf("cron expressions = %v, want 2", cronWFs["multi-wf"])
	}
}

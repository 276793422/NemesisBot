package models

import (
	"strings"
	"testing"
)

// W1.5 别名机制单元测试：TESTAI_ALIASES 解析 + Get 未命中回退。
// 硬编码测试模型行为零改动——直接命中永远优先于别名。

func newAliasTestRegistry(t *testing.T) *ModelRegistry {
	t.Helper()
	r := NewModelRegistry()
	r.Register(NewTestAI11())
	r.Register(NewTestAI20())
	return r
}

func TestParseAliasesValid(t *testing.T) {
	aliases, errs := ParseAliases("gpt-4o-mini=testai-1.1, gpt-4.1=testai-2.0")
	if len(errs) != 0 {
		t.Fatalf("expected no errors, got %v", errs)
	}
	if aliases["gpt-4o-mini"] != "testai-1.1" || aliases["gpt-4.1"] != "testai-2.0" {
		t.Fatalf("parsed aliases wrong: %v", aliases)
	}
}

func TestParseAliasesMalformedSegmentsSkip(t *testing.T) {
	aliases, errs := ParseAliases("no-eq-seg, =testai-1.1, empty=, ok=testai-2.0")
	if aliases["ok"] != "testai-2.0" {
		t.Fatalf("valid segment lost: %v", aliases)
	}
	if len(aliases) != 1 {
		t.Fatalf("malformed segments must be skipped, got %v", aliases)
	}
	if len(errs) != 3 {
		t.Fatalf("expected 3 errors for malformed segments, got %v", errs)
	}
}

func TestParseAliasesEmptySpec(t *testing.T) {
	aliases, errs := ParseAliases("  , , ")
	if len(aliases) != 0 || len(errs) != 0 {
		t.Fatalf("blank spec should yield nothing: %v %v", aliases, errs)
	}
}

func TestGetDirectHitWinsOverAlias(t *testing.T) {
	r := newAliasTestRegistry(t)
	// 别名试图劫持硬编码名 → 拒绝（硬编码优先，零行为改动）。
	if err := r.SetAlias("testai-1.1", "testai-2.0"); err == nil {
		t.Fatal("alias shadowing a registered model must be rejected")
	}
	m, ok := r.Get("testai-1.1")
	if !ok || m.Name() != "testai-1.1" {
		t.Fatalf("direct hit must return hardcoded model, got %v %v", ok, m)
	}
}

func TestGetMissFallsBackToAlias(t *testing.T) {
	r := newAliasTestRegistry(t)
	if errs := r.ApplyAliases(map[string]string{"gpt-4o-mini": "testai-1.1"}); len(errs) != 0 {
		t.Fatalf("apply failed: %v", errs)
	}
	m, ok := r.Get("gpt-4o-mini")
	if !ok || m.Name() != "testai-1.1" {
		t.Fatalf("alias fallback failed: ok=%v model=%v", ok, m)
	}
	// 原名仍可直取，List 不含别名（别名对 /v1/models 不可见）。
	if _, ok := r.Get("testai-1.1"); !ok {
		t.Fatal("canonical name must remain resolvable")
	}
	if len(r.List()) != 2 {
		t.Fatalf("aliases must not appear in List, got %d", len(r.List()))
	}
}

func TestAliasToUnknownTargetRejected(t *testing.T) {
	r := newAliasTestRegistry(t)
	if err := r.SetAlias("gpt-4o-mini", "testai-9.9"); err == nil {
		t.Fatal("alias to unregistered target must be rejected")
	}
	if _, ok := r.Get("gpt-4o-mini"); ok {
		t.Fatal("rejected alias must not resolve")
	}
}

func TestAliasChainNotSupported(t *testing.T) {
	// a→b 合法后，c→a（a 是别名不是模型）必须被拒：目标必须 testai-* 真名。
	r := newAliasTestRegistry(t)
	if errs := r.ApplyAliases(map[string]string{"a": "testai-1.1"}); len(errs) != 0 {
		t.Fatalf("setup failed: %v", errs)
	}
	if err := r.SetAlias("b", "a"); err == nil {
		t.Fatal("alias-to-alias must be rejected (one-hop resolution only)")
	}
}

func TestParseAliasesErrorMessageShape(t *testing.T) {
	_, errs := ParseAliases("bad-seg")
	if len(errs) != 1 || !strings.Contains(errs[0].Error(), "bad-seg") {
		t.Fatalf("error should name the offending segment: %v", errs)
	}
}

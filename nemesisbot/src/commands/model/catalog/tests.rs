//! Catalog module tests (U16 sixth batch) — fixture-based, no network.

use super::*;

const FIXTURE: &str = r#"{
  "openai": {
    "id": "openai",
    "name": "OpenAI",
    "env": ["OPENAI_API_KEY"],
    "npm": "@ai-sdk/openai",
    "doc": "https://platform.openai.com/docs/models",
    "models": {
      "gpt-5.2": {
        "name": "GPT-5.2",
        "description": "Reliable GPT generation",
        "family": "gpt",
        "release_date": "2025-12-11",
        "last_updated": "2025-12-11",
        "attachment": true,
        "reasoning": true,
        "tool_call": true,
        "open_weights": false,
        "limit": { "context": 400000, "output": 128000 }
      },
      "gpt-4o-mini-no-limit": {
        "name": "GPT-4o mini",
        "description": "legacy entry without limit block",
        "open_weights": false
      }
    }
  },
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "env": ["ANTHROPIC_API_KEY"],
    "npm": "@ai-sdk/anthropic",
    "doc": "https://docs.anthropic.com",
    "models": {
      "claude-opus-4-8": {
        "name": "Claude Opus 4.8",
        "family": "claude",
        "open_weights": false,
        "limit": { "context": 1000000 }
      }
    }
  },
  "weird-vendor-without-models": {
    "id": "weird",
    "name": "Weird"
  }
}"#;

#[test]
fn test_parse_api_json_extracts_limits() {
    let entries = parse_api_json(FIXTURE).expect("fixture parses");
    // Providers without models / models without limit blocks are skipped.
    assert_eq!(entries.len(), 2, "entries: {entries:?}");
    // Sorted by key.
    assert_eq!(entries[0].key, "anthropic/claude-opus-4-8");
    assert_eq!(entries[0].context_window, 1_000_000);
    assert_eq!(entries[0].max_output_tokens, None);
    assert_eq!(entries[0].family.as_deref(), Some("claude"));
    assert_eq!(entries[1].key, "openai/gpt-5.2");
    assert_eq!(entries[1].context_window, 400_000);
    assert_eq!(entries[1].max_output_tokens, Some(128_000));
}

#[test]
fn test_parse_api_json_rejects_garbage() {
    assert!(parse_api_json("not json").is_err());
    assert!(parse_api_json("[1,2,3]").is_err()); // top-level array
}

#[test]
fn test_parse_api_json_ignores_unknown_fields() {
    // Future API fields must not break parsing (goal §八 risk clause).
    let future = r#"{
      "x": {
        "models": {
          "m1": { "brand_new_field": {"nested": true}, "limit": {"context": 8, "output": 4} }
        }
      }
    }"#;
    let entries = parse_api_json(future).expect("future fields tolerated");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].context_window, 8);
}

#[test]
fn test_cache_roundtrip_and_lookup() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    assert!(load_cache(dir).unwrap().is_none(), "missing file → None");

    let entries = parse_api_json(FIXTURE).unwrap();
    save_cache(dir, entries).expect("save");
    // Atomic rename leaves no temp residue.
    assert!(!catalog_path(dir).with_extension("json.tmp").exists());

    let cat = load_cache(dir).unwrap().expect("loaded after save");
    assert_eq!(cat.version, 1);
    assert_eq!(cat.entries.len(), 2);
    let hit = lookup(&cat, "openai/gpt-5.2").expect("hit");
    assert_eq!(hit.context_window, 400_000);
    assert!(lookup(&cat, "nope/model").is_none());
}

#[test]
fn test_corrupt_cache_is_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = catalog_path(tmp.path());
    std::fs::write(&p, "{ broken").unwrap();
    assert!(load_cache(tmp.path()).is_err(), "present-but-corrupt → Err");
}

#[test]
fn test_parse_models_json_mirror_shape() {
    // Repo models.json (OpenRouter sync cache) — the jsDelivr mirror shape.
    let mirror = r#"{
      "data": [
        {"id": "anthropic/claude-opus-4.7-fast", "name": "Opus", "context_length": 1000000,
         "top_provider": {"max_completion_tokens": 64000}},
        {"id": "~anthropic/claude-haiku-latest", "context_length": 200000},
        {"id": "no-slash-id", "context_length": 8000},
        {"id": "openai/gpt-no-ctx", "name": "no context field"},
        {"id": "openai/gpt-zero", "context_length": 0}
      ]
    }"#;
    let entries = parse_models_json(mirror).expect("mirror shape parses");
    assert_eq!(entries.len(), 1, "alias/no-slash/no-ctx/zero skipped: {entries:?}");
    assert_eq!(entries[0].key, "anthropic/claude-opus-4.7-fast");
    assert_eq!(entries[0].context_window, 1_000_000);
    assert_eq!(entries[0].max_output_tokens, Some(64_000));
    // parse_any accepts both shapes.
    assert!(parse_any(FIXTURE).is_ok());
    assert!(parse_any(mirror).is_ok());
    assert!(parse_any("garbage").is_err());
}

#[test]
fn test_parse_any_zero_entry_shapes_are_misses() {
    // Live-observed regression (2026-08-23): the mirror body (models.json
    // shape) also parses as a top-level OBJECT, so lenient parse_api_json
    // returned Ok(vec![]) and shadowed the mirror parser → 0 models cached.
    // Now: wrong-shape objects are Err from parse_api_json, and parse_any
    // treats zero-entry successes as misses.
    let wrong_shape_object = r#"{"data": [{"id": "x/y", "context_length": 8}]}"#;
    assert!(parse_api_json(wrong_shape_object).is_err());
    // A genuinely empty api.json provider map is also rejected by parse_api_json
    // (no provider entries) — parse_any then tries the other parser.
    assert!(parse_api_json("{}").is_err());
    assert!(parse_any(wrong_shape_object).is_ok()); // via models_json parser
    assert!(parse_any("{}").is_err()); // neither shape yields entries
}

// by_family — 4b layer-1 gap fill: grouping semantics had no direct test
// (parse/cache tests above cover ingestion, not the family grouping).
#[test]
fn test_by_family_groups_and_none_bucket() {
    let catalog = Catalog {
        version: 1,
        fetched_at: "2026-08-24T00:00:00Z".to_string(),
        entries: vec![
            CatalogEntry {
                key: "openai/gpt-5.2".to_string(),
                context_window: 400000,
                max_output_tokens: Some(128000),
                family: Some("gpt".to_string()),
            },
            CatalogEntry {
                key: "openai/o4-mini".to_string(),
                context_window: 200000,
                max_output_tokens: Some(100000),
                family: Some("gpt".to_string()),
            },
            CatalogEntry {
                key: "anthropic/claude-opus-5".to_string(),
                context_window: 1000000,
                max_output_tokens: Some(64000),
                family: Some("claude".to_string()),
            },
            CatalogEntry {
                key: "weird/no-family".to_string(),
                context_window: 8000,
                max_output_tokens: None,
                family: None,
            },
        ],
    };
    let grouped = by_family(&catalog);
    assert_eq!(
        grouped.get("gpt").map(|v| v.as_slice()),
        Some(&["openai/gpt-5.2".to_string(), "openai/o4-mini".to_string()][..]),
        "same-family entries share a bucket, in catalog order"
    );
    assert_eq!(
        grouped.get("claude").map(|v| v.as_slice()),
        Some(&["anthropic/claude-opus-5".to_string()][..])
    );
    assert_eq!(
        grouped.get("(none)").map(|v| v.as_slice()),
        Some(&["weird/no-family".to_string()][..]),
        "missing family lands in the (none) bucket, not dropped"
    );
    assert_eq!(grouped.len(), 3, "exactly three buckets");
}

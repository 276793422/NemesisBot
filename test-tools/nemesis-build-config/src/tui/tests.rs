//! W5d batch tests: pure TUI logic — navigation tree construction (`rows`)
//! and per-row text rendering (`row_text`). The real terminal loop
//! (raw mode + alternate screen + crossterm event polling) is structurally
//! untestable in-process and exempt (see goal §9.4).

use super::*;

fn man(text: &str) -> FeatureManifest {
    FeatureManifest::parse(text).unwrap()
}

fn labels(rs: &[Row]) -> Vec<String> {
    rs.iter().map(|r| r.label.clone()).collect()
}

#[test]
fn w5d_rows_orders_known_categories_then_unknown() {
    // stable order: channels, subsystems, core, build, then anything else
    let m = man(
        r#"
[[feature]]
id = "f-build"
category = "build"
[[feature]]
id = "f-core"
category = "core"
[[feature]]
id = "f-weird"
category = "zzz-other"
[[feature]]
id = "f-channels"
category = "channels"
[[feature]]
id = "f-subsystems"
category = "subsystems"
"#,
    );
    let rs = rows(&m);
    let headers: Vec<String> = labels(&rs)
        .into_iter()
        .filter(|l| l.starts_with("—"))
        .collect();
    assert_eq!(
        headers,
        vec![
            "— channels —",
            "— subsystems —",
            "— core —",
            "— build —",
            "— zzz-other —"
        ]
    );
}

#[test]
fn w5d_rows_group_features_under_their_category_header() {
    let m = man(
        r#"
[[feature]]
id = "channels-web"
label = "Web 通道"
category = "channels"
[[feature]]
id = "migrate"
label = "迁移"
category = "subsystems"
"#,
    );
    let rs = rows(&m);
    let labs = labels(&rs);
    assert_eq!(
        labs,
        vec![
            "— channels —".to_string(),
            "Web 通道".to_string(),
            "— subsystems —".to_string(),
            "迁移".to_string()
        ]
    );
    assert_eq!(rs[1].id, "channels-web");
    assert!(!rs[1].is_enum);
    assert!(rs[0].id.is_empty(), "header rows carry an empty id");
}

#[test]
fn w5d_rows_drop_features_with_empty_category() {
    // pins current behavior: an authored feature without a category is not
    // rendered in the TUI (the menu is grouped exclusively by category)
    let m = man(
        r#"
[[feature]]
id = "orphan"
label = "无分类"
[[feature]]
id = "kept"
label = "kept"
category = "channels"
"#,
    );
    let rs = rows(&m);
    assert_eq!(labels(&rs), vec!["— channels —".to_string(), "kept".to_string()]);
}

#[test]
fn w5d_row_text_header_is_yellow_bold() {
    let m = man("");
    let row = Row {
        id: String::new(),
        label: "— channels —".to_string(),
        is_enum: false,
    };
    let line = row_text(&row, &m, &BuildConfig::default());
    assert_eq!(line.spans.len(), 1);
    let span = &line.spans[0];
    assert_eq!(span.content.to_string(), "— channels —");
    assert_eq!(span.style.fg, Some(Color::Yellow));
    assert!(span.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn w5d_row_text_bool_markers_on_off_and_unset() {
    let m = man(
        r#"
[[feature]]
id = "a"
label = "A"
category = "channels"
[[feature]]
id = "b"
label = "B"
category = "channels"
[[feature]]
id = "c"
label = "C"
category = "channels"
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    cfg.set_bool("b", false);
    // "c" left unset entirely
    for (id, expected) in [("a", "[x]"), ("b", "[ ]"), ("c", "[ ]")] {
        let row = Row {
            id: id.to_string(),
            label: id.to_string(),
            is_enum: false,
        };
        let line = row_text(&row, &m, &cfg);
        let text = line.spans[0].content.to_string();
        assert!(text.starts_with(expected), "{id}: {text}");
    }
}

#[test]
fn w5d_row_text_enum_value_right_aligned_width_8() {
    let m = man(
        r#"
[[feature]]
id = "build-profile"
label = "profile"
category = "build"
type = "enum"
options = ["release", "iotsmall"]
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_enum("build-profile", "release");
    let row = Row {
        id: "build-profile".to_string(),
        label: "profile".to_string(),
        is_enum: true,
    };
    let text = row_text(&row, &m, &cfg).spans[0].content.to_string();
    // {:>8}: "release" is 7 chars => exactly one leading space
    assert!(text.starts_with("[ release]"), "text was: {text}");
    // unset enum renders as an 8-space slot
    let unset = row_text(&row, &m, &BuildConfig::default()).spans[0].content.to_string();
    assert!(unset.starts_with("[        ]"), "unset was: {unset}");
}

#[test]
fn w5d_row_text_appends_desc_when_present() {
    let with_desc = man(
        r#"
[[feature]]
id = "a"
label = "Label"
desc = "描述文本"
category = "channels"
"#,
    );
    let without_desc = man(
        r#"
[[feature]]
id = "a"
label = "Label"
category = "channels"
"#,
    );
    let row = Row {
        id: "a".to_string(),
        label: "Label".to_string(),
        is_enum: false,
    };
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    let t1 = row_text(&row, &with_desc, &cfg).spans[0].content.to_string();
    assert_eq!(t1, "[x]  Label — 描述文本");
    let t2 = row_text(&row, &without_desc, &cfg).spans[0].content.to_string();
    assert_eq!(t2, "[x]  Label");
}

#[test]
fn w5d_row_text_survives_row_id_not_in_manifest() {
    // defensive path: a Row whose id no longer resolves in the manifest still
    // renders marker + bare label (no panic, no desc lookup)
    let m = man("");
    let row = Row {
        id: "ghost".to_string(),
        label: "Ghost".to_string(),
        is_enum: false,
    };
    let text = row_text(&row, &m, &BuildConfig::default()).spans[0].content.to_string();
    assert_eq!(text, "[ ]  Ghost");
}

#[test]
fn w5d_rows_empty_manifest_yields_no_rows() {
    let rs = rows(&man(""));
    assert!(rs.is_empty());
}

// ===========================================================================
// S12b batch: handle_key state machine (extracted verbatim from the
// interactive loop so navigation/toggle/save semantics are testable without
// a TTY). The draw/poll/raw-mode shell remains exempt (goal §9.4).
// ===========================================================================

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ctrl_c() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
}

const MANIFEST_TOML: &str = r#"
[[feature]]
id = "b-on"
label = "bool on"
category = "channels"
[[feature]]
id = "b-off"
label = "bool off"
category = "channels"
[[feature]]
id = "e"
label = "enum"
category = "build"
type = "enum"
options = ["release", "iotsmall"]
"#;

/// rows layout: [0]=header(channels), [1]=b-on, [2]=b-off,
/// [3]=header(build), [4]=enum
fn fixture() -> (FeatureManifest, BuildConfig, Vec<Row>) {
    let m = man(MANIFEST_TOML);
    let mut cfg = BuildConfig::default();
    cfg.set_bool("b-on", true);
    cfg.set_bool("b-off", false);
    cfg.set_enum("e", "release");
    let rs = rows(&m);
    assert_eq!(rs.len(), 5);
    (m, cfg, rs)
}

#[test]
fn s12b_move_down_clamps_at_last_row() {
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    let cp = std::path::Path::new("unused.config");
    // down from index 3 (build header)
    assert_eq!(
        handle_key(press(KeyCode::Down), &rs, Some(3), &m, &mut cfg, &mut dirty, cp),
        KeyDisposition::Select(Some(4))
    );
    // down at the last row stays clamped (j alias)
    assert_eq!(
        handle_key(press(KeyCode::Char('j')), &rs, Some(4), &m, &mut cfg, &mut dirty, cp),
        KeyDisposition::Select(Some(4))
    );
    assert!(!dirty, "movement never dirties the config");
}

#[test]
fn s12b_move_up_clamps_at_zero() {
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    let cp = std::path::Path::new("unused.config");
    assert_eq!(
        handle_key(press(KeyCode::Up), &rs, Some(0), &m, &mut cfg, &mut dirty, cp),
        KeyDisposition::Select(Some(0)),
        "up at top clamps to 0"
    );
    assert_eq!(
        handle_key(press(KeyCode::Char('k')), &rs, Some(1), &m, &mut cfg, &mut dirty, cp),
        KeyDisposition::Select(Some(0)),
        "k moves one row up"
    );
}

#[test]
fn s12b_space_toggles_bool_and_sets_dirty_header_rows_ignored() {
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    let cp = std::path::Path::new("unused.config");
    // toggle b-off on row 2: false -> true, dirty set
    handle_key(press(KeyCode::Char(' ')), &rs, Some(2), &m, &mut cfg, &mut dirty, cp);
    assert_eq!(cfg.get_bool("b-off"), Some(true));
    assert!(dirty);
    // header row 0 has an empty id → ignored entirely
    dirty = false;
    handle_key(press(KeyCode::Char(' ')), &rs, Some(0), &m, &mut cfg, &mut dirty, cp);
    assert!(!dirty);
}

#[test]
fn s12b_enter_cycles_enum_wrapping_boolean_rows_untouched() {
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    let cp = std::path::Path::new("unused.config");
    // enum sits at row 4; release -> iotsmall
    handle_key(press(KeyCode::Right), &rs, Some(4), &m, &mut cfg, &mut dirty, cp);
    assert_eq!(cfg.get_enum("e"), Some("iotsmall"));
    assert!(dirty);
    // wraparound: iotsmall -> release
    dirty = false;
    handle_key(press(KeyCode::Enter), &rs, Some(4), &m, &mut cfg, &mut dirty, cp);
    assert_eq!(cfg.get_enum("e"), Some("release"));
    // Right on a boolean row is a no-op (not enum)
    dirty = false;
    handle_key(press(KeyCode::Right), &rs, Some(1), &m, &mut cfg, &mut dirty, cp);
    assert!(!dirty);
}

#[test]
fn s12b_q_saves_config_and_exits_ctrl_c_exits_without_saving() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join(".config");
    let (m, mut cfg, rs) = fixture();
    let mut dirty = true;
    // flip a value first so the save has observable effect
    handle_key(press(KeyCode::Char(' ')), &rs, Some(2), &m, &mut cfg, &mut dirty, &cp);
    assert_eq!(
        handle_key(press(KeyCode::Char('q')), &rs, Some(2), &m, &mut cfg, &mut dirty, &cp),
        KeyDisposition::Exit
    );
    let saved = std::fs::read_to_string(&cp).unwrap();
    assert!(saved.contains("b-off = true"), "saved was: {saved}");

    // Ctrl+C exits with NO write: make in-memory state differ from disk
    cfg.set_bool("b-off", false);
    assert_eq!(
        handle_key(ctrl_c(), &rs, Some(2), &m, &mut cfg, &mut dirty, &cp),
        KeyDisposition::Exit
    );
    let after = std::fs::read_to_string(&cp).unwrap();
    assert!(after.contains("b-off = true"), "abort must not rewrite .config");
}

#[test]
fn s12b_esc_saves_like_q_and_s_saves_in_place_clearing_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join(".config");
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    assert_eq!(
        handle_key(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            &rs, Some(1), &m, &mut cfg, &mut dirty, &cp
        ),
        KeyDisposition::Exit
    );

    // s saves in place and clears the dirty marker but keeps editing
    let (m2, mut cfg2, rs2) = fixture();
    let mut dirty2 = false;
    handle_key(press(KeyCode::Char(' ')), &rs2, Some(2), &m2, &mut cfg2, &mut dirty2, &cp);
    assert!(dirty2);
    assert_eq!(
        handle_key(press(KeyCode::Char('s')), &rs2, Some(2), &m2, &mut cfg2, &mut dirty2, &cp),
        KeyDisposition::Nothing,
        "s keeps editing"
    );
    assert!(!dirty2, "s must clear the dirty marker");
    let saved = std::fs::read_to_string(&cp).unwrap();
    assert!(saved.contains("b-off = true"));
}

#[test]
fn s12b_non_key_events_and_non_press_kind_are_noops() {
    let (m, mut cfg, rs) = fixture();
    let mut dirty = false;
    let cp = std::path::Path::new("unused.config");
    // Resize / mouse events fall through
    assert_eq!(
        handle_key(Event::Resize(80, 24), &rs, Some(1), &m, &mut cfg, &mut dirty, cp),
        KeyDisposition::Nothing
    );
    // key released (Windows repeat report) must not toggle anything
    let release = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char(' '),
        KeyModifiers::empty(),
        KeyEventKind::Release,
    ));
    handle_key(release, &rs, Some(2), &m, &mut cfg, &mut dirty, cp);
    assert_eq!(cfg.get_bool("b-off"), Some(false), "Release kind must not toggle");
    assert!(!dirty);
    // unhandled keys (Left arrow, function keys, ...) are no-ops
    handle_key(press(KeyCode::Left), &rs, Some(1), &m, &mut cfg, &mut dirty, cp);
    handle_key(press(KeyCode::F(1)), &rs, Some(1), &m, &mut cfg, &mut dirty, cp);
    assert!(!dirty);
}

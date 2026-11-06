//! Pure-logic unit tests (R579: BUG-20261106-010/-011 audit round).
//! The FFI main loop cannot be driven offline (needs a live Sandboxie
//! driver), so what gets pinned here is the decision contract around it:
//! a failed enumeration must never arm the tail window, completion must
//! require a verified-empty window older than EMPTY_TAIL_MS, plus the
//! small JSONL helpers the evidence format rests on.

use super::*;

// --- BUG-20261106-010: tail-window / completion contract ---

#[test]
fn empty_box_arms_window_on_first_round() {
    assert_eq!(advance_empty_window(None, true, 1000), Some(1000));
}

#[test]
fn empty_box_keeps_anchor_on_later_rounds() {
    assert_eq!(advance_empty_window(Some(1000), true, 1500), Some(1000));
}

#[test]
fn live_processes_reset_window() {
    assert_eq!(advance_empty_window(Some(1000), false, 1500), None);
}

#[test]
fn failed_enumeration_never_arms_window() {
    // A failed round is "unknown", never "empty" — no anchor may come from
    // it, and an existing window must not survive an unknown round.
    assert_eq!(advance_empty_window(None, false, 1500), None);
    assert_eq!(advance_empty_window(Some(1000), false, 1500), None);
}

#[test]
fn completion_requires_window_older_than_tail() {
    assert!(!collect_done(Some(1000), 1000));
    assert!(!collect_done(Some(1000), 1000 + EMPTY_TAIL_MS));
    assert!(collect_done(Some(1000), 1000 + EMPTY_TAIL_MS + 1));
}

#[test]
fn completion_never_fires_without_window() {
    assert!(!collect_done(None, u64::MAX));
}

// --- JSONL helpers ---

#[test]
fn json_escape_quotes_backslash_and_controls() {
    assert_eq!(json_escape("a\"b\\c\nd\te"), "a\\\"b\\\\c\\nd\\te");
    assert_eq!(json_escape("\u{1}"), "\\u0001");
    assert_eq!(json_escape("plain"), "plain");
}

#[test]
fn type_label_known_and_unknown() {
    assert_eq!(type_label(0x01), "SYSCALL");
    assert_eq!(type_label(0x0A), "FILE");
    assert_eq!(type_label(0x11), "DNS");
    assert_eq!(type_label(0x99), "OTHER");
}

#[test]
fn from_wide_stops_at_null_and_tolerates_garbage() {
    let s: Vec<u16> = "abc".encode_utf16().chain([0, 0xD800]).collect();
    assert_eq!(from_wide(&s), "abc");
    assert_eq!(from_wide(&[]), "");
}

#[test]
fn wide_roundtrips_with_terminator() {
    let w = wide("NemesisEvalBox");
    assert_eq!(w.last(), Some(&0));
    assert_eq!(from_wide(&w), "NemesisEvalBox");
}

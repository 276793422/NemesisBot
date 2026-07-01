//! menuconfig-style build configurator for NemesisBot.
//!
//! This crate is a HOST-side developer tool. It is deliberately a separate
//! binary so it is never compiled into the (possibly trimmed) `nemesisbot`
//! binary itself.
//!
//! Model (mirrors Linux Kconfig):
//!   `features.toml` (manifest)  — single source of truth for toggleable features
//!   `.config`       (selection) — the developer's current on/off choices
//!   `nemesis-config export`     — turns a `.config` into cargo `--features` args
//!
//! See docs/PLAN/2026-07-01_feature-trimming-and-build-configurator.md

pub mod cargo_scan;
pub mod config;
pub mod export;
pub mod manifest;
pub mod tui;

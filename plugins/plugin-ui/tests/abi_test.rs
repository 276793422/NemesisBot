//! Basic C ABI and config parsing tests for plugin-ui DLL.
//!
//! These tests verify the C ABI exports and configuration parsing
//! without requiring a display server or WebView2 runtime.

// Note: cdylib crate types cannot have integration tests that link to the library.
// Instead, all tests are unit tests within the library itself.
// This file is kept as a reference but is not used as an integration test.
// The actual tests are in src/lib.rs under #[cfg(test)].

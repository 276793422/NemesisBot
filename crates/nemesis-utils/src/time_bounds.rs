//! Age/retention day-count upper bound (unified definition R735 numeric
//! panic surface).
//!
//! Two failure shapes share the same root: an unbounded day count (model
//! tool argument, config file value) reaching date arithmetic.
//!
//! 1. `chrono::Duration::days(d)` panics once `d` exceeds the representable
//!    millisecond range (~1.07e14 days) — a remote-triggerable panic when
//!    the value comes from tool args or a WSAPI-editable config.
//! 2. `u64/usize as i64` wraps values `> i64::MAX` to negative; `now -
//!    Duration::days(negative)` yields a cutoff in the FUTURE, so any
//!    cleanup consumer then mass-deletes its whole store (episodic memory,
//!    forge experiences, reflection reports, ...).
//!
//! The unified bound: every "days" input is clamped to [`MAX_AGE_DAYS`]
//! (100 years). No legitimate retention window exceeds that, so the clamp
//! is the identity for all sane values and only bounds absurd magnitudes.

/// Upper bound for age/retention day inputs (100 years).
pub const MAX_AGE_DAYS: i64 = 36_500;

#[cfg(test)]
mod tests;

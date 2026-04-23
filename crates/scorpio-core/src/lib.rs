#![allow(clippy::absurd_extreme_comparisons)]

//! # scorpio-core
//!
//! Shared runtime and domain logic for the `scorpio-analyst` multi-agent trading
//! system. This crate owns the reusable surface consumed by `scorpio-cli` today
//! and by future internal surfaces (TUI, backtest) tomorrow.
//!
//! The preferred consumer-facing entry points are:
//! - [`app`] — the `AnalysisRuntime` application facade.
//! - [`settings`] — the non-interactive user-config file boundary.
//!
//! Broader module visibility stays available where the cross-crate split still
//! requires it; new consumers should default to `app` and `settings` before
//! reaching deeper into the module tree.

pub mod agents;
pub mod analysis_packs;
pub mod app;
pub mod backtest;
pub mod config;
pub mod constants;
pub mod data;
pub mod error;
pub mod indicators;
pub mod observability;
pub mod providers;
pub mod rate_limit;
pub mod settings;
pub mod state;
pub mod workflow;

// Canonical re-export — the facade is the preferred entry point for new
// consumers, per the Unit 6 documentation guidance. Deeper module paths stay
// available for existing call sites that reach past the facade.
pub use app::AnalysisRuntime;

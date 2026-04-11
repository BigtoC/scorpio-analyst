#![allow(clippy::absurd_extreme_comparisons)]

pub mod analysis_packs;
pub mod config;
pub mod constants;
pub mod error;
pub mod observability;
pub mod rate_limit;
pub mod state;

// Skeleton modules — populated by downstream changes
pub mod agents;
pub mod backtest;
pub mod cli;
pub mod data;
pub mod indicators;
pub mod providers;
pub mod report;
pub mod workflow;

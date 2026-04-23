//! # scorpio-cli
//!
//! Binary crate for the `scorpio` CLI. This crate owns the clap/inquire command
//! surface, the interactive setup wizard, the update/self-upgrade path, the
//! terminal banner, and the terminal report formatting.
//!
//! The `pub mod` declarations below exist so the in-crate integration tests
//! (`tests/*.rs`) can reach CLI-specific helpers; they do not constitute a
//! stable public library surface for external consumers. Shared runtime and
//! domain logic live in the sibling `scorpio-core` crate.

pub mod cli;

//! Cross-pack prompt assets shared between equity and ETF packs.
//!
//! Each prompt is included via `include_str!("prompts/<name>.md")` by the
//! pack manifests; this module owns only the directory. Adding a file here
//! does not register it anywhere — the consuming pack chooses to pull it in.

use serde::{Deserialize, Serialize};

/// Built-in analysis pack identifier.
///
/// First-slice: only built-in packs selected by config/env string.
/// Serde support enables lightweight persistence in snapshot metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackId {
    /// Balanced institutional strategy — the default pack.
    Baseline,
}

impl PackId {
    /// Canonical string representation for config/env selection.
    pub fn as_str(self) -> &'static str {
        match self {
            PackId::Baseline => "baseline",
        }
    }
}

impl std::fmt::Display for PackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PackId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "baseline" => Ok(PackId::Baseline),
            unknown => Err(format!(
                "unknown analysis pack: \"{unknown}\" (available: baseline)"
            )),
        }
    }
}

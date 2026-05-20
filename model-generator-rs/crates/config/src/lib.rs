//! PDAG configuration types for the PRA model generator.
//!
//! A [`PdagConfig`] is the primary artifact produced by the `config`
//! sub-command of the CLI.  It is a plain serialisable struct that
//! completely describes how a PDAG (and the derived fault-tree / event-tree)
//! should be constructed.  The config can be written to — and read back
//! from — both TOML and JSON files, making it a reproducible, version-able
//! input artifact.
//!
//! # Example (TOML round-trip)
//! ```rust
//! use config::{PdagConfig, GateWeights};
//!
//! let cfg = PdagConfig::default();
//! let toml_str = cfg.to_toml().unwrap();
//! let parsed: PdagConfig = PdagConfig::from_toml(&toml_str).unwrap();
//! assert_eq!(cfg.seed, parsed.seed);
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur when creating or validating a [`PdagConfig`].
#[derive(Debug, Error, PartialEq)]
pub enum ConfigError {
    #[error("min_prob ({min}) must be ≤ max_prob ({max}) and both must be in [0, 1]")]
    InvalidProbabilityBounds { min: f64, max: f64 },

    #[error("layers must be ≥ 1, got {0}")]
    InvalidLayers(usize),

    #[error("nodes_per_layer_min ({min}) must be ≥ 1 and ≤ nodes_per_layer_max ({max})")]
    InvalidNodesPerLayer { min: usize, max: usize },

    #[error("children_per_node_min ({min}) must be ≥ 2 and ≤ children_per_node_max ({max})")]
    InvalidChildrenPerNode { min: usize, max: usize },

    #[error("gate weights must not all be zero")]
    AllZeroGateWeights,

    #[error("number_of_functional_events must be ≥ 1, got {0}")]
    InvalidFunctionalEvents(usize),

    #[error("serialisation error: {0}")]
    Serialise(String),

    #[error("deserialisation error: {0}")]
    Deserialise(String),
}

// ─── Gate-weight distribution ────────────────────────────────────────────────

/// Relative weights for the gate-type distribution.
///
/// Each field is a non-negative weight.  At least one must be > 0.
/// The weights are normalised internally when sampling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateWeights {
    /// Weight for AND gates.
    pub and: f64,
    /// Weight for OR gates.
    pub or: f64,
    /// Weight for K-of-N (atleast) gates.
    pub k_of_n: f64,
}

impl Default for GateWeights {
    fn default() -> Self {
        // Mirror the Python defaults: equal AND/OR weight, no K/N
        Self {
            and: 1.0,
            or: 1.0,
            k_of_n: 0.0,
        }
    }
}

impl GateWeights {
    /// Returns `true` if at least one weight is strictly positive.
    pub fn has_nonzero(&self) -> bool {
        self.and > 0.0 || self.or > 0.0 || self.k_of_n > 0.0
    }

    /// Returns the cumulative distribution function (CDF) values used for
    /// weighted random sampling.  The CDF has the same length as the number
    /// of gate types (3) and its last element is always 1.0.
    pub fn cdf(&self) -> [f64; 3] {
        let total = self.and + self.or + self.k_of_n;
        debug_assert!(total > 0.0, "gate weights must not all be zero");
        let and_p = self.and / total;
        let or_p = self.or / total;
        [and_p, and_p + or_p, 1.0]
    }
}

// ─── Main config struct ───────────────────────────────────────────────────────

/// Complete configuration for PDAG and model generation.
///
/// Produce via the CLI `config` sub-command, edit as needed, then pass to the
/// `generate` sub-command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdagConfig {
    // ── Identity ────────────────────────────────────────────────────────────
    /// Name of the generated fault-tree model.
    pub model_name: String,

    // ── Reproducibility ─────────────────────────────────────────────────────
    /// Seed for the random number generator.  Identical seeds produce
    /// identical models.
    pub seed: u64,

    // ── PDAG topology ───────────────────────────────────────────────────────
    /// Number of layers in the PDAG (≥ 1).
    pub layers: usize,

    /// Minimum number of nodes per layer (≥ 1).
    pub nodes_per_layer_min: usize,

    /// Maximum number of nodes per layer (≥ nodes_per_layer_min).
    pub nodes_per_layer_max: usize,

    /// Minimum number of children per non-leaf node (≥ 2).
    pub children_per_node_min: usize,

    /// Maximum number of children per non-leaf node (≥ children_per_node_min).
    pub children_per_node_max: usize,

    // ── Gate distribution ────────────────────────────────────────────────────
    /// Relative weights used to sample the gate type for each node.
    pub gate_weights: GateWeights,

    // ── Basic-event probabilities ────────────────────────────────────────────
    /// Minimum probability for basic events (inclusive, in [0, 1]).
    pub min_prob: f64,

    /// Maximum probability for basic events (inclusive, in [0, 1]).
    pub max_prob: f64,

    // ── Common-cause structure ───────────────────────────────────────────────
    /// Fraction of basic events that are shared between gates (0, 1].
    pub common_basic_event_fraction: f64,

    /// Average number of parents for shared basic events (≥ 2).
    pub common_basic_event_parents: usize,

    // ── Event-tree linkage ───────────────────────────────────────────────────
    /// Number of functional events in the event tree (≥ 1).
    pub number_of_functional_events: usize,
}

impl Default for PdagConfig {
    fn default() -> Self {
        Self {
            model_name: String::from("generated_model"),
            seed: 123,
            layers: 5,
            nodes_per_layer_min: 3,
            nodes_per_layer_max: 8,
            children_per_node_min: 2,
            children_per_node_max: 4,
            gate_weights: GateWeights::default(),
            min_prob: 0.01,
            max_prob: 0.1,
            common_basic_event_fraction: 0.3,
            common_basic_event_parents: 2,
            number_of_functional_events: 4,
        }
    }
}

impl PdagConfig {
    /// Validates that all fields contain consistent, in-range values.
    ///
    /// Call this after deserialising from a user-supplied file.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Probability bounds
        if self.min_prob < 0.0 || self.max_prob > 1.0 || self.min_prob > self.max_prob {
            return Err(ConfigError::InvalidProbabilityBounds {
                min: self.min_prob,
                max: self.max_prob,
            });
        }

        // Layers
        if self.layers < 1 {
            return Err(ConfigError::InvalidLayers(self.layers));
        }

        // Nodes per layer
        if self.nodes_per_layer_min < 1 || self.nodes_per_layer_min > self.nodes_per_layer_max {
            return Err(ConfigError::InvalidNodesPerLayer {
                min: self.nodes_per_layer_min,
                max: self.nodes_per_layer_max,
            });
        }

        // Children per node (must be ≥ 2 so every gate has at least 2 inputs)
        if self.children_per_node_min < 2 || self.children_per_node_min > self.children_per_node_max
        {
            return Err(ConfigError::InvalidChildrenPerNode {
                min: self.children_per_node_min,
                max: self.children_per_node_max,
            });
        }

        // Gate weights
        if !self.gate_weights.has_nonzero() {
            return Err(ConfigError::AllZeroGateWeights);
        }

        // Functional events
        if self.number_of_functional_events < 1 {
            return Err(ConfigError::InvalidFunctionalEvents(
                self.number_of_functional_events,
            ));
        }

        Ok(())
    }

    // ── Serialisation helpers ────────────────────────────────────────────────

    /// Serialises the config to a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Serialise(e.to_string()))
    }

    /// Deserialises a config from a TOML string and validates it.
    pub fn from_toml(input: &str) -> Result<Self, ConfigError> {
        let cfg: Self =
            toml::from_str(input).map_err(|e| ConfigError::Deserialise(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Serialises the config to a pretty-printed JSON string.
    pub fn to_json(&self) -> Result<String, ConfigError> {
        serde_json::to_string_pretty(self).map_err(|e| ConfigError::Serialise(e.to_string()))
    }

    /// Deserialises a config from a JSON string and validates it.
    pub fn from_json(input: &str) -> Result<Self, ConfigError> {
        let cfg: Self =
            serde_json::from_str(input).map_err(|e| ConfigError::Deserialise(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> PdagConfig {
        PdagConfig::default()
    }

    // ── Validation: happy path ───────────────────────────────────────────────

    #[test]
    fn default_config_is_valid() {
        assert!(valid_config().validate().is_ok());
    }

    // ── Validation: probability bounds ──────────────────────────────────────

    #[test]
    fn invalid_min_gt_max_prob() {
        let mut cfg = valid_config();
        cfg.min_prob = 0.9;
        cfg.max_prob = 0.1;
        assert_eq!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidProbabilityBounds { min: 0.9, max: 0.1 }
        );
    }

    #[test]
    fn invalid_prob_out_of_range() {
        let mut cfg = valid_config();
        cfg.min_prob = -0.01;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidProbabilityBounds { .. }
        ));

        cfg.min_prob = 0.0;
        cfg.max_prob = 1.1;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidProbabilityBounds { .. }
        ));
    }

    // ── Validation: layers ───────────────────────────────────────────────────

    #[test]
    fn zero_layers_invalid() {
        let mut cfg = valid_config();
        cfg.layers = 0;
        assert_eq!(cfg.validate().unwrap_err(), ConfigError::InvalidLayers(0));
    }

    // ── Validation: nodes per layer ──────────────────────────────────────────

    #[test]
    fn zero_min_nodes_invalid() {
        let mut cfg = valid_config();
        cfg.nodes_per_layer_min = 0;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidNodesPerLayer { .. }
        ));
    }

    #[test]
    fn min_nodes_gt_max_nodes_invalid() {
        let mut cfg = valid_config();
        cfg.nodes_per_layer_min = 10;
        cfg.nodes_per_layer_max = 5;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidNodesPerLayer { .. }
        ));
    }

    // ── Validation: children per node ────────────────────────────────────────

    #[test]
    fn children_min_lt_two_invalid() {
        let mut cfg = valid_config();
        cfg.children_per_node_min = 1;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidChildrenPerNode { .. }
        ));
    }

    #[test]
    fn children_min_gt_max_invalid() {
        let mut cfg = valid_config();
        cfg.children_per_node_min = 5;
        cfg.children_per_node_max = 3;
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::InvalidChildrenPerNode { .. }
        ));
    }

    // ── Validation: gate weights ─────────────────────────────────────────────

    #[test]
    fn all_zero_gate_weights_invalid() {
        let mut cfg = valid_config();
        cfg.gate_weights = GateWeights {
            and: 0.0,
            or: 0.0,
            k_of_n: 0.0,
        };
        assert_eq!(cfg.validate().unwrap_err(), ConfigError::AllZeroGateWeights);
    }

    // ── Serialisation: TOML round-trip ───────────────────────────────────────

    #[test]
    fn toml_round_trip() {
        let cfg = valid_config();
        let toml_str = cfg.to_toml().expect("serialise to TOML");
        let parsed = PdagConfig::from_toml(&toml_str).expect("deserialise from TOML");
        assert_eq!(cfg, parsed);
    }

    // ── Serialisation: JSON round-trip ───────────────────────────────────────

    #[test]
    fn json_round_trip() {
        let cfg = valid_config();
        let json_str = cfg.to_json().expect("serialise to JSON");
        let parsed = PdagConfig::from_json(&json_str).expect("deserialise from JSON");
        assert_eq!(cfg, parsed);
    }

    // ── Serialisation: invalid TOML rejected ─────────────────────────────────

    #[test]
    fn invalid_toml_rejected() {
        let result = PdagConfig::from_toml("this is not valid toml [[[");
        assert!(matches!(result, Err(ConfigError::Deserialise(_))));
    }

    // ── GateWeights: CDF ─────────────────────────────────────────────────────

    #[test]
    fn gate_weights_cdf_sums_to_one() {
        let w = GateWeights {
            and: 1.0,
            or: 1.0,
            k_of_n: 1.0,
        };
        let cdf = w.cdf();
        assert!((cdf[2] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn gate_weights_or_only_cdf() {
        let w = GateWeights {
            and: 0.0,
            or: 1.0,
            k_of_n: 0.0,
        };
        let cdf = w.cdf();
        assert_eq!(cdf[0], 0.0); // P(AND) = 0
        assert_eq!(cdf[1], 1.0); // P(AND) + P(OR) = 1
    }
}

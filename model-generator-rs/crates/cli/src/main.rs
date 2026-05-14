//! CLI entry point for the PRA model generator.
//!
//! # Sub-commands
//!
//! | Sub-command | Description |
//! |-------------|-------------|
//! | `config`    | Write a default (or customised) PDAG config to a file |
//! | `generate`  | Build a PDAG from a config file and print a summary |
//!
//! # Usage examples
//!
//! ```text
//! # Write a default config to pdag_config.toml
//! model-gen config --out pdag_config.toml
//!
//! # Override some fields inline
//! model-gen config --seed 42 --layers 6 --out my_config.json --format json
//!
//! # Build a PDAG from an existing config (summary to stdout)
//! model-gen generate --config pdag_config.toml
//! ```

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use config::{GateWeights, PdagConfig};
use pdag::PdagBuilder;

// ─── CLI structure ────────────────────────────────────────────────────────────

/// PRA Model Generator — Rust edition.
///
/// Generates synthetic Probabilistic Risk Assessment (PRA) models
/// (fault trees and event trees) in the Open-PSA MEF format.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Generate a PDAG configuration file with optional overrides.
    ///
    /// The config file is the primary input artifact for the `generate`
    /// sub-command.  Saving it allows reproducible model generation.
    Config(ConfigArgs),

    /// Build a PDAG from a config file and print a structural summary.
    ///
    /// In Phase 2 this sub-command will also write the Open-PSA MEF XML.
    Generate(GenerateArgs),
}

// ─── `config` sub-command ────────────────────────────────────────────────────

/// Arguments for the `config` sub-command.
#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    // ── Output ───────────────────────────────────────────────────────────────
    /// Path to write the config file (e.g. `pdag_config.toml`).
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub out: PathBuf,

    /// Output file format.
    #[arg(short, long, value_enum, default_value_t = ConfigFormat::Toml)]
    pub format: ConfigFormat,

    // ── Identity ─────────────────────────────────────────────────────────────
    /// Name of the generated fault-tree model.
    #[arg(long, default_value = "generated_model")]
    pub model_name: String,

    // ── Reproducibility ──────────────────────────────────────────────────────
    /// Seed for the random number generator.
    #[arg(long, default_value_t = 123)]
    pub seed: u64,

    // ── PDAG topology ────────────────────────────────────────────────────────
    /// Number of layers in the PDAG (root layer + intermediate layers + leaf layer).
    #[arg(long, default_value_t = 5)]
    pub layers: usize,

    /// Minimum number of nodes per layer.
    #[arg(long, default_value_t = 3)]
    pub nodes_min: usize,

    /// Maximum number of nodes per layer.
    #[arg(long, default_value_t = 8)]
    pub nodes_max: usize,

    /// Minimum number of children per gate node.
    #[arg(long, default_value_t = 2)]
    pub children_min: usize,

    /// Maximum number of children per gate node.
    #[arg(long, default_value_t = 4)]
    pub children_max: usize,

    // ── Gate distribution ────────────────────────────────────────────────────
    /// Relative weight for AND gates.
    #[arg(long, default_value_t = 1.0)]
    pub weight_and: f64,

    /// Relative weight for OR gates.
    #[arg(long, default_value_t = 1.0)]
    pub weight_or: f64,

    /// Relative weight for K-of-N gates.
    #[arg(long, default_value_t = 0.0)]
    pub weight_kon: f64,

    // ── Probabilities ────────────────────────────────────────────────────────
    /// Minimum failure probability for basic events.
    #[arg(long, default_value_t = 0.01)]
    pub min_prob: f64,

    /// Maximum failure probability for basic events.
    #[arg(long, default_value_t = 0.1)]
    pub max_prob: f64,

    // ── Common-cause ─────────────────────────────────────────────────────────
    /// Fraction of basic events shared across multiple parent gates.
    #[arg(long, default_value_t = 0.3)]
    pub common_fraction: f64,

    /// Average number of parents for shared basic events.
    #[arg(long, default_value_t = 2)]
    pub common_parents: usize,

    // ── Event tree ───────────────────────────────────────────────────────────
    /// Number of functional events in the event tree.
    #[arg(long, default_value_t = 4)]
    pub functional_events: usize,
}

/// Supported config file formats.
#[derive(Debug, Clone, ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Json,
}

// ─── `generate` sub-command ──────────────────────────────────────────────────

/// Arguments for the `generate` sub-command.
#[derive(Debug, clap::Args)]
pub struct GenerateArgs {
    /// Path to the PDAG config file (TOML or JSON).
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub config: PathBuf,
}

// ─── Sub-command handlers ─────────────────────────────────────────────────────

/// Handles the `config` sub-command.
pub fn run_config(args: &ConfigArgs) -> Result<()> {
    let cfg = PdagConfig {
        model_name: args.model_name.clone(),
        seed: args.seed,
        layers: args.layers,
        nodes_per_layer_min: args.nodes_min,
        nodes_per_layer_max: args.nodes_max,
        children_per_node_min: args.children_min,
        children_per_node_max: args.children_max,
        gate_weights: GateWeights {
            and: args.weight_and,
            or: args.weight_or,
            k_of_n: args.weight_kon,
        },
        min_prob: args.min_prob,
        max_prob: args.max_prob,
        common_basic_event_fraction: args.common_fraction,
        common_basic_event_parents: args.common_parents,
        number_of_functional_events: args.functional_events,
    };

    cfg.validate().context("invalid configuration values")?;

    let output = match args.format {
        ConfigFormat::Toml => cfg.to_toml().context("serialise to TOML")?,
        ConfigFormat::Json => cfg.to_json().context("serialise to JSON")?,
    };

    fs::write(&args.out, &output)
        .with_context(|| format!("write config to {:?}", args.out))?;

    println!("Config written to {:?}", args.out);
    Ok(())
}

/// Handles the `generate` sub-command.
pub fn run_generate(args: &GenerateArgs) -> Result<()> {
    let raw = fs::read_to_string(&args.config)
        .with_context(|| format!("read config from {:?}", args.config))?;

    // Detect format by extension; fall back to TOML.
    let cfg = if args
        .config
        .extension()
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        PdagConfig::from_json(&raw).context("parse JSON config")?
    } else {
        PdagConfig::from_toml(&raw).context("parse TOML config")?
    };

    println!("Building PDAG for model \"{}\"…", cfg.model_name);

    let pdag = PdagBuilder::new(cfg).context("create PDAG builder")?
        .build()
        .context("build PDAG")?;

    println!("  Nodes : {}", pdag.node_count());
    println!("  Edges : {}", pdag.edge_count());
    println!(
        "  Basic events : {}",
        pdag.basic_events().count()
    );
    println!(
        "  Gates        : {}",
        pdag.gates().count()
    );
    println!("PDAG built successfully.");
    Ok(())
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Config(args) => run_config(args),
        Commands::Generate(args) => run_generate(args),
    }
}

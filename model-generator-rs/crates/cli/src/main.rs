//! CLI entry point for the PRA model generator (`model-gen` binary).
//!
//! # Sub-commands
//!
//! | Command | Purpose |
//! |---------|---------|
//! | `config` | Write a [`PdagConfig`] file (TOML or JSON). |
//! | `generate` | Build a single model from a config file; optionally write Open-PSA MEF XML. |
//! | `batch` | Generate N models in one run, optionally with an event tree and a JSON manifest. |
//!
//! # Typical usage
//!
//! ```text
//! # 1. Write a config file with the "large" preset profile:
//! model-gen config --profile large --out large.toml
//!
//! # 2. Generate a fault tree + event tree and write MEF XML:
//! model-gen generate --config large.toml --format xml --out model.xml --event-tree
//!
//! # 3. Batch-generate 10 models and emit a manifest:
//! model-gen batch --config large.toml --count 10 --out-dir outputs --manifest
//! ```
//!
//! # Validation pipeline
//!
//! Every `generate` and `batch` run automatically runs three validators
//! (see the `validator` crate):
//! 1. [`StructuralValidator`] on the PDAG.
//! 2. [`ProbabilisticValidator`] on the FaultTree.
//! 3. [`ReferenceIntegrityValidator`] on the FaultTree ↔ EventTree pair
//!    (only when `--event-tree` is passed).
//!
//! Any validation failure causes a non-zero exit code.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use config::{GateWeights, PdagConfig};
use connector::TreeConnector;
use event_tree::EventTreeMapper;
use fault_tree::{FaultTreeMapper, XmlSerializer};
use pdag::PdagBuilder;
use validator::{ProbabilisticValidator, ReferenceIntegrityValidator, StructuralValidator};

// ─── Top-level CLI parser ────────────────────────────────────────────────────

/// The PRA model generator CLI.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Dispatch to one of the three top-level sub-commands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Write a PDAG configuration file.
    Config(ConfigArgs),
    /// Generate a single model and optionally write MEF XML.
    Generate(GenerateArgs),
    /// Generate N models in one batch run.
    Batch(BatchArgs),
}

// ─── `config` sub-command ────────────────────────────────────────────────────

/// Arguments for the `config` sub-command.
///
/// Either supply individual topology parameters (`--layers`, `--nodes-min`,
/// etc.) or choose a built-in `--profile` which sets sensible defaults for
/// a target model size.
#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    /// Path of the config file to write.
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub out: PathBuf,

    /// Output format: `toml` (default) or `json`.
    #[arg(short, long, value_enum, default_value_t = ConfigFormat::Toml)]
    pub format: ConfigFormat,

    /// Built-in topology preset.  Overrides all individual topology flags.
    #[arg(long)]
    pub profile: Option<GenerationProfile>,

    /// Name embedded in the generated model (default: `"generated_model"`).
    #[arg(long, default_value = "generated_model")]
    pub model_name: String,

    /// RNG seed for reproducibility.
    #[arg(long, default_value_t = 123)]
    pub seed: u64,

    /// Number of PDAG layers (≥ 1).
    #[arg(long, default_value_t = 5)]
    pub layers: usize,

    /// Minimum nodes per layer.
    #[arg(long, default_value_t = 3)]
    pub nodes_min: usize,

    /// Maximum nodes per layer.
    #[arg(long, default_value_t = 8)]
    pub nodes_max: usize,

    /// Minimum children per gate.
    #[arg(long, default_value_t = 2)]
    pub children_min: usize,

    /// Maximum children per gate.
    #[arg(long, default_value_t = 4)]
    pub children_max: usize,

    /// Relative weight for AND gates.
    #[arg(long, default_value_t = 1.0)]
    pub weight_and: f64,

    /// Relative weight for OR gates.
    #[arg(long, default_value_t = 1.0)]
    pub weight_or: f64,

    /// Relative weight for K-of-N gates.
    #[arg(long, default_value_t = 0.0)]
    pub weight_kon: f64,

    /// Minimum basic-event probability.
    #[arg(long, default_value_t = 0.01)]
    pub min_prob: f64,

    /// Maximum basic-event probability.
    #[arg(long, default_value_t = 0.1)]
    pub max_prob: f64,

    /// Fraction of basic events to promote to common-cause status.
    #[arg(long, default_value_t = 0.3)]
    pub common_fraction: f64,

    /// Average number of parents for common-cause basic events.
    #[arg(long, default_value_t = 2)]
    pub common_parents: usize,

    /// Number of functional events in the event tree.
    #[arg(long, default_value_t = 4)]
    pub functional_events: usize,
}

// ─── Enums used by multiple sub-commands ────────────────────────────────────

/// Serialization format for the config file.
#[derive(Debug, Clone, ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Json,
}

/// Output format for generated model files.
#[derive(Debug, Clone, ValueEnum)]
pub enum GenerateOutputFormat {
    /// Write an Open-PSA MEF XML file.
    Xml,
    /// Do not write a model file (print summary only).
    None,
}

/// Built-in topology preset that sets all topology parameters at once.
///
/// | Profile | Layers | Nodes/layer | Purpose |
/// |---------|--------|-------------|---------|
/// | `small` | 3 | 2–4 | Quick smoke tests |
/// | `medium` | 5 | 3–8 | Balanced default |
/// | `large` | 10 | 10–20 | Realistic workloads |
/// | `stress` | 12 | 20–30 | Load and performance tests |
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum GenerationProfile {
    Small,
    Medium,
    Large,
    Stress,
}

// ─── `generate` sub-command ──────────────────────────────────────────────────

/// Arguments for the `generate` sub-command.
#[derive(Debug, clap::Args)]
pub struct GenerateArgs {
    /// Path to the PDAG config file (TOML or JSON).
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub config: PathBuf,

    /// If specified, write the model to this file.
    /// Requires `--format xml`.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Output format when `--out` is given.
    #[arg(long, value_enum, default_value_t = GenerateOutputFormat::None)]
    pub format: GenerateOutputFormat,

    /// Also generate an event tree and include it in the MEF XML output.
    #[arg(long, default_value_t = false)]
    pub event_tree: bool,
}

// ─── `batch` sub-command ────────────────────────────────────────────────────

/// Arguments for the `batch` sub-command.
#[derive(Debug, clap::Args)]
pub struct BatchArgs {
    /// Path to the PDAG config file (TOML or JSON).
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub config: PathBuf,

    /// Number of models to generate (≥ 1).
    #[arg(long, default_value_t = 10)]
    pub count: usize,

    /// Directory to write model XML files into.
    /// Files are named `<model_name>_<i>.xml`.
    #[arg(long)]
    pub out_dir: PathBuf,

    /// Write a `manifest.json` listing each file's path, seed, and graph
    /// statistics.
    #[arg(long, default_value_t = false)]
    pub manifest: bool,

    /// Include event trees in each generated XML file.
    #[arg(long, default_value_t = false)]
    pub event_tree: bool,
}

// ─── Profile defaults ────────────────────────────────────────────────────────

/// Returns the [`PdagConfig`] for the given built-in profile.
///
/// The `medium` profile returns [`PdagConfig::default()`].  All other profiles
/// override specific fields and inherit the rest from the default.
fn profile_defaults(profile: GenerationProfile) -> PdagConfig {
    match profile {
        // Small: fast, lightweight models for unit/integration testing.
        GenerationProfile::Small => PdagConfig {
            layers: 3,
            nodes_per_layer_min: 2,
            nodes_per_layer_max: 4,
            children_per_node_min: 2,
            children_per_node_max: 3,
            min_prob: 0.001,
            max_prob: 0.01,
            common_basic_event_fraction: 0.2,
            common_basic_event_parents: 2,
            number_of_functional_events: 3,
            ..PdagConfig::default()
        },
        // Medium: balanced defaults — same as PdagConfig::default().
        GenerationProfile::Medium => PdagConfig::default(),
        // Large: 10 layers, 10–20 nodes per layer, mixed gate types.
        GenerationProfile::Large => PdagConfig {
            layers: 10,
            nodes_per_layer_min: 10,
            nodes_per_layer_max: 20,
            children_per_node_min: 2,
            children_per_node_max: 6,
            min_prob: 1e-5,
            max_prob: 5e-2,
            common_basic_event_fraction: 0.35,
            common_basic_event_parents: 3,
            number_of_functional_events: 8,
            gate_weights: GateWeights {
                and: 1.0,
                or: 1.0,
                k_of_n: 0.5,
            },
            ..PdagConfig::default()
        },
        // Stress: very large model intended for performance load tests.
        // The ignored `stress_profile_generation_under_30_seconds` test
        // asserts this completes within 30 s.
        GenerationProfile::Stress => PdagConfig {
            layers: 12,
            nodes_per_layer_min: 20,
            nodes_per_layer_max: 30,
            children_per_node_min: 2,
            children_per_node_max: 8,
            min_prob: 1e-6,
            max_prob: 1e-1,
            common_basic_event_fraction: 0.5,
            common_basic_event_parents: 4,
            number_of_functional_events: 10,
            gate_weights: GateWeights {
                and: 1.0,
                or: 1.0,
                k_of_n: 1.0,
            },
            ..PdagConfig::default()
        },
    }
}

// ─── Sub-command handlers ────────────────────────────────────────────────────

/// Handles the `config` sub-command.
///
/// Assembles a [`PdagConfig`] from `args` (or from a profile preset),
/// validates it, and writes it to `args.out` in the requested format.
pub fn run_config(args: &ConfigArgs) -> Result<()> {
    // Start from either a preset profile or from the individual CLI flags.
    let mut cfg = if let Some(profile) = args.profile {
        profile_defaults(profile)
    } else {
        PdagConfig {
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
            ..PdagConfig::default()
        }
    };

    // Identity fields are always taken from the command line (not the profile).
    cfg.model_name = args.model_name.clone();
    cfg.seed = args.seed;

    // Validate before writing to avoid persisting an unusable config.
    cfg.validate().context("invalid configuration values")?;

    let output = match args.format {
        ConfigFormat::Toml => cfg.to_toml().context("serialise to TOML")?,
        ConfigFormat::Json => cfg.to_json().context("serialise to JSON")?,
    };

    fs::write(&args.out, &output).with_context(|| format!("write config to {:?}", args.out))?;

    println!("Config written to {:?}", args.out);
    Ok(())
}

/// Loads a [`PdagConfig`] from `path`, auto-detecting TOML vs JSON by
/// file extension (`.json` → JSON; anything else → TOML).
fn load_config(path: &Path) -> Result<PdagConfig> {
    let raw = fs::read_to_string(path).with_context(|| format!("read config from {:?}", path))?;
    if path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        PdagConfig::from_json(&raw).context("parse JSON config")
    } else {
        PdagConfig::from_toml(&raw).context("parse TOML config")
    }
}

/// Handles the `generate` sub-command.
///
/// Pipeline:
/// 1. Load config → build PDAG → structural validation.
/// 2. Map PDAG → FaultTree → probabilistic validation.
/// 3. Optionally build EventTree → reference-integrity validation.
/// 4. If `--out` and `--format xml` are given, serialize to MEF XML.
/// 5. Print a structural summary to stdout.
pub fn run_generate(args: &GenerateArgs) -> Result<()> {
    let cfg = load_config(&args.config)?;

    println!("Building PDAG for model \"{}\"…", cfg.model_name);

    // Step 1: build the PDAG.
    let pdag = PdagBuilder::new(cfg.clone())
        .context("create PDAG builder")?
        .build()
        .context("build PDAG")?;

    // Step 1b: structural validation (acyclicity + fan-in).
    StructuralValidator::validate_pdag(&pdag).context("structural validation")?;

    // Step 2: map to typed FaultTree.
    let ft =
        FaultTreeMapper::from_pdag(&cfg.model_name, &pdag).context("map PDAG to fault tree")?;

    // Step 2b: probabilistic validation (probability bounds).
    ProbabilisticValidator::validate_fault_tree(&ft).context("probabilistic validation")?;

    // Step 3: optionally build an EventTree.
    let event_tree = if args.event_tree {
        let et = EventTreeMapper::build(
            &cfg.model_name,
            cfg.number_of_functional_events,
            &ft.name,
            &ft.top_gate,
        )
        .context("build event tree")?;
        // Step 3b: reference integrity validation (ET names resolve in FT).
        ReferenceIntegrityValidator::validate(&ft, &et)
            .context("reference integrity validation")?;
        Some(et)
    } else {
        None
    };

    // Step 4: write MEF XML if requested.
    if let (Some(out_path), GenerateOutputFormat::Xml) = (&args.out, &args.format) {
        let xml = XmlSerializer::serialize_model(&ft, event_tree.as_ref())
            .context("serialize Open-PSA MEF XML")?;
        fs::write(out_path, xml).with_context(|| format!("write XML output to {out_path:?}"))?;
        println!("Wrote Open-PSA MEF XML to {:?}", out_path);
    }

    // Step 5: print summary.
    println!("  Nodes : {}", pdag.node_count());
    println!("  Edges : {}", pdag.edge_count());
    println!("  Basic events : {}", pdag.basic_events().count());
    println!("  Gates        : {}", pdag.gates().count());
    println!("PDAG built and validated successfully.");
    Ok(())
}

/// Handles the `batch` sub-command.
///
/// Pipeline:
/// 1. Load the base config.
/// 2. For each model `i` (seed = base_seed + i):
///    a. Build PDAG → validate → map to FaultTree → validate.
/// 3. Promote shared basic events across all trees via [`TreeConnector`].
/// 4. For each model:
///    a. Optionally build EventTree → validate.
///    b. Serialize to `<out_dir>/<model_name>_<i+1>.xml`.
/// 5. Optionally write `<out_dir>/manifest.json`.
pub fn run_batch(args: &BatchArgs) -> Result<()> {
    let base_cfg = load_config(&args.config)?;
    if args.count == 0 {
        anyhow::bail!("--count must be >= 1");
    }

    // Create the output directory (including parents) if it does not exist.
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create output dir {:?}", args.out_dir))?;

    let mut trees = Vec::with_capacity(args.count);
    let mut pdags = Vec::with_capacity(args.count);
    let mut output_paths = Vec::with_capacity(args.count);
    let mut seeds = Vec::with_capacity(args.count);

    // Phase 1: build all models.  Each model gets an incremented seed so they
    // are all structurally different while remaining reproducible.
    for i in 0..args.count {
        let mut cfg = base_cfg.clone();
        cfg.seed = base_cfg.seed + i as u64;

        let pdag = PdagBuilder::new(cfg.clone())?.build()?;
        StructuralValidator::validate_pdag(&pdag).context("structural validation")?;

        // Derive a unique fault-tree name per model (e.g. "model-1", "model-2").
        let ft_name = format!("{}-{}", cfg.model_name, i + 1);
        let ft = FaultTreeMapper::from_pdag(&ft_name, &pdag)?;
        ProbabilisticValidator::validate_fault_tree(&ft).context("probabilistic validation")?;

        pdags.push(pdag);
        trees.push(ft);
        seeds.push(cfg.seed);
    }

    // Phase 2: promote a fraction of basic events to shared (common-cause)
    // events that appear in every tree.  This mutates the trees in-place.
    let _shared_events = TreeConnector::promote_shared_events(
        &mut trees,
        base_cfg.common_basic_event_fraction,
        base_cfg.seed,
    );

    // Phase 3: serialize each tree to an XML file.
    for (i, tree) in trees.iter().enumerate() {
        let out_file = args
            .out_dir
            .join(format!("{}_{}.xml", base_cfg.model_name, i + 1));

        // Optionally build and validate an event tree for this model.
        let et = if args.event_tree {
            let et = EventTreeMapper::build(
                &base_cfg.model_name,
                base_cfg.number_of_functional_events,
                &tree.name,
                &tree.top_gate,
            )?;
            ReferenceIntegrityValidator::validate(tree, &et)
                .context("reference integrity validation")?;
            Some(et)
        } else {
            None
        };

        let xml = XmlSerializer::serialize_model(tree, et.as_ref())?;
        fs::write(&out_file, xml).with_context(|| format!("write batch output to {out_file:?}"))?;
        output_paths.push(out_file.to_string_lossy().to_string());
    }

    // Phase 4: optionally write the JSON manifest.
    if args.manifest {
        let node_counts = pdags.iter().map(|p| p.node_count()).collect::<Vec<_>>();
        let edge_counts = pdags.iter().map(|p| p.edge_count()).collect::<Vec<_>>();

        // Build one JSON object per model.
        let entries = output_paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                json!({
                    "path": path,
                    "seed": seeds[i],
                    "node_count": node_counts[i],
                    "edge_count": edge_counts[i]
                })
            })
            .collect::<Vec<_>>();

        let manifest = json!({
            "model_name": base_cfg.model_name,
            "count": args.count,
            "entries": entries
        });
        let manifest_path = args.out_dir.join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).context("serialize manifest")?,
        )
        .with_context(|| format!("write manifest to {manifest_path:?}"))?;
        println!("Manifest written to {:?}", manifest_path);
    }

    println!("Batch generation completed: {} model(s)", args.count);
    Ok(())
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Config(args) => run_config(args),
        Commands::Generate(args) => run_generate(args),
        Commands::Batch(args) => run_batch(args),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;

    /// Returns a unique temporary directory path that is unlikely to collide
    /// across concurrent test runs (uses process ID as a discriminator).
    fn unique_tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "model-generator-rs-{}-{}",
            name,
            std::process::id()
        ))
    }

    // ── Batch + manifest ─────────────────────────────────────────────────────

    #[test]
    fn batch_manifest_has_expected_entry_count() {
        // Run a 3-model batch with the manifest flag and verify the manifest
        // contains exactly 3 entries.
        let dir = unique_tmp("batch-manifest");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let cfg_path = dir.join("cfg.toml");
        fs::write(&cfg_path, PdagConfig::default().to_toml().unwrap()).unwrap();

        run_batch(&BatchArgs {
            config: cfg_path,
            count: 3,
            out_dir: dir.clone(),
            manifest: true,
            event_tree: false,
        })
        .unwrap();

        let manifest = fs::read_to_string(dir.join("manifest.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(value["entries"].as_array().unwrap().len(), 3);
    }

    // ── Profile values ───────────────────────────────────────────────────────

    #[test]
    fn profile_large_values_are_sensible() {
        let cfg = profile_defaults(GenerationProfile::Large);
        assert_eq!(cfg.layers, 10);
        assert_eq!(cfg.nodes_per_layer_min, 10);
        assert_eq!(cfg.nodes_per_layer_max, 20);
    }

    // ── Load / performance test (ignored by default) ─────────────────────────

    #[test]
    #[ignore]
    fn stress_profile_generation_under_30_seconds() {
        // Run with `cargo test -- --ignored` to exercise the stress profile.
        // Fails if the complete generate pipeline (PDAG build + validation +
        // fault tree + event tree + XML serialization) takes > 30 s.
        let dir = unique_tmp("stress");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let cfg = profile_defaults(GenerationProfile::Stress);
        let cfg_path = dir.join("stress.toml");
        fs::write(&cfg_path, cfg.to_toml().unwrap()).unwrap();

        let start = Instant::now();
        run_generate(&GenerateArgs {
            config: cfg_path,
            out: Some(dir.join("stress.xml")),
            format: GenerateOutputFormat::Xml,
            event_tree: true,
        })
        .unwrap();
        assert!(start.elapsed().as_secs() < 30);
    }
}

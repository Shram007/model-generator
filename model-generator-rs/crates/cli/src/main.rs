//! CLI entry point for the PRA model generator.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use config::{GateWeights, PdagConfig};
use connector::TreeConnector;
use event_tree::EventTreeMapper;
use fault_tree::{FaultTreeMapper, XmlSerializer};
use pdag::PdagBuilder;
use validator::{ProbabilisticValidator, ReferenceIntegrityValidator, StructuralValidator};

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Config(ConfigArgs),
    Generate(GenerateArgs),
    Batch(BatchArgs),
}

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub out: PathBuf,

    #[arg(short, long, value_enum, default_value_t = ConfigFormat::Toml)]
    pub format: ConfigFormat,

    #[arg(long)]
    pub profile: Option<GenerationProfile>,

    #[arg(long, default_value = "generated_model")]
    pub model_name: String,

    #[arg(long, default_value_t = 123)]
    pub seed: u64,

    #[arg(long, default_value_t = 5)]
    pub layers: usize,

    #[arg(long, default_value_t = 3)]
    pub nodes_min: usize,

    #[arg(long, default_value_t = 8)]
    pub nodes_max: usize,

    #[arg(long, default_value_t = 2)]
    pub children_min: usize,

    #[arg(long, default_value_t = 4)]
    pub children_max: usize,

    #[arg(long, default_value_t = 1.0)]
    pub weight_and: f64,

    #[arg(long, default_value_t = 1.0)]
    pub weight_or: f64,

    #[arg(long, default_value_t = 0.0)]
    pub weight_kon: f64,

    #[arg(long, default_value_t = 0.01)]
    pub min_prob: f64,

    #[arg(long, default_value_t = 0.1)]
    pub max_prob: f64,

    #[arg(long, default_value_t = 0.3)]
    pub common_fraction: f64,

    #[arg(long, default_value_t = 2)]
    pub common_parents: usize,

    #[arg(long, default_value_t = 4)]
    pub functional_events: usize,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ConfigFormat {
    Toml,
    Json,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum GenerateOutputFormat {
    Xml,
    None,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum GenerationProfile {
    Small,
    Medium,
    Large,
    Stress,
}

#[derive(Debug, clap::Args)]
pub struct GenerateArgs {
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub config: PathBuf,

    #[arg(long)]
    pub out: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = GenerateOutputFormat::None)]
    pub format: GenerateOutputFormat,

    #[arg(long, default_value_t = false)]
    pub event_tree: bool,
}

#[derive(Debug, clap::Args)]
pub struct BatchArgs {
    #[arg(short, long, default_value = "pdag_config.toml")]
    pub config: PathBuf,

    #[arg(long, default_value_t = 10)]
    pub count: usize,

    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long, default_value_t = false)]
    pub manifest: bool,

    #[arg(long, default_value_t = false)]
    pub event_tree: bool,
}

fn profile_defaults(profile: GenerationProfile) -> PdagConfig {
    match profile {
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
        GenerationProfile::Medium => PdagConfig::default(),
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

pub fn run_config(args: &ConfigArgs) -> Result<()> {
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

    cfg.model_name = args.model_name.clone();
    cfg.seed = args.seed;

    cfg.validate().context("invalid configuration values")?;

    let output = match args.format {
        ConfigFormat::Toml => cfg.to_toml().context("serialise to TOML")?,
        ConfigFormat::Json => cfg.to_json().context("serialise to JSON")?,
    };

    fs::write(&args.out, &output).with_context(|| format!("write config to {:?}", args.out))?;

    println!("Config written to {:?}", args.out);
    Ok(())
}

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

pub fn run_generate(args: &GenerateArgs) -> Result<()> {
    let cfg = load_config(&args.config)?;

    println!("Building PDAG for model \"{}\"…", cfg.model_name);
    let pdag = PdagBuilder::new(cfg.clone())
        .context("create PDAG builder")?
        .build()
        .context("build PDAG")?;

    StructuralValidator::validate_pdag(&pdag).context("structural validation")?;

    let ft =
        FaultTreeMapper::from_pdag(&cfg.model_name, &pdag).context("map PDAG to fault tree")?;
    ProbabilisticValidator::validate_fault_tree(&ft).context("probabilistic validation")?;

    let event_tree = if args.event_tree {
        let et = EventTreeMapper::build(
            &cfg.model_name,
            cfg.number_of_functional_events,
            &ft.name,
            &ft.top_gate,
        )
        .context("build event tree")?;
        ReferenceIntegrityValidator::validate(&ft, &et)
            .context("reference integrity validation")?;
        Some(et)
    } else {
        None
    };

    if let (Some(out_path), GenerateOutputFormat::Xml) = (&args.out, &args.format) {
        let xml = XmlSerializer::serialize_model(&ft, event_tree.as_ref())
            .context("serialize Open-PSA MEF XML")?;
        fs::write(out_path, xml).with_context(|| format!("write XML output to {out_path:?}"))?;
        println!("Wrote Open-PSA MEF XML to {:?}", out_path);
    }

    println!("  Nodes : {}", pdag.node_count());
    println!("  Edges : {}", pdag.edge_count());
    println!("  Basic events : {}", pdag.basic_events().count());
    println!("  Gates        : {}", pdag.gates().count());
    println!("PDAG built and validated successfully.");
    Ok(())
}

pub fn run_batch(args: &BatchArgs) -> Result<()> {
    let base_cfg = load_config(&args.config)?;
    if args.count == 0 {
        anyhow::bail!("--count must be >= 1");
    }

    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create output dir {:?}", args.out_dir))?;

    let mut trees = Vec::with_capacity(args.count);
    let mut pdags = Vec::with_capacity(args.count);
    let mut output_paths = Vec::with_capacity(args.count);
    let mut seeds = Vec::with_capacity(args.count);

    for i in 0..args.count {
        let mut cfg = base_cfg.clone();
        cfg.seed = base_cfg.seed + i as u64;
        let pdag = PdagBuilder::new(cfg.clone())?.build()?;
        StructuralValidator::validate_pdag(&pdag).context("structural validation")?;

        let ft_name = format!("{}-{}", cfg.model_name, i + 1);
        let ft = FaultTreeMapper::from_pdag(&ft_name, &pdag)?;
        ProbabilisticValidator::validate_fault_tree(&ft).context("probabilistic validation")?;

        pdags.push(pdag);
        trees.push(ft);
        seeds.push(cfg.seed);
    }

    let _shared_events = TreeConnector::promote_shared_events(
        &mut trees,
        base_cfg.common_basic_event_fraction,
        base_cfg.seed,
    );

    for (i, tree) in trees.iter().enumerate() {
        let out_file = args
            .out_dir
            .join(format!("{}_{}.xml", base_cfg.model_name, i + 1));

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

    if args.manifest {
        let node_counts = pdags.iter().map(|p| p.node_count()).collect::<Vec<_>>();
        let edge_counts = pdags.iter().map(|p| p.edge_count()).collect::<Vec<_>>();
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Config(args) => run_config(args),
        Commands::Generate(args) => run_generate(args),
        Commands::Batch(args) => run_batch(args),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn unique_tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "model-generator-rs-{}-{}",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn batch_manifest_has_expected_entry_count() {
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

    #[test]
    fn profile_large_values_are_sensible() {
        let cfg = profile_defaults(GenerationProfile::Large);
        assert_eq!(cfg.layers, 10);
        assert_eq!(cfg.nodes_per_layer_min, 10);
        assert_eq!(cfg.nodes_per_layer_max, 20);
    }

    #[test]
    #[ignore]
    fn stress_profile_generation_under_30_seconds() {
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

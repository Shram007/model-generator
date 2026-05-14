# model-generator-rs

A Rust reimplementation of the PRA Model Generator — producing synthetic
[Open-PSA MEF](https://open-psa.github.io/mef/index.html) Fault Trees and
Event Trees via a **two-stage, reproducible pipeline**:

```
model-gen config [options]  →  pdag_config.toml   (Stage 1)
model-gen generate --config pdag_config.toml      (Stage 2)
```

---

## Architecture

The project is a Cargo workspace with three crates:

| Crate | Path | Purpose |
|-------|------|---------|
| `config` | `crates/config` | Typed, serialisable PDAG configuration structs (TOML + JSON) |
| `pdag` | `crates/pdag` | Layer-by-layer PDAG builder on `petgraph` with seeded RNG |
| `model-generator-rs` (binary) | `crates/cli` | `clap` CLI with `config` and `generate` sub-commands |

### Planned crates (future phases)

| Crate | Phase | Purpose |
|-------|-------|---------|
| `fault_tree` | 2 | PDAG → Fault Tree mapping + Open-PSA MEF XML output |
| `event_tree` | 3 | Event Tree with OR-gate FT↔ET linkage |
| `connector` | 4 | Cross-tree fault-event connectors + batch generation |
| `validator` | 5 | Acyclicity, probability range, and reference-integrity checks |

---

## Prerequisites

- [Rust toolchain](https://rustup.rs/) ≥ 1.75

---

## Build

```bash
cd model-generator-rs
cargo build --release
```

The binary is written to `target/release/model-gen`.

---

## Usage

### Stage 1 — Generate a config file

```bash
# Write default config
model-gen config --out pdag_config.toml

# Customise key parameters
model-gen config \
  --model-name my_model \
  --seed 42 \
  --layers 6 \
  --nodes-min 3 --nodes-max 10 \
  --children-min 2 --children-max 5 \
  --weight-and 1.0 --weight-or 1.0 --weight-kon 0.2 \
  --min-prob 1e-6 --max-prob 1e-2 \
  --functional-events 5 \
  --out my_config.toml

# Write as JSON instead of TOML
model-gen config --seed 99 --format json --out my_config.json
```

### Stage 2 — Build a PDAG from the config

```bash
model-gen generate --config pdag_config.toml
```

Output example:

```
Building PDAG for model "generated_model"…
  Nodes : 44
  Edges : 78
  Basic events : 24
  Gates        : 20
PDAG built successfully.
```

---

## Config file reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_name` | string | `"generated_model"` | Name of the model |
| `seed` | u64 | `123` | RNG seed (same seed → identical model) |
| `layers` | usize | `5` | Layers including root and leaf layers (≥ 1) |
| `nodes_per_layer_min` | usize | `3` | Min nodes per layer (≥ 1) |
| `nodes_per_layer_max` | usize | `8` | Max nodes per layer (≥ min) |
| `children_per_node_min` | usize | `2` | Min children per gate (≥ 2) |
| `children_per_node_max` | usize | `4` | Max children per gate (≥ min) |
| `gate_weights.and` | f64 | `1.0` | Relative weight for AND gates |
| `gate_weights.or` | f64 | `1.0` | Relative weight for OR gates |
| `gate_weights.k_of_n` | f64 | `0.0` | Relative weight for K-of-N gates |
| `min_prob` | f64 | `0.01` | Min basic-event failure probability |
| `max_prob` | f64 | `0.1` | Max basic-event failure probability |
| `common_basic_event_fraction` | f64 | `0.3` | Fraction of leaves shared across parents |
| `common_basic_event_parents` | usize | `2` | Average parents per shared basic event |
| `number_of_functional_events` | usize | `4` | Functional events in the event tree |

---

## Testing

```bash
cargo test
```

All 25 unit and doc tests must pass before any merge.

---

## Linting

```bash
cargo clippy -- -D warnings
```

---

## Development phases

| Phase | Status | Deliverable |
|-------|--------|-------------|
| 1 | ✅ Complete | Rust workspace, `config` crate, `pdag` builder, `cli` |
| 2 | ⏳ Planned | `fault_tree` crate + Open-PSA MEF XML output |
| 3 | ⏳ Planned | `event_tree` crate with OR-gate FT↔ET linkage |
| 4 | ⏳ Planned | `connector` crate + multi-tree batch generation |
| 5 | ⏳ Planned | `validator` crate + docs + load-test profiles |

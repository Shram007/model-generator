# Code Guide — `model-generator-rs`

A developer-focused reference for the Rust workspace.
Use this document to orient yourself before reading any source file.

---

## Table of contents

1. [Workspace layout](#1-workspace-layout)
2. [End-to-end data flow](#2-end-to-end-data-flow)
3. [Crate-by-crate reference](#3-crate-by-crate-reference)
   - [config](#31-config)
   - [pdag](#32-pdag)
   - [fault_tree](#33-fault_tree)
   - [event_tree](#34-event_tree)
   - [connector](#35-connector)
   - [validator](#36-validator)
   - [cli](#37-cli-binary-model-gen)
4. [Key data types at a glance](#4-key-data-types-at-a-glance)
5. [How to build, test, and lint](#5-how-to-build-test-and-lint)
6. [Adding a new feature — checklist](#6-adding-a-new-feature--checklist)

---

## 1. Workspace layout

```
model-generator-rs/
├── Cargo.toml          # workspace manifest; lists every member crate
├── CODE_GUIDE.md       # ← you are here
├── README.md           # user-facing introduction and phase status table
└── crates/
    ├── config/         # PdagConfig struct — shared by all other crates
    ├── pdag/           # probabilistic DAG builder
    ├── fault_tree/     # PDAG → FaultTree model + Open-PSA MEF XML serializer
    ├── event_tree/     # EventTree model and mapper
    ├── connector/      # cross-tree shared-event promotion + manifest helper
    ├── validator/      # structural, probabilistic, reference-integrity checks
    └── cli/            # model-gen binary (clap sub-commands)
```

Each sub-directory under `crates/` is an independent Rust crate with its own
`Cargo.toml` and a single source file `src/lib.rs` (or `src/main.rs` for the
binary).

---

## 2. End-to-end data flow

```
┌───────────────────────────────────────────────────────────────────┐
│  CLI: model-gen generate --config cfg.toml --format xml --out m.xml │
└───────────────────────────────────────────────────┬───────────────┘
                                                    │ load_config
                                                    ▼
                                              PdagConfig
                                    (validated struct from config crate)
                                                    │ PdagBuilder::new
                                                    │ PdagBuilder::build
                                                    ▼
                                               Pdag
                                    (petgraph DiGraph of NodeData)
                                                    │ StructuralValidator::validate_pdag
                                                    │ FaultTreeMapper::from_pdag
                                                    ▼
                                            FaultTree
                                    (gates + basic events in BTreeMaps)
                                                    │ ProbabilisticValidator::validate_fault_tree
                                                    │ (optional) EventTreeMapper::build
                                                    ▼
                                           EventTree  (optional)
                                                    │ ReferenceIntegrityValidator::validate
                                                    │ XmlSerializer::serialize_model
                                                    ▼
                                         Open-PSA MEF XML file
```

For the `batch` command the pipeline is run N times (with incremented seeds),
then `TreeConnector::promote_shared_events` is called across all trees before
serialization, and a `manifest.json` is optionally emitted.

---

## 3. Crate-by-crate reference

### 3.1 `config`

**File:** `crates/config/src/lib.rs`

Defines the configuration struct that fully describes how a PDAG and its
derived models are generated.  This is the only crate that every other crate
depends on directly or transitively.

| Symbol | Kind | Description |
|--------|------|-------------|
| `PdagConfig` | `struct` | Complete generation parameters (seed, topology, gate weights, probabilities, …). |
| `GateWeights` | `struct` | Relative weights for AND / OR / K-of-N gates. |
| `GateWeights::cdf` | `fn` | Converts weights to a 3-element CDF for weighted sampling. |
| `PdagConfig::validate` | `fn` | Checks all fields for consistency; call after deserializing. |
| `PdagConfig::to_toml` / `from_toml` | `fn` | TOML serialization round-trip. |
| `PdagConfig::to_json` / `from_json` | `fn` | JSON serialization round-trip. |
| `ConfigError` | `enum` | All validation and serialization errors. |

**Key design decisions:**
- `PdagConfig` is `Clone + PartialEq + Serialize + Deserialize` so it can be
  written to disk, read back, and compared in tests.
- `GateWeights::cdf()` returns a fixed-size array `[f64; 3]` which maps
  neatly to the three gate types without heap allocation.

---

### 3.2 `pdag`

**File:** `crates/pdag/src/lib.rs`

Builds a **Probabilistic DAG** (PDAG) — a directed acyclic graph whose nodes
carry logical operator and probability metadata.  This is the core
intermediate representation.

| Symbol | Kind | Description |
|--------|------|-------------|
| `Pdag` | `struct` | The built graph plus the root `NodeIndex`. |
| `Pdag::graph` | `field` | The underlying `petgraph::DiGraph<NodeData, ()>`. |
| `Pdag::root` | `field` | `NodeIndex` of the single root node (top gate). |
| `Pdag::basic_events` | `fn` | Iterator over all leaf (`BasicEvent`) node indices. |
| `Pdag::gates` | `fn` | Iterator over all gate (non-leaf) node indices. |
| `Pdag::children` | `fn` | Direct successors of a node. |
| `Pdag::parents` | `fn` | Direct predecessors of a node. |
| `PdagBuilder` | `struct` | Stateful builder seeded with `ChaCha8Rng`. |
| `PdagBuilder::new` | `fn` | Validates config and initialises the RNG. |
| `PdagBuilder::build` | `fn` | Executes the layer-by-layer construction; returns `Pdag`. |
| `NodeData` | `struct` | Per-node payload: `name`, `kind`, `probability`, layer indices. |
| `NodeKind` | `enum` | `Root`, `Gate(GateType)`, or `BasicEvent`. |
| `GateType` | `enum` | `And`, `Or`, or `KofN(usize)`. |

**Construction algorithm (inside `PdagBuilder::build`):**
1. Create layer 0 with a single `Root` node.
2. For each subsequent layer: sample `n_nodes` from
   `[nodes_per_layer_min, nodes_per_layer_max]`; on the last layer create
   `BasicEvent` nodes with random probabilities.
3. Call `connect_layers` to wire every parent to at least one child and up
   to `children_per_node_max` children each.
4. Call `introduce_common_events` to add extra edges from penultimate-layer
   parents to leaf nodes, modeling common-cause failures.

**Determinism note:** `BTreeSet` (not `HashSet`) is used whenever a set of
`NodeIndex` values is iterated to feed the RNG, so the same seed always
produces the same graph.

---

### 3.3 `fault_tree`

**File:** `crates/fault_tree/src/lib.rs`

Converts a `Pdag` into a typed `FaultTree` model and serializes it to
Open-PSA MEF v2.0 XML.

| Symbol | Kind | Description |
|--------|------|-------------|
| `FaultTree` | `struct` | Container: `name`, `top_gate`, `gates`, `basic_events`, `house_events`. |
| `Gate` | `struct` | A logical gate with a sorted `inputs` list. |
| `BasicEvent` | `struct` | A leaf failure event with its probability. |
| `HouseEvent` | `struct` | A fixed-state boundary condition. |
| `GateType` | `enum` | `And`, `Or`, `KofN(usize)`. |
| `FaultTree::all_references` | `fn` | Union of all gate/BE/HE names — used by the validator. |
| `FaultTreeMapper::from_pdag` | `fn` | Single-pass PDAG walk that populates `gates` and `basic_events`. |
| `XmlSerializer::serialize_fault_tree` | `fn` | Convenience wrapper: serialize FaultTree only. |
| `XmlSerializer::serialize_model` | `fn` | Serialize FaultTree + optional EventTree into one MEF XML string. |
| `FaultTreeError` | `enum` | `NoGates`, `MissingProbability`, `Xml`, `Utf8`. |

**XML structure emitted by `serialize_model`:**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<opsa-mef version="2.0">
  <define-fault-tree name="…">
    <top-gate name="root"/>
    <define-gate name="root" type="or">
      <input ref="G-1-0"/>
      …
    </define-gate>
    …
    <define-basic-event name="BE-4-0">
      <float value="0.012345678901"/>
    </define-basic-event>
    …
  </define-fault-tree>
  <!-- present only when an EventTree is provided: -->
  <define-event-tree name="…">
    …
  </define-event-tree>
</opsa-mef>
```

---

### 3.4 `event_tree`

**File:** `crates/event_tree/src/lib.rs`

Defines the event tree domain model and the `EventTreeMapper` that builds it
analytically from configuration values.

| Symbol | Kind | Description |
|--------|------|-------------|
| `EventTree` | `struct` | Root container: initiating event, functional events, branch set, sequences. |
| `InitiatingEvent` | `struct` | The accident trigger; holds a `top_gate_ref` back to the FaultTree root. |
| `FunctionalEvent` | `struct` | One safety system; holds a `fault_tree_ref`. |
| `BranchSet` | `struct` | Ordered header of functional events. |
| `Sequence` | `struct` | One accident path; `outcomes[i]` = success/failure of FE `i`. |
| `EventTreeMapper::build` | `fn` | Creates an `EventTree` with `2^N` sequences by bit-enumerating N-bit integers. |
| `EventTree::referenced_fault_trees` | `fn` | Set of distinct FT names used by any functional event. |

**Sequence enumeration:** for N functional events, index `i` encodes the
outcomes as an N-bit integer — bit `b` set means FE `b` succeeded.  The
sequence ID is the zero-padded binary representation of `i` (e.g. `"SEQ-011"`
for i=3, N=3).

---

### 3.5 `connector`

**File:** `crates/connector/src/lib.rs`

Promotes basic events to shared (common-cause) events that appear in multiple
fault trees.

| Symbol | Kind | Description |
|--------|------|-------------|
| `SharedEvent` | `struct` | Records the canonical shared name and which tree indices contain it. |
| `TreeConnector::promote_shared_events` | `fn` | Mutates trees in-place: renames a fraction of basic events to `SHARED-BE-{n}`. |
| `TreeConnector::rename_basic_event` | `fn` | Private helper: renames one BE and patches gate `inputs` lists. |
| `manifest_rows` | `fn` | Builds per-model metadata rows for `manifest.json`. |

**Promotion algorithm:**
1. `shared_count = round(min_tree_event_count × fraction)`.
2. For slot `i`: pick a random basic event (by sorted name for determinism)
   from each tree and rename it to `"SHARED-BE-{i+1}"`.
3. Gate `inputs` lists are re-sorted and deduplicated after renaming because
   the same canonical name may now appear twice if a gate already had both
   the old and new name as inputs.

---

### 3.6 `validator`

**File:** `crates/validator/src/lib.rs`

Three independent validator unit structs.  Each has a single public method.

| Struct | Method | Checks |
|--------|--------|--------|
| `StructuralValidator` | `validate_pdag(&Pdag)` | ① `is_cyclic_directed` = false; ② every gate has ≥ 2 children. |
| `ProbabilisticValidator` | `validate_fault_tree(&FaultTree)` | ① ≥ 1 basic event; ② all probabilities ∈ `[0, 1]`. |
| `ReferenceIntegrityValidator` | `validate(&FaultTree, &EventTree)` | ① FT top gate defined; ② ET initiating-event gate ref matches FT top gate; ③ every FE `fault_tree_ref` resolves. |

All methods return `Result<(), ValidationError>` where `ValidationError` is an
`enum` with three variants: `Structural`, `Probabilistic`, `Reference`.

---

### 3.7 `cli` (binary `model-gen`)

**File:** `crates/cli/src/main.rs`

The top-level binary.  Parsed by `clap` into three sub-commands.

#### Sub-command: `config`

| Function / type | Role |
|-----------------|------|
| `ConfigArgs` | CLI arguments (topology flags + `--profile` override). |
| `profile_defaults(GenerationProfile)` | Returns a `PdagConfig` matching a built-in preset. |
| `run_config(&ConfigArgs)` | Builds, validates, and writes the config file. |
| `GenerationProfile` | `Small` / `Medium` / `Large` / `Stress` presets. |

#### Sub-command: `generate`

| Function / type | Role |
|-----------------|------|
| `GenerateArgs` | `--config`, `--out`, `--format`, `--event-tree`. |
| `load_config(&Path)` | Reads a TOML or JSON config file. |
| `run_generate(&GenerateArgs)` | Full single-model pipeline (build → validate → serialize). |

#### Sub-command: `batch`

| Function / type | Role |
|-----------------|------|
| `BatchArgs` | `--config`, `--count`, `--out-dir`, `--manifest`, `--event-tree`. |
| `run_batch(&BatchArgs)` | Runs N pipelines, promotes shared events, serializes, and optionally writes manifest. |

#### Main entry point

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();          // clap parses argv
    match &cli.command {
        Commands::Config(args)   => run_config(args),
        Commands::Generate(args) => run_generate(args),
        Commands::Batch(args)    => run_batch(args),
    }
}
```

---

## 4. Key data types at a glance

```
PdagConfig                 (config crate)
  └─ GateWeights

Pdag                       (pdag crate)
  ├─ DiGraph<NodeData, ()>
  │    └─ NodeData
  │         ├─ NodeKind: Root | Gate(GateType) | BasicEvent
  │         └─ GateType: And | Or | KofN(usize)
  └─ NodeIndex (root)

FaultTree                  (fault_tree crate)
  ├─ BTreeMap<name, Gate>
  │    └─ Gate { name, GateType, inputs: Vec<name> }
  ├─ BTreeMap<name, BasicEvent>
  │    └─ BasicEvent { name, probability }
  └─ BTreeMap<name, HouseEvent>

EventTree                  (event_tree crate)
  ├─ InitiatingEvent { name, top_gate_ref }
  ├─ Vec<FunctionalEvent> { name, fault_tree_ref }
  ├─ BranchSet { name, functional_event_refs }
  └─ Vec<Sequence> { id, outcomes: Vec<bool> }

SharedEvent                (connector crate)
  └─ name, tree_indices: Vec<usize>

ValidationError            (validator crate)
  └─ Structural | Probabilistic | Reference
```

---

## 5. How to build, test, and lint

All commands must be run from `model-generator-rs/`:

```bash
# Build all crates
cargo build

# Run all unit and integration tests
cargo test

# Run the ignored load-test tier (stress profile)
cargo test -- --ignored

# Lint with all warnings as errors
cargo clippy -- -D warnings

# Format code
cargo fmt

# Run the CLI after building
cargo run -p model-generator-rs -- config --profile large --out large.toml
cargo run -p model-generator-rs -- generate --config large.toml --format xml --out model.xml --event-tree
cargo run -p model-generator-rs -- batch  --config large.toml --count 5 --out-dir out --manifest
```

---

## 6. Adding a new feature — checklist

1. **Config change?** Add field to `PdagConfig` in `config/src/lib.rs`,
   update `validate()`, default value, and serialization tests.
2. **PDAG change?** Modify `PdagBuilder` in `pdag/src/lib.rs`; use
   `BTreeSet` for any set that feeds the RNG to preserve determinism.
3. **New model element?** Add the type to the appropriate crate
   (`fault_tree`, `event_tree`, or `connector`).
4. **New XML output?** Extend `XmlSerializer` in `fault_tree/src/lib.rs`.
5. **New validation rule?** Add a check to the relevant validator in
   `validator/src/lib.rs`.
6. **New CLI flag?** Add to the relevant `*Args` struct in
   `cli/src/main.rs`; update the corresponding `run_*` function.
7. **Add tests** — unit tests go in `#[cfg(test)] mod tests` at the bottom
   of the relevant source file.
8. **Run** `cargo fmt && cargo clippy -- -D warnings && cargo test` before
   committing.

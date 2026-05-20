# User Guide

## Quick Start

```bash
cd model-generator-rs
cargo build --release
./target/release/model-gen config --out pdag_config.toml
./target/release/model-gen generate --config pdag_config.toml
```

## Generate Open-PSA MEF XML

```bash
model-gen generate --config pdag_config.toml --format xml --out model.xml
```

Include event trees:

```bash
model-gen generate --config pdag_config.toml --format xml --out model.xml --event-tree
```

## Batch generation

```bash
model-gen batch --config pdag_config.toml --count 10 --out-dir outputs --manifest
```

## Configuration profiles

Use preset profiles while creating configs:

```bash
model-gen config --profile small --out small.toml
model-gen config --profile medium --out medium.toml
model-gen config --profile large --out large.toml
model-gen config --profile stress --out stress.toml
```

Profiles tune topology defaults:
- `small`: quick checks and smoke tests
- `medium`: balanced default
- `large`: larger model sizes for realistic workloads
- `stress`: high-volume generation for load tests

## Validation

Validation runs automatically during `generate` and `batch`:
- Structural validation (acyclic graph, gate fan-in)
- Probabilistic validation (`[0,1]` bounds and sanity estimate)
- Reference integrity for event-tree links

Generation exits with a non-zero status on validation failures.

# OpenBSE — Project Instructions

## Before Every Commit

Run these three commands and fix any issues before committing:

```bash
cargo fmt --all            # auto-format Rust code
cargo clippy --workspace   # lint for common mistakes
cargo test --workspace     # run all tests
```

All three must pass cleanly. `cargo fmt` is enforced by CI — commits with formatting drift will fail.

If the change alters a model's equations or method, update that model's section of the
Engineering Reference (`docs/engineering-reference/`) in the same PR, including its
EnergyPlus comparison and known limitations.

## Editor Frontend (tools/editor/)

```bash
cd tools/editor
npm ci
npx tsc -b        # must compile without errors
npm run build      # full build (tsc + vite)
```

## Architecture

- Rust workspace with 11 crates in `crates/` — core, components, controls, envelope, weather, psychrometrics, io, airloop, cli, a205, cosim
- Tauri + React desktop editor in `tools/editor/` (excluded from Rust workspace)
- YAML input format — schema at `docs/openbse_schema.json`
- Engineering Reference (physics and equations) in `docs/engineering-reference/`, built with Quarto (website + PDF, `.github/workflows/docs.yml`)
- ASHRAE 140 test cases in `140_tests/cases/`
- DOE prototype validation models in `prototype_tests/`
- Example models in `examples/`

## Code Style

- Use `cargo fmt` — do not manually override its formatting
- Aim for zero clippy warnings on new code
- Keep functions focused; prefer small helper functions over deeply nested logic
- Comments should explain *why*, not *what*

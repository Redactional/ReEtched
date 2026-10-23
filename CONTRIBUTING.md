# Contributing to ReEtched

Thanks for helping! Start with [docs/PLAN.md](docs/PLAN.md) to see where the project is headed and which phase is in progress.

## Before you open a PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Windows, macOS and Linux.

- Keep changes focused; one feature or fix per PR.
- Logic with branches, parsing or math should come with a small test.
- Pin new dependencies to a minor version and say why the dependency is needed.

## Rules about Adobe code, files and assets

ReEtched aims to interoperate with Substance Painter files. To keep the project legally clean:

1. **Never copy Adobe code, shaders, documentation text, or bundled assets** (materials, smart materials, `sbs://` library graphs, alphas, environments) into this repository.
2. **Never commit Substance files you didn't create yourself.** Test files in `tests/corpus/` must be made by contributors, from scratch, and contributed under this project's license.
3. **Do not disassemble or decompile Adobe software.** Format work is done by observing files you saved yourself.
4. **Reverse-engineering work on undocumented formats** (`.spp`, `.sbsasm`) follows the clean-room process in [docs/PLAN.md](docs/PLAN.md#legal-ground-rules-not-legal-advice-get-real-counsel-before-phase-8): analysts write a format spec, implementers code only from that spec, and every finding records where it came from. Don't start on this without coordinating in an issue first.
5. Code from GPL projects (Blender, Krita) and from non-commercially licensed projects (DiffMat) must not be copied. Studying their design is fine. Code from permissively licensed projects (ArmorPaint, Material Maker, Bevy, Graphite) may be reused with attribution, respecting their licenses.

## License of contributions

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed under MIT or Apache-2.0, without any additional terms or conditions.

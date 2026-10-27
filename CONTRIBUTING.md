# Contributing to ReEtched

Thanks for helping make ReEtched better. This guide covers reporting issues, building, the project layout, the rules about Adobe files, and how releases are made.

## Reporting bugs and ideas

Use the [issue forms](https://github.com/Redactional/ReEtched/issues/new/choose). They ask for the version, your system and GPU, and the steps to reproduce. That's usually everything needed to fix a bug.

When something goes wrong, the debug log helps: turn on **Settings → Developer → Show the console window**, reproduce the problem and paste what it printed.

## Building

You need [Rust](https://rustup.rs) 1.96 or newer and a C compiler, since the FBX loader is C:

- **Windows:** Visual Studio Build Tools with "Desktop development with C++"
- **macOS:** `xcode-select --install`
- **Linux:** `gcc` or `clang`

```bash
cargo run -p reetched-app                      # debug build
cargo run --release -p reetched-app -- mesh.fbx   # optional: open a mesh or project
```

The workspace build also builds `reetched_python`, the scripting bridge library, next to the executable. Without it, or without Python, everything except scripting works.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Windows, macOS and Linux.

- Keep pull requests focused: one feature or fix each.
- Logic with branches, parsing or math comes with a small test.
- New dependencies: pin a minor version and say why it's needed.

## Project layout

```
crates/
  reetched-app/       the application: UI, GPU layer compositor, viewport, shaders
  reetched-doc/       the document: texture sets, layer stack, undo, .retx projects, texture export
  reetched-io/        mesh import (FBX, OBJ, glTF, USD) and export, topology, unwrapping
  reetched-bake/      mesh map baker
  reetched-graph/     procedural graph engine and material library
  reetched-substance/ Substance Designer .sbs reader and the Painter bridge importer
  reetched-spp/       read-only Painter project (.spp) conversion
  reetched-color/     OpenColorIO v2 subset with the ACES configs
  reetched-shader/    Painter custom shader (.glsl) translation
  reetched-trace/     GPU path tracer
  reetched-library/   asset library: scanning, tags, favorites, thumbnails
  reetched-script/    Python scripting host
  reetched-python/    the Python side (pyo3, loaded at runtime)
tools/painter-bridge/ Painter plugin that exports projects for ReEtched
docs/                 design notes, the .spp format notes and the migration guide
```

## Rules about Adobe code, files and assets

ReEtched reads Substance Painter files. To keep the project legally clean:

1. **Never copy Adobe code, shaders, documentation text or bundled assets** (materials, smart materials, `sbs://` library graphs, alphas, environments) into this repository.
2. **Never commit Substance files you didn't create yourself.** Test files must be made from scratch by contributors and contributed under this project's license. Tests that need real projects read them from a folder set in an environment variable and skip without it.
3. **Do not disassemble or decompile Adobe software.** Format work is done by observing files you saved yourself.
4. Work on undocumented formats follows the clean-room process in [docs/PLAN.md](docs/PLAN.md). Open an issue to coordinate before starting.
5. Don't copy code from GPL projects (Blender, Krita) or non-commercially licensed ones; studying their design is fine. Permissively licensed code (ArmorPaint, Material Maker, Bevy, Graphite) may be reused with attribution.

## Releases

Versions follow [semantic versioning](https://semver.org).

1. Set the new version in `[workspace.package]` in `Cargo.toml` and commit it.
2. Tag the commit and push the tag:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
3. The [Release workflow](.github/workflows/release.yml) builds Windows, macOS and Linux, then publishes the release:
   - installers
   - the portable `-<platform>.zip` archives [ReLauncher](https://github.com/Redactional/ReLauncher) installs
   - notes generated from the merged pull requests
4. A tag with a suffix (`v0.2.0-beta.1`) is published as a pre-release, which ReLauncher doesn't offer.

The workflow refuses to run if the tag and the version in `Cargo.toml` don't match. To rebuild an existing tag, run it from the Actions tab.

## License of contributions

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed under MIT or Apache-2.0, without any additional terms or conditions.

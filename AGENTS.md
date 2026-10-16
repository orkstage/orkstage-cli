# AGENTS.md: orkstage-cli

Guide for AI coding agents and the developers working with them. Details: `docs/building.md`,
`docs/scan-engine.md`, `docs/reference.md`.

## What this repository is

A Rust workspace (edition 2024) with three crates:

| Crate | Role |
|---|---|
| `crates/ork-cli` | The `ork` binary: `cli.rs` (clap definitions), `commands/` (`auth`, `project`, `template`, `operate`), `api.rs` (HTTP client for the control API), `config.rs`, `credentials.rs`, `output.rs` |
| `crates/orkstage-scan` | Scan engine: `detect/` has one detector per source (Terraform and OpenTofu with `hcl_eval.rs`, CloudFormation, Kubernetes, Compose, Dockerfiles, package manifests); `merge.rs`, `sizing.rs` and `template.rs` turn findings into a `platform/v1` template with evidence |
| `crates/orkstage-scan-wasm` | WebAssembly bindings; built into `dist/scan-wasm/`, published as `@orkstage/scan` and used by the API's `/imports` |

The scan runs locally; only the resulting report ever leaves the machine. Keep it that way: no
command uploads source files.

## Commands

Native builds work where unsigned binaries are allowed (Rust 1.88+):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
sh scripts/build-wasm.sh          # regenerates dist/scan-wasm/ (committed)
```

Where local binaries are blocked, use the builder image (see `docs/building.md`; Git Bash needs
`MSYS_NO_PATHCONV=1` and `$(pwd -W)`):

```bash
docker build -t orkstage/cli-builder:1.99 -f docker/builder.Dockerfile docker
RUN="docker run --rm -v $PWD:/work -v orkstage-cargo-registry:/usr/local/cargo/registry -v orkstage-cli-target:/work/target -w /work orkstage/cli-builder:1.99"
$RUN cargo test --workspace
```

## Rules

- **`dist/scan-wasm/` must match the source.** CI rebuilds it and fails on any difference, so run
  `scripts/build-wasm.sh` after touching `orkstage-scan` or `orkstage-scan-wasm`. The build remaps
  paths so the output is reproducible; do not remove the `--remap-path-prefix` flags.
- **Snapshot tests** use `insta`; fixtures live in `crates/orkstage-scan/tests/fixtures/<kind>/<project>/`.
  After an intended change, rerun with `INSTA_UPDATE=always` and review every `.snap` diff.
- **Every detector finding carries evidence** (file and location). Unknown or partial inputs
  produce warnings, never panics; the engine must not fail on a repository it does not understand.
- **The template the scan writes must validate** against the `platform/v1` schema of the
  `orkstage` repository (`packages/templates/src/schema.ts`). When that schema gains a resource
  type or field, map it here too.
- **API calls** follow the control API contract (`orkstage/packages/contracts`); API keys are read
  from the credential store or `ORKSTAGE_API_KEY`, never printed.
- `clippy -D warnings` is enforced; no `unwrap()` on user input or network data.
- Prose in docs and help text: plain English, no em or en dashes, no emoji, sentence case.

## License headers

Every source file starts with an SPDX header, `AGPL-3.0-only`. Enterprise code does not live here:
it is in the private `orkstage-ee` repository (see `docs/OPEN_CORE.md` in orkstage), so everything
in this repository is AGPL. CI runs the check; to add missing headers:

```bash
bash scripts/spdx.sh --fix
```

## Git workflow

Branch, pull request, squash merge when CI (`lint`, `test`, `wasm`) is green. Pull requests run
Linux only; the OS matrix and release builds run on tags and manual dispatch. After merging, bump
the pin in `orkstage-dev`; if `dist/scan-wasm` changed, the API picks it up from the workspace.

# Building, testing and releasing

## Build on your machine

Requirements: Rust 1.88 or later (CI uses 1.99).

```sh
cargo install --path crates/ork-cli --locked
ork --version
```

## Build in Docker

Everything (build, test, lint, WebAssembly) also runs in containers, which helps on machines
that block unsigned local binaries. Build the toolchain image once:

```sh
docker build -t orkstage/cli-builder:1.99 -f docker/builder.Dockerfile docker
```

Then run the commands from the repository root (PowerShell: use `${PWD}`; Git Bash: use
`$(pwd -W)` and set `MSYS_NO_PATHCONV=1`):

```sh
RUN="docker run --rm -v $PWD:/work -v orkstage-cargo-registry:/usr/local/cargo/registry -v orkstage-cli-target:/work/target -w /work orkstage/cli-builder:1.99"
$RUN cargo fmt --all --check
$RUN cargo clippy --workspace --all-targets -- -D warnings
$RUN cargo test --workspace
$RUN cargo bench -p orkstage-scan --bench scan          # synthetic repository of about 2,000 files
$RUN cargo build --release -p orkstage-cli                    # target/release/ork (Linux)
$RUN sh scripts/build-wasm.sh                            # regenerates dist/scan-wasm/
```

Snapshot tests use `insta`. After an intended change, run the tests with
`-e INSTA_UPDATE=always` and review the `.snap` diffs. Fixtures live in
`crates/orkstage-scan/tests/fixtures/<kind>/<project>/`.

`dist/scan-wasm/` is committed on purpose, and CI fails when it is out of date, so run
`scripts/build-wasm.sh` after changing the engine.

## Try the CLI against a local platform

Run the Linux binary on the Docker Compose network of the platform:

```sh
docker run --rm --network orkstage_default \
  -e ORK_API_URL=http://api:4000 -e ORK_TOKEN=<your ork_live_ key> \
  -v orkstage-cli-target:/t:ro debian:bookworm-slim /t/release/ork whoami
```

Use `http://host.docker.internal:4000` when the API runs on the host.

## Cross compiling

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | Native runners (`ubuntu-latest`, `ubuntu-24.04-arm`). |
| `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | Static binaries; install `musl-tools`. TLS is rustls with *ring* and the keyring backend is the pure Rust zbus, so neither OpenSSL nor libdbus is needed. |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS runners. |
| `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | `windows-latest`. |

Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`.

## Code signing

Windows binaries must be signed with Authenticode (for example Azure Trusted Signing, or
`signtool` with an EV certificate), because machines with application control refuse unsigned
executables. macOS binaries must be signed with a Developer ID (`codesign --options runtime`) and
notarized (`xcrun notarytool submit --wait`). The CI workflow has both steps; they run on tags
when the signing secrets are configured:

| Secret | Used for |
|---|---|
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TRUSTED_SIGNING_ENDPOINT`, `AZURE_TRUSTED_SIGNING_ACCOUNT`, `AZURE_CERTIFICATE_PROFILE` | Windows, Azure Trusted Signing |
| `MACOS_CERTIFICATE` (base64 .p12), `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | macOS, signing and notarization |

Without them a release is still published, unsigned, and the release notes should say so.

## Releasing

1. Set `version` in the workspace `Cargo.toml` and merge it to `main`.
2. Tag that commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. A tag with a
   suffix (`v0.2.0-rc.1`) publishes a prerelease.
3. CI builds every target, signs when it can, and the `release` job checks that the tag matches
   `Cargo.toml`, writes `SHA256SUMS`, adds build provenance (public repositories), and creates the
   GitHub Release with the archives and both install scripts.
4. The `install-test` job then installs that release with `install.sh` on Linux and macOS and
   `install.ps1` on Windows, and checks `ork --version`.

Archives are named `ork-<version>-<target>.tar.gz` (`.zip` on Windows) and hold the binary, the
license and the README. Release builds report the commit and build date in `ork --version`.

## crates.io

A stable version tag (`v0.2.0`, not `v0.2.0-rc.1`) also runs `publish.yml`, which publishes
`orkstage-scan` and then `orkstage-cli` to crates.io, so that `cargo install orkstage-cli` installs
the `ork` binary. It needs the `CARGO_REGISTRY_TOKEN` repository secret: a crates.io API token
(crates.io > Account settings > API tokens) with the `publish-new` and `publish-update` scopes,
limited to these two crates once they exist. A version already on crates.io is skipped, so the job
can be rerun. Run it by hand with "dry-run" to package and verify without uploading.

## Install scripts

[`scripts/install.sh`](../scripts/install.sh) (Linux, macOS) and
[`scripts/install.ps1`](../scripts/install.ps1) (Windows) download the archive for the machine,
verify it against `SHA256SUMS`, and install it without administrator rights (`~/.local/bin`, or
`%LOCALAPPDATA%\Programs\ork`, added to the user PATH). `orkstage.io/install.sh` and
`orkstage.io/install.ps1` redirect to the scripts on `main`, which install the latest release (the
newest prerelease while there is no stable one). Settings:
`ORK_VERSION`, `ORK_INSTALL_DIR` and `ORK_DOWNLOAD_BASE` (a mirror with the archive and
`SHA256SUMS` side by side).


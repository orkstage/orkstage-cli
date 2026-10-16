# orkstage-scan

The scan engine behind the [`ork` CLI](https://crates.io/crates/orkstage-cli) and the OrkStage
platform. It reads infrastructure and project files (Terraform and OpenTofu, CloudFormation, SAM and
CDK output, Kubernetes manifests and Helm charts, Docker Compose, Dockerfiles, package manifests)
and returns a `ScanReport`: the resources it found, with evidence, the connections between them,
and a `platform/v1` template that OrkStage can deploy.

```rust
use orkstage_scan::{scan_files, InputFile, ScanOptions};

let files = vec![InputFile::new("docker-compose.yml", "services:\n  db:\n    image: postgres:17\n")];
let report = scan_files(&files, &ScanOptions::default());
println!("{}", report.template);
```

Nothing leaves your machine: the engine works on the file contents you pass in. It is also
published for JavaScript as WebAssembly (`@orkstage/scan`).

Licensed under the GNU AGPL 3.0. Source, issues and documentation:
<https://github.com/orkstage/orkstage-cli> and <https://orkstage.io/docs/developer/import/>.

// SPDX-License-Identifier: AGPL-3.0-only
//! Command-line definition (clap derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `ork --version`: release builds set ORK_LONG_VERSION to "<version> (<commit> <date>)" at compile
/// time; local builds show the crate version only.
const LONG_VERSION: &str = match option_env!("ORK_LONG_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(
    name = "ork",
    version,
    long_version = LONG_VERSION,
    about = "OrkStage command line: scan a project into a template, push it, deploy and operate instances.",
    long_about = None,
    propagate_version = true,
    after_help = "Settings precedence: flags > environment (ORK_*) > .ork/config.toml > user config.\nExit codes: 0 ok, 1 failure, 2 usage, 3 authentication, 4 not found."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Args)]
pub struct Global {
    /// OrkStage API base URL, e.g. https://orkstage.example.com (without /api/v1).
    #[arg(long, global = true, env = "ORK_API_URL", value_name = "URL")]
    pub api_url: Option<String>,

    /// API key (ork_live_… / ork_test_…). Prefer ORK_TOKEN or `ork login` over this flag.
    #[arg(long, global = true, env = "ORK_TOKEN", hide_env_values = true, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Output format.
    #[arg(long, short = 'o', global = true, value_enum, env = "ORK_OUTPUT", default_value_t = OutputFormat::Table)]
    pub output: OutputFormat,

    /// Disable colors (also honoured: NO_COLOR).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// More logs (-v info, -vv debug, -vvv trace). Logs go to stderr.
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Run as if started in this directory.
    #[arg(long = "chdir", short = 'C', global = true, value_name = "DIR")]
    pub chdir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Store an API key for an OrkStage API (per API URL, outside the project).
    Login(LoginArgs),
    /// Forget the stored API key for the API URL.
    Logout,
    /// Show who the current credentials belong to.
    Whoami,
    /// Create .ork/ in the project root from a scan.
    Init(InitArgs),
    /// Detect infrastructure and services in a project and print the ScanReport.
    Scan(ScanArgs),
    /// Validate, push and diff the project template.
    #[command(subcommand)]
    Template(TemplateCommand),
    /// Scan locally and create an import (draft template) on the platform.
    Import(ImportArgs),
    /// List and inspect instances.
    #[command(subcommand)]
    Instances(InstancesCommand),
    /// Push the template, create the instance if it does not exist, then deploy it.
    Up(UpArgs),
    /// Deploy an instance.
    Deploy(DeployArgs),
    /// Show an instance, its resources and the last deployment.
    Status(InstanceSelector),
    /// Show recent logs of an instance resource.
    Logs(LogsArgs),
    /// Destroy an instance (asks for the slug as confirmation).
    Destroy(DestroyArgs),
    /// Show the plan for the next deploy, or for an upgrade with --template-version, without applying it.
    Preview(PreviewArgs),
    /// Upgrade an instance to another published version of its template (shows the plan first).
    Upgrade(UpgradeArgs),
    /// Incidents of the instances you can see.
    #[command(subcommand)]
    Incidents(IncidentsCommand),
    /// Triggers of an instance: actions declared in its template (restart, redeploy, backup, ...).
    #[command(subcommand)]
    Triggers(TriggersCommand),
    /// Browse the template marketplace.
    #[command(subcommand)]
    Marketplace(MarketplaceCommand),
    /// Print shell completions.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    // The key itself comes from the global `--token` / ORK_TOKEN, stdin or a prompt.
    /// Read the API key from stdin (CI-friendly).
    #[arg(long)]
    pub with_token: bool,
    /// Store the key without calling /me first.
    #[arg(long)]
    pub no_verify: bool,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Template name (default: the directory name).
    #[arg(long)]
    pub name: Option<String>,
    /// Tenant slug or id the project deploys to.
    #[arg(long)]
    pub tenant: Option<String>,
    /// Instance environment.
    #[arg(long, value_parser = ["production", "staging", "development", "sandbox"])]
    pub environment: Option<String>,
    /// Accept defaults, never prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Overwrite an existing .ork/ configuration.
    #[arg(long)]
    pub force: bool,
    /// Start from a template in the marketplace (listing slug) instead of the scan's template.
    #[arg(long, value_name = "SLUG")]
    pub from: Option<String>,
    /// Project directory (default: current directory).
    pub path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Directory to scan (default: the project root or the current directory).
    pub path: Option<PathBuf>,
    /// Report format (defaults to --output).
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// Update .ork/inventory.json and .ork/template.yaml.
    #[arg(long)]
    pub write: bool,
    /// Skip files larger than this many bytes.
    #[arg(long, value_name = "BYTES")]
    pub max_file_bytes: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum TemplateCommand {
    /// Validate the template with the platform (POST /templates/validate).
    Validate(TemplateFileArgs),
    /// Create the template or add a new version (patch version bumped automatically).
    Push(TemplateFileArgs),
    /// Show the difference between the local template and the latest server version.
    Diff(TemplateDiffArgs),
    /// Publish a draft version so instances can use it (needs permission to publish templates).
    Publish(TemplatePublishArgs),
}

#[derive(Debug, Args)]
pub struct TemplatePublishArgs {
    #[arg(long, short = 'f')]
    pub file: Option<PathBuf>,
    /// Version to publish (default: metadata.version of the local template).
    #[arg(long)]
    pub template_version: Option<String>,
}

#[derive(Debug, Args)]
pub struct TemplateFileArgs {
    /// Template file (default: .ork/template.yaml).
    #[arg(long, short = 'f')]
    pub file: Option<PathBuf>,
    /// Also check a tenant's limits and feature flags (slug, name or id).
    #[arg(long)]
    pub tenant: Option<String>,
}

#[derive(Debug, Args)]
pub struct TemplateDiffArgs {
    #[arg(long, short = 'f')]
    pub file: Option<PathBuf>,
    /// Exit with 1 when the templates differ.
    #[arg(long)]
    pub exit_code: bool,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Directory to scan (default: the project root or the current directory).
    pub path: Option<PathBuf>,
    /// Tenant slug or id (default: .ork/config.toml).
    #[arg(long)]
    pub tenant: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum InstancesCommand {
    /// List instances you can see.
    List {
        /// Only this tenant (slug or id).
        #[arg(long)]
        tenant: Option<String>,
        /// Include destroyed instances.
        #[arg(long)]
        all: bool,
    },
    /// Create the instance described in .ork/config.toml ([instance], [placements], [variables]).
    Create(InstanceCreateArgs),
}

#[derive(Debug, Args, Default)]
pub struct InstanceCreateArgs {
    /// Display name (default: the slug).
    #[arg(long)]
    pub name: Option<String>,
    /// Published template version to use (default: the latest published one).
    #[arg(long)]
    pub template_version: Option<String>,
    /// Template variable, repeatable: --var key=value (overrides [variables]).
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,
    /// Do not ask for confirmation (required when not running in a terminal).
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct UpArgs {
    /// Also publish the pushed template version (needs permission to publish templates).
    #[arg(long)]
    pub publish: bool,
    #[command(flatten)]
    pub create: InstanceCreateArgs,
    /// Stream deployment events until it finishes; exit non-zero on failure.
    #[arg(long, short = 'w')]
    pub wait: bool,
    /// Give up waiting after this many seconds.
    #[arg(long, default_value_t = 3600)]
    pub timeout: u64,
}

#[derive(Debug, Args)]
pub struct InstanceSelector {
    /// Instance slug (default: .ork/config.toml [instance].slug).
    #[arg(long, short = 'i')]
    pub instance: Option<String>,
}

#[derive(Debug, Args)]
pub struct DeployArgs {
    #[command(flatten)]
    pub selector: InstanceSelector,
    /// Stream deployment events until it finishes; exit non-zero on failure.
    #[arg(long, short = 'w')]
    pub wait: bool,
    /// Give up waiting after this many seconds.
    #[arg(long, default_value_t = 3600)]
    pub timeout: u64,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Template resource id, e.g. `api`.
    pub resource: String,
    #[command(flatten)]
    pub selector: InstanceSelector,
    /// Number of lines.
    #[arg(long, short = 'n', default_value_t = 50)]
    pub limit: u32,
}

#[derive(Debug, Args)]
pub struct PreviewArgs {
    #[command(flatten)]
    pub selector: InstanceSelector,
    /// Preview the change to this published template version (an upgrade or a rollback).
    #[arg(long)]
    pub template_version: Option<String>,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    #[command(flatten)]
    pub selector: InstanceSelector,
    /// Published version to move to (default: the latest published one).
    #[arg(long)]
    pub template_version: Option<String>,
    /// Do not ask for confirmation (required when not running in a terminal).
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Stream deployment events until it finishes; exit non-zero on failure.
    #[arg(long, short = 'w')]
    pub wait: bool,
    #[arg(long, default_value_t = 3600)]
    pub timeout: u64,
}

#[derive(Debug, Subcommand)]
pub enum IncidentsCommand {
    /// List open incidents (all of them with --all).
    List {
        /// Only this tenant (slug or id).
        #[arg(long)]
        tenant: Option<String>,
        /// Only this instance (slug).
        #[arg(long, short = 'i')]
        instance: Option<String>,
        /// Include resolved incidents.
        #[arg(long)]
        all: bool,
    },
    /// Show an incident and its updates.
    Show { id: String },
    /// Resolve an incident (needs the incidents:write scope).
    Resolve {
        id: String,
        /// Message for the incident timeline.
        #[arg(long, default_value = "Resolved from the ork CLI.")]
        message: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TriggersCommand {
    /// List the triggers of an instance and whether they can run.
    List(InstanceSelector),
    /// Run a trigger (API keys can run triggers whose template sets on.webhook).
    Run(TriggerRunArgs),
}

#[derive(Debug, Args)]
pub struct TriggerRunArgs {
    /// Trigger name from the template.
    pub name: String,
    #[command(flatten)]
    pub selector: InstanceSelector,
    /// Parameter override, repeatable: --param key=value (only names the template marks overridable).
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
    /// Wait until the run finishes; exit non-zero when it fails.
    #[arg(long, short = 'w')]
    pub wait: bool,
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
}

#[derive(Debug, Subcommand)]
pub enum MarketplaceCommand {
    /// List the published listings.
    List,
    /// Show a listing: description, placements, resources and estimated cost.
    Show { slug: String },
}

#[derive(Debug, Args)]
pub struct DestroyArgs {
    /// Instance slug.
    #[arg(long, short = 'i')]
    pub instance: String,
    /// Repeat the slug to confirm.
    #[arg(long)]
    pub confirm: String,
    /// Wait for the destroy to finish.
    #[arg(long, short = 'w')]
    pub wait: bool,
    #[arg(long, default_value_t = 3600)]
    pub timeout: u64,
}

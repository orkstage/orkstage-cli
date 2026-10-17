// SPDX-License-Identifier: AGPL-3.0-only
//! `ork targets`: connect a cloud account step by step (list, add, validate, edit), with the same
//! fields the console asks for. The forms come from `GET /cloud/providers`, so the CLI keeps no
//! copy of them. Needs a platform API key with the `targets:manage` and `instances:read` scopes.

use reqwest::Method;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::client;
use crate::api::{ApiClient, CloudTarget};
use crate::cli::TargetsCommand;
use crate::config::Ctx;
use crate::error::{CliError, Result};
use crate::output::{self, heading};

const DOCS: &str = "https://orkstage.io/docs/";
const ENVIRONMENTS: [&str; 5] = ["development", "staging", "production", "sandbox", "any"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub key: String,
    pub required: bool,
    pub example: String,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    pub config: Vec<FormField>,
    pub credential_fields: Vec<String>,
    pub regions_example: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub provider: String,
    pub display_name: String,
    pub kind: String,
    pub enabled: bool,
    pub form: Form,
}

/// Docs page that explains how to prepare an account of this provider.
pub fn docs_for(provider: &str) -> String {
    match provider {
        "aws" => format!("{DOCS}self-hosting/connect-aws/"),
        "mock" => format!("{DOCS}get-started/quickstart/"),
        p => format!("{DOCS}providers/{p}/"),
    }
}

/// `ORKSTAGE_<PROVIDER>_<NAME>[_SUFFIX]`: an env:// name outside the reserved prefixes.
pub fn env_name(provider: &str, name: &str, suffix: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_uppercase());
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let upper = provider.to_ascii_uppercase();
    // "aws-prod" for aws gives ORKSTAGE_AWS_PROD, not ORKSTAGE_AWS_AWS_PROD.
    let slug = slug.trim_matches('_');
    let slug = slug
        .strip_prefix(&upper)
        .filter(|rest| rest.is_empty() || rest.starts_with('_'))
        .map(|rest| rest.trim_start_matches('_'))
        .unwrap_or(slug);
    let mut parts =
        vec!["ORKSTAGE".to_string(), upper.clone(), if slug.is_empty() { "TARGET".into() } else { slug.to_string() }];
    if !suffix.is_empty() {
        parts.push(suffix.to_string());
    }
    parts.join("_")
}

/// 24 random bytes from the OS, as hex: an AWS external id.
pub fn new_external_id() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|e| CliError::failed(format!("no randomness from the OS: {e}"), None))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `KEY=VALUE` pairs; an empty value (`KEY=`) means "remove" for `edit`.
pub fn parse_pairs(pairs: &[String]) -> Result<Vec<(String, String)>> {
    pairs
        .iter()
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| CliError::usage(format!("--config {p:?} is not KEY=VALUE"), None))
        })
        .collect()
}

pub fn split_regions(s: &str) -> Vec<String> {
    s.split([',', ' ']).map(str::trim).filter(|r| !r.is_empty()).map(String::from).collect()
}

/// Checks a value against a field's pattern, with the example in the message.
pub fn check_field(field: &FormField, value: &str) -> std::result::Result<(), String> {
    if value.is_empty() {
        return if field.required { Err(format!("{} is required", field.key)) } else { Ok(()) };
    }
    if let Some(p) = &field.pattern {
        let re = regex::Regex::new(p).map_err(|e| e.to_string())?;
        if !re.is_match(value) {
            return Err(format!("{} does not look right (for example {})", field.key, field.example));
        }
    }
    Ok(())
}

async fn providers(api: &ApiClient) -> Result<Vec<Provider>> {
    api.get("/cloud/providers", &[]).await
}

async fn find_target(api: &ApiClient, wanted: &str) -> Result<CloudTarget> {
    let targets = api.targets().await?;
    let matches: Vec<_> =
        targets.into_iter().filter(|t| t.id == wanted || t.name.eq_ignore_ascii_case(wanted)).collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one match")),
        0 => Err(CliError::not_found(
            format!("cloud target \"{wanted}\" not found"),
            Some("`ork targets list` shows the names".into()),
        )),
        _ => Err(CliError::usage(
            format!("several targets are named \"{wanted}\""),
            Some("use the target id instead".into()),
        )),
    }
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Prints a validation report; returns whether every check passed.
fn print_validation(ctx: &Ctx, target: &Value) -> bool {
    let v = &target["lastValidation"];
    let ok = v["ok"].as_bool().unwrap_or(false);
    if ctx.json() {
        output::print_json(target);
        return ok;
    }
    let mut t = output::table(ctx, &["CHECK", "STATUS", "DETAIL"]);
    for c in v["checks"].as_array().into_iter().flatten() {
        t.add_row(vec![
            comfy_table::Cell::new(text(c, "name")),
            output::status_cell(ctx, text(c, "status")),
            comfy_table::Cell::new(text(c, "message")),
        ]);
    }
    crate::outln!("{t}");
    crate::outln!(
        "{} {}",
        heading(ctx, "Result:"),
        if ok {
            "every required check passed"
        } else {
            "some checks failed; fix them and run `ork targets validate` again"
        }
    );
    ok
}

pub async fn run(ctx: &Ctx, cmd: TargetsCommand) -> Result<()> {
    match cmd {
        TargetsCommand::ExternalId => {
            crate::outln!("{}", new_external_id()?);
            Ok(())
        }
        TargetsCommand::List => {
            let api = client(ctx)?;
            let targets: Vec<Value> = api.get("/cloud/targets", &[]).await?;
            if ctx.json() {
                output::print_json(&targets);
                return Ok(());
            }
            if targets.is_empty() {
                eprintln!("No cloud targets. Connect one with `ork targets add`.");
                return Ok(());
            }
            let mut t = output::table(ctx, &["NAME", "PROVIDER", "ENVIRONMENT", "REGIONS", "HEALTH", "VALIDATED"]);
            for x in &targets {
                let regions: Vec<&str> =
                    x["allowedRegions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let validated = match x["lastValidation"]["ok"].as_bool() {
                    Some(true) => "passed",
                    Some(false) => "failed",
                    None => "never",
                };
                t.add_row(vec![
                    comfy_table::Cell::new(text(x, "name")),
                    comfy_table::Cell::new(text(x, "provider")),
                    comfy_table::Cell::new(text(x, "environment")),
                    comfy_table::Cell::new(regions.join(", ")),
                    output::status_cell(ctx, text(x, "health")),
                    comfy_table::Cell::new(validated),
                ]);
            }
            crate::outln!("{t}");
            Ok(())
        }
        TargetsCommand::Validate { target, region } => {
            let api = client(ctx)?;
            let t = find_target(&api, &target).await?;
            let body = match region {
                Some(r) => json!({ "region": r }),
                None => json!({}),
            };
            let updated: Value = api.post(&format!("/cloud/targets/{}/validate", t.id), &body).await?;
            if print_validation(ctx, &updated) {
                Ok(())
            } else {
                Err(CliError::failed(format!("{} did not pass validation", t.name), None))
            }
        }
        TargetsCommand::Edit { target, name, config, credential_ref, regions, default_region, max_concurrent } => {
            let api = client(ctx)?;
            let t = find_target(&api, &target).await?;
            let mut body = Map::new();
            if let Some(n) = name {
                body.insert("name".into(), json!(n));
            }
            let pairs = parse_pairs(&config)?;
            if !pairs.is_empty() {
                let cfg: Map<String, Value> =
                    pairs.into_iter().map(|(k, v)| (k, if v.is_empty() { Value::Null } else { json!(v) })).collect();
                body.insert("config".into(), Value::Object(cfg));
            }
            if let Some(r) = credential_ref {
                body.insert("credentialRef".into(), json!(r));
            }
            if let Some(r) = regions {
                body.insert("allowedRegions".into(), json!(split_regions(&r)));
            }
            if let Some(r) = default_region {
                body.insert("defaultRegion".into(), json!(r));
            }
            if let Some(m) = max_concurrent {
                body.insert("maxConcurrentDeployments".into(), json!(m));
            }
            if body.is_empty() {
                return Err(CliError::usage("nothing to change", Some("pass --name, --config KEY=VALUE, --credential-ref, --regions, --default-region or --max-concurrent".into())));
            }
            let updated: Value = api
                .request(Method::PATCH, &format!("/cloud/targets/{}", t.id), &[], Some(&Value::Object(body)))
                .await?;
            if ctx.json() {
                output::print_json(&updated);
            } else {
                crate::outln!(
                    "Updated {}. Run `ork targets validate {}` to check it again.",
                    text(&updated, "name"),
                    text(&updated, "name")
                );
            }
            Ok(())
        }
        TargetsCommand::Add(args) => add(ctx, args).await,
    }
}

async fn add(ctx: &Ctx, args: crate::cli::TargetsAddArgs) -> Result<()> {
    let api = client(ctx)?;
    let interactive = ctx.interactive && !args.yes;
    let prompt_err = |e: dialoguer::Error| CliError::failed(e.to_string(), None);
    let all = providers(&api).await?;

    // 1. Provider
    let provider = match &args.provider {
        Some(p) => all.iter().find(|x| &x.provider == p).cloned().ok_or_else(|| {
            CliError::usage(
                format!("unknown provider {p:?}"),
                Some(format!("one of: {}", all.iter().map(|x| x.provider.as_str()).collect::<Vec<_>>().join(", "))),
            )
        })?,
        None if interactive => {
            let labels: Vec<String> = all
                .iter()
                .map(|p| format!("{} ({}){}", p.display_name, p.kind, if p.enabled { "" } else { ", switched off" }))
                .collect();
            let i = dialoguer::Select::new()
                .with_prompt("Provider")
                .items(&labels)
                .default(0)
                .interact()
                .map_err(prompt_err)?;
            all[i].clone()
        }
        None => return Err(CliError::usage("--provider is required without a terminal", None)),
    };
    if !provider.enabled {
        return Err(CliError::failed(
            format!("{} is switched off on this platform", provider.display_name),
            Some(format!(
                "a platform owner turns it on in Settings > Feature flags (PROVIDER_{})",
                provider.provider.to_ascii_uppercase()
            )),
        ));
    }
    if !ctx.json() {
        output::note(
            ctx,
            &format!("Prepare the account first (role, state store, secret): {}", docs_for(&provider.provider)),
        );
    }

    // 2. Name, environment, tenant
    let name = match &args.name {
        Some(n) => n.clone(),
        None if interactive => dialoguer::Input::<String>::new()
            .with_prompt("Target name")
            .default(format!(
                "{}-{}",
                provider.provider,
                args.environment.clone().unwrap_or_else(|| "development".into())
            ))
            .interact_text()
            .map_err(prompt_err)?,
        None => return Err(CliError::usage("--name is required without a terminal", None)),
    };
    let environment = match &args.environment {
        Some(e) => e.clone(),
        None if interactive => {
            let i = dialoguer::Select::new()
                .with_prompt("Environment it serves")
                .items(ENVIRONMENTS)
                .default(0)
                .interact()
                .map_err(prompt_err)?;
            ENVIRONMENTS[i].to_string()
        }
        None => "development".into(),
    };
    let tenant_id = match &args.tenant {
        Some(t) => Some(api.resolve_tenant(t).await?.id),
        None => None,
    };

    // 3. Config fields of the provider's form
    let given: std::collections::HashMap<String, String> = parse_pairs(&args.config)?.into_iter().collect();
    let mut config = Map::new();
    for field in &provider.form.config {
        let value = match given.get(&field.key) {
            Some(v) => v.clone(),
            None if interactive => {
                let f = field.clone();
                let mut input = dialoguer::Input::<String>::new()
                    .with_prompt(format!("{}{}", field.key, if field.required { "" } else { " (optional)" }))
                    .allow_empty(!field.required)
                    .validate_with(move |s: &String| check_field(&f, s.trim()));
                if let Some(d) = &field.default_value {
                    input = input.default(d.clone());
                }
                input.interact_text().map_err(prompt_err)?.trim().to_string()
            }
            None => field.default_value.clone().unwrap_or_default(),
        };
        check_field(field, &value)
            .map_err(|m| CliError::usage(m, Some(format!("example: --config {}={}", field.key, field.example))))?;
        if !value.is_empty() {
            config.insert(field.key.clone(), json!(value));
        }
    }
    for (k, v) in &given {
        if !config.contains_key(k) {
            config.insert(k.clone(), json!(v));
        }
    }

    // 4. Credential reference (never the secret itself)
    let credential_ref = if provider.form.credential_fields.is_empty() {
        None
    } else {
        let suggested = format!(
            "env://{}",
            env_name(&provider.provider, &name, if provider.provider == "aws" { "EXTERNAL_ID" } else { "" })
        );
        let r = match &args.credential_ref {
            Some(r) => r.clone(),
            None if interactive => dialoguer::Input::<String>::new()
                .with_prompt(format!(
                    "Credential reference (secret with: {})",
                    provider.form.credential_fields.join(", ")
                ))
                .default(suggested)
                .validate_with(|s: &String| -> std::result::Result<(), String> {
                    if s.starts_with("env://") || s.starts_with("aws-sm://") {
                        Ok(())
                    } else {
                        Err("use env://NAME or aws-sm://name, never the secret".into())
                    }
                })
                .interact_text()
                .map_err(prompt_err)?,
            None => {
                return Err(CliError::usage(
                    "--credential-ref is required without a terminal",
                    Some(format!("for example --credential-ref {suggested}")),
                ));
            }
        };
        Some(r)
    };

    // 5. Regions
    let regions = match &args.regions {
        Some(r) => split_regions(r),
        None if interactive => split_regions(
            &dialoguer::Input::<String>::new()
                .with_prompt("Allowed regions (comma separated)")
                .default(provider.form.regions_example.split(',').next().unwrap_or("").trim().to_string())
                .interact_text()
                .map_err(prompt_err)?,
        ),
        None => {
            return Err(CliError::usage(
                "--regions is required without a terminal",
                Some(format!("for example --regions {}", provider.form.regions_example.replace(' ', ""))),
            ));
        }
    };
    if regions.is_empty() {
        return Err(CliError::usage("at least one region is required", None));
    }
    let default_region = args.default_region.clone().unwrap_or_else(|| regions[0].clone());

    let mut body = json!({
        "provider": provider.provider,
        "name": name,
        "environment": environment,
        "config": Value::Object(config),
        "allowedRegions": regions,
        "defaultRegion": default_region,
    });
    if let Some(r) = credential_ref {
        body["credentialRef"] = json!(r);
    }
    if let Some(t) = tenant_id {
        body["tenantId"] = json!(t);
    }
    let created: Value = api.post("/cloud/targets", &body).await?;
    if !ctx.json() {
        crate::outln!("{} {} ({})", heading(ctx, "Registered:"), text(&created, "name"), text(&created, "id"));
    }
    if args.no_validate {
        if ctx.json() {
            output::print_json(&created);
        }
        return Ok(());
    }
    if !ctx.json() {
        output::note(ctx, "Validating: signing in with the reference, reaching the state store, checking permissions.");
    }
    let validated: Value = api.post(&format!("/cloud/targets/{}/validate", text(&created, "id")), &json!({})).await?;
    if print_validation(ctx, &validated) {
        if !ctx.json() {
            crate::outln!(
                "Next: add it to a profile in the console (Profiles), or deploy the portable-starter template."
            );
        }
        Ok(())
    } else {
        Err(CliError::failed(
            format!("{} is registered but did not pass validation", text(&created, "name")),
            Some("fix the failed checks, then `ork targets validate`".into()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, required: bool, pattern: Option<&str>) -> FormField {
        FormField {
            key: key.into(),
            required,
            example: "123456789012".into(),
            pattern: pattern.map(String::from),
            default_value: None,
        }
    }

    #[test]
    fn env_names_avoid_reserved_prefixes() {
        assert_eq!(env_name("aws", "aws prod", "EXTERNAL_ID"), "ORKSTAGE_AWS_PROD_EXTERNAL_ID");
        assert_eq!(env_name("aws", "awsome", ""), "ORKSTAGE_AWS_AWSOME");
        assert_eq!(env_name("aws", "aws", ""), "ORKSTAGE_AWS_TARGET");
        assert_eq!(env_name("azure", "Acme / dev!", ""), "ORKSTAGE_AZURE_ACME_DEV");
        assert_eq!(env_name("oci", "", ""), "ORKSTAGE_OCI_TARGET");
    }

    #[test]
    fn external_ids_are_random_hex() {
        let a = new_external_id().unwrap();
        let b = new_external_id().unwrap();
        assert_eq!(a.len(), 48);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn fields_follow_the_form() {
        let account = field("accountId", true, Some("^\\d{12}$"));
        assert!(check_field(&account, "123456789012").is_ok());
        assert!(check_field(&account, "12").unwrap_err().contains("for example"));
        assert!(check_field(&account, "").unwrap_err().contains("required"));
        assert!(check_field(&field("dnsZone", false, None), "").is_ok());
    }

    #[test]
    fn pairs_and_regions_parse() {
        let p = parse_pairs(&["accountId=1".into(), "dnsZone=".into()]).unwrap();
        assert_eq!(p, vec![("accountId".into(), "1".into()), ("dnsZone".into(), "".into())]);
        assert!(parse_pairs(&["nope".into()]).is_err());
        assert_eq!(split_regions("us-east-1, sa-east-1 eu-west-1"), vec!["us-east-1", "sa-east-1", "eu-west-1"]);
    }

    #[test]
    fn docs_pages_per_provider() {
        assert_eq!(docs_for("aws"), "https://orkstage.io/docs/self-hosting/connect-aws/");
        assert_eq!(docs_for("azure"), "https://orkstage.io/docs/providers/azure/");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! `ork instances create` and `ork up`: from a project directory to a deployed instance.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::client;
use super::operate::deploy_slug;
use super::template::{publish_version, push_file, template_path};
use crate::api::{ApiClient, CloudTarget, Instance, Template, TemplateVersion, Tenant, is_uuid};
use crate::cli::{InstanceCreateArgs, UpArgs};
use crate::config::{Ctx, Project};
use crate::error::{CliError, Result};
use crate::output::{self, heading};

/// What `.ork/config.toml` must describe before anything is sent.
struct Plan {
    slug: String,
    environment: String,
    tenant: String,
    template: String,
    profile: Option<String>,
    placements: BTreeMap<String, (String, String)>,
    variables: Map<String, Value>,
}

fn plan(ctx: &Ctx, args: &InstanceCreateArgs) -> Result<Plan> {
    let project: &Project = ctx.require_project()?;
    let cfg = &project.config;
    let missing = |what: &str, fix: &str| CliError::usage(format!("{what} is not set"), Some(fix.to_string()));
    let slug = cfg.instance.slug.clone().ok_or_else(|| missing("[instance].slug", "set it in .ork/config.toml"))?;
    let tenant = ctx.tenant().ok_or_else(|| missing("tenant", "set `tenant` in .ork/config.toml"))?;
    let template = cfg.template.name.clone().ok_or_else(|| missing("[template].name", "set it in .ork/config.toml"))?;
    // Slots missing from [placements] are bound by the platform from [instance].profile, or from the
    // default profile of the instance's environment; the API names any slot left without a target.
    let mut variables = Map::new();
    for (k, v) in &cfg.variables {
        variables.insert(k.clone(), toml_value(k, v)?);
    }
    for raw in &args.vars {
        let (k, v) = parse_var(raw)?;
        variables.insert(k, v);
    }
    if !args.yes && !ctx.interactive {
        return Err(CliError::usage(
            "creating an instance needs confirmation",
            Some("pass --yes when not running in a terminal".into()),
        ));
    }
    Ok(Plan {
        slug,
        environment: cfg.instance.environment.clone().unwrap_or_else(|| "production".into()),
        tenant,
        template,
        profile: cfg.instance.profile.clone(),
        placements: cfg.placements.iter().map(|(s, b)| (s.clone(), (b.target.clone(), b.region.clone()))).collect(),
        variables,
    })
}

/// `key=value`; `true`/`false` and numbers keep their type, everything else is a string.
pub fn parse_var(raw: &str) -> Result<(String, Value)> {
    let (k, v) = raw.split_once('=').filter(|(k, _)| !k.trim().is_empty()).ok_or_else(|| {
        CliError::usage(format!("--var {raw:?} is not KEY=VALUE"), Some("for example --var replicas=2".into()))
    })?;
    let value = match v {
        "true" => json!(true),
        "false" => json!(false),
        _ => v
            .parse::<i64>()
            .map(|n| json!(n))
            .or_else(|_| v.parse::<f64>().map(|n| json!(n)))
            .unwrap_or_else(|_| json!(v)),
    };
    Ok((k.trim().to_string(), value))
}

fn toml_value(key: &str, v: &toml::Value) -> Result<Value> {
    match v {
        toml::Value::String(s) => Ok(json!(s)),
        toml::Value::Integer(n) => Ok(json!(n)),
        toml::Value::Float(n) => Ok(json!(n)),
        toml::Value::Boolean(b) => Ok(json!(b)),
        _ => Err(CliError::usage(
            format!("[variables].{key} must be a string, number or boolean"),
            Some("fix it in .ork/config.toml".into()),
        )),
    }
}

fn published(t: &Template, wanted: Option<&str>) -> Result<TemplateVersion> {
    let versions = t.versions.clone().unwrap_or_default();
    match wanted {
        Some(w) => versions.into_iter().find(|v| v.version == w && v.status == "PUBLISHED").ok_or_else(|| {
            CliError::not_found(
                format!("{} has no published version {w}", t.name),
                Some("publish it with `ork template publish --template-version` or in the console".into()),
            )
        }),
        None => t.latest_published.clone().ok_or_else(|| {
            CliError::not_found(
                format!("{} has no published version yet", t.name),
                Some("publish it with `ork template publish` (or `ork up --publish`) or in the console".into()),
            )
        }),
    }
}

/// Target names become ids. Listing targets needs a platform role, so customers use ids.
async fn bindings(api: &ApiClient, tenant: &Tenant, placements: &BTreeMap<String, (String, String)>) -> Result<Value> {
    let mut targets: Option<Vec<CloudTarget>> = None;
    let mut out = Map::new();
    for (slot, (target, region)) in placements {
        let id = if is_uuid(target) {
            target.clone()
        } else {
            if targets.is_none() {
                targets = Some(api.targets().await.map_err(|e| match e {
                    CliError::Api { status: 403, .. } => CliError::usage(
                        "this API key cannot list cloud targets to resolve their names",
                        Some("use target ids in [placements] (ask a platform administrator for them)".into()),
                    ),
                    other => other,
                })?);
            }
            let list = targets.as_deref().unwrap_or_default();
            let found = list
                .iter()
                .filter(|t| t.name == *target)
                .find(|t| t.tenant_id.as_deref().is_none_or(|id| id == tenant.id))
                .ok_or_else(|| {
                    CliError::not_found(
                        format!("cloud target \"{target}\" (slot {slot}) not found for tenant {}", tenant.slug),
                        Some("check [placements] in .ork/config.toml".into()),
                    )
                })?;
            found.id.clone()
        };
        out.insert(slot.clone(), json!({ "targetId": id, "region": region }));
    }
    Ok(Value::Object(out))
}

fn confirm(summary: &str) -> Result<()> {
    eprintln!("{summary}");
    let ok = dialoguer::Confirm::new()
        .with_prompt("Create this instance?")
        .default(false)
        .interact()
        .map_err(|e| CliError::failed(e.to_string(), None))?;
    if ok { Ok(()) } else { Err(CliError::failed("cancelled", None)) }
}

async fn create_instance(ctx: &Ctx, api: &ApiClient, args: &InstanceCreateArgs) -> Result<Instance> {
    let p = plan(ctx, args)?;
    let tenant = api.resolve_tenant(&p.tenant).await?;
    // Slugs stay taken after a destroy, so explain a clash instead of surfacing a bare 409.
    if let Some(i) = api.instances(Some(&tenant.id)).await?.into_iter().find(|i| i.slug == p.slug) {
        return Err(if i.status == "DESTROYED" {
            CliError::usage(
                format!("instance \"{}\" was destroyed and its slug cannot be reused", p.slug),
                Some("set a new [instance].slug in .ork/config.toml".into()),
            )
        } else {
            CliError::usage(
                format!("instance \"{}\" already exists ({})", p.slug, i.status),
                Some("deploy it with `ork deploy --wait`, or set a new [instance].slug".into()),
            )
        });
    }
    let template = api.template_by_name(&p.template).await?.ok_or_else(|| {
        CliError::not_found(
            format!("template \"{}\" does not exist on the server", p.template),
            Some("create it with `ork template push`".into()),
        )
    })?;
    let version = published(&template, args.template_version.as_deref())?;
    let bindings = bindings(api, &tenant, &p.placements).await?;
    let profile = match &p.profile {
        Some(wanted) => Some(api.resolve_profile(&tenant, wanted).await?),
        None => None,
    };
    if !args.yes {
        let mut slots: Vec<String> = p.placements.iter().map(|(s, (t, r))| format!("{s} on {t} ({r})")).collect();
        slots.push(match &profile {
            Some(pr) => format!("other slots from profile {}", pr.name),
            None => "other slots from the default profile".into(),
        });
        confirm(&format!(
            "{} {} ({}) in tenant {}\n{} {}@{}\n{} {}",
            heading(ctx, "Instance:"),
            p.slug,
            p.environment,
            tenant.slug,
            heading(ctx, "Template:"),
            template.name,
            version.version,
            heading(ctx, "Placements:"),
            slots.join(", ")
        ))?;
    }
    let mut body = json!({
        "tenantId": tenant.id,
        "name": args.name.clone().unwrap_or_else(|| p.slug.clone()),
        "slug": p.slug,
        "environment": p.environment,
        "templateVersionId": version.id,
        "variables": p.variables,
        "bindings": bindings,
    });
    if let Some(pr) = &profile {
        body["profileId"] = json!(pr.id);
    }
    api.post("/instances", &body).await
}

pub async fn create(ctx: &Ctx, args: InstanceCreateArgs) -> Result<()> {
    // Check the local configuration before touching the server.
    plan(ctx, &args)?;
    let api = client(ctx)?;
    let instance = create_instance(ctx, &api, &args).await?;
    if ctx.json() {
        output::print_json(&instance);
    } else {
        eprintln!(
            "Created instance {} ({}{}). Deploy it with `ork deploy --wait`.",
            instance.slug,
            instance.environment,
            if instance.simulated { ", simulated" } else { "" }
        );
    }
    Ok(())
}

/// Push the template when it changed (and publish it with --publish), create the instance when it
/// does not exist, then deploy. Progress goes to stderr; with --output json, stdout gets the
/// deployment only.
pub async fn up(ctx: &Ctx, args: UpArgs) -> Result<()> {
    // Check the local configuration before touching the server.
    plan(ctx, &args.create)?;
    let api = client(ctx)?;
    let pushed = push_file(&api, &template_path(ctx, None)?).await?;
    let name = pushed["template"].as_str().unwrap_or("").to_string();
    let version = pushed["version"].as_str().unwrap_or("").to_string();
    let draft = pushed["action"] != "up-to-date" || pushed["status"].as_str() == Some("DRAFT");
    match pushed["action"].as_str() {
        Some("up-to-date") => output::note(ctx, &format!("Template {name}@{version} is up to date.")),
        _ => output::note(ctx, &format!("Pushed template {name}@{version} (draft).")),
    }
    if draft && args.publish {
        publish_version(&api, &name, &version).await?;
        output::note(ctx, &format!("Published {name}@{version}."));
    } else if draft {
        output::note(
            ctx,
            &format!(
                "{name}@{version} is a draft: new instances use the latest published version. Pass --publish to publish it."
            ),
        );
    }

    let slug = ctx.require_project()?.config.instance.slug.clone().unwrap_or_default();
    // A destroyed instance is left to create_instance, which explains that its slug is taken.
    let exists = api.instances(None).await?.into_iter().any(|i| i.slug == slug && i.status != "DESTROYED");
    if exists {
        output::note(ctx, &format!("Instance {slug} exists."));
    } else {
        let i = create_instance(ctx, &api, &args.create).await?;
        output::note(ctx, &format!("Created instance {} from {}@{}.", i.slug, i.template.name, i.template.version));
    }
    deploy_slug(ctx, &api, &slug, args.wait, args.timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars_keep_their_type() {
        assert_eq!(parse_var("replicas=2").unwrap(), ("replicas".into(), json!(2)));
        assert_eq!(parse_var("ratio=0.5").unwrap(), ("ratio".into(), json!(0.5)));
        assert_eq!(parse_var("public=true").unwrap(), ("public".into(), json!(true)));
        assert_eq!(parse_var("image=app:1.2=x").unwrap(), ("image".into(), json!("app:1.2=x")));
        assert_eq!(parse_var("empty=").unwrap(), ("empty".into(), json!("")));
        assert!(parse_var("novalue").is_err());
        assert!(parse_var("=x").is_err());
    }

    #[test]
    fn uuids() {
        assert!(is_uuid("0f8b2c1e-3a4d-4e5f-9a8b-7c6d5e4f3a2b"));
        assert!(!is_uuid("aws-prod"));
        assert!(!is_uuid("0f8b2c1e3a4d4e5f9a8b7c6d5e4f3a2b0000"));
    }
}

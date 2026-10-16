// SPDX-License-Identifier: AGPL-3.0-only
//! Day two operations: preview, upgrade, incidents, triggers and the marketplace.

use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{Map, Value, json};

use super::client;
use super::operate::{finish, follow, selected_slug};
use super::up::parse_var;
use crate::api::{ApiClient, Deployment, Instance, Page};
use crate::cli::{
    IncidentsCommand, InstanceSelector, MarketplaceCommand, PreviewArgs, TriggerRunArgs, TriggersCommand, UpgradeArgs,
};
use crate::config::Ctx;
use crate::error::{CliError, Result};
use crate::output::{self, heading};

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

async fn instance(ctx: &Ctx, api: &ApiClient, sel: &InstanceSelector) -> Result<Instance> {
    api.instance_by_slug(&selected_slug(ctx, sel)?).await
}

/// Id of a published version of the instance's template: the one asked for, else the latest.
async fn published_version(api: &ApiClient, inst: &Instance, wanted: Option<&str>) -> Result<(String, String)> {
    let t = api.template_by_name(&inst.template.name).await?.ok_or_else(|| {
        CliError::not_found(format!("template \"{}\" is not visible to this API key", inst.template.name), None)
    })?;
    let v = match wanted {
        Some(w) => t.versions.unwrap_or_default().into_iter().find(|v| v.version == w && v.status == "PUBLISHED"),
        None => t.latest_published,
    };
    let v = v.ok_or_else(|| {
        CliError::not_found(
            format!("{} has no published version {}", t.name, wanted.unwrap_or("yet")),
            Some("publish it with `ork template publish`".into()),
        )
    })?;
    Ok((v.id, v.version))
}

async fn fetch_preview(api: &ApiClient, inst: &Instance, version_id: Option<&str>) -> Result<Value> {
    let query: Vec<(&str, String)> = version_id.map(|v| vec![("templateVersionId", v.to_string())]).unwrap_or_default();
    api.request(Method::POST, &format!("/instances/{}/preview", inst.id), &query, Some(&json!({}))).await
}

/// The plan as text, its counts and risk, the monthly cost and any blocking issue.
fn print_preview(ctx: &Ctx, p: &Value) {
    let plan = &p["plan"];
    let count = |k: &str| plan["counts"][k].as_u64().unwrap_or(0);
    let plan_text = text(plan, "text");
    if !plan_text.is_empty() {
        crate::outln!("{}", plan_text.trim_end());
        crate::outln!("");
    }
    crate::outln!(
        "{} {} to create, {} to update, {} to delete, {} unchanged (risk {})",
        heading(ctx, "Plan:"),
        count("CREATE"),
        count("UPDATE"),
        count("DELETE"),
        count("NOOP"),
        text(plan, "risk")
    );
    if let Some(total) = p["cost"]["total"].as_f64() {
        crate::outln!(
            "{} {total:.2} {} a month ({} confidence)",
            heading(ctx, "Cost:"),
            text(&p["cost"], "currency"),
            text(&p["cost"], "confidence")
        );
    }
    for i in p["issues"].as_array().into_iter().flatten().filter(|i| text(i, "severity") == "error") {
        crate::outln!("{} {}", heading(ctx, "Issue:"), text(i, "message"));
    }
}

pub async fn preview(ctx: &Ctx, args: PreviewArgs) -> Result<()> {
    let api = client(ctx)?;
    let inst = instance(ctx, &api, &args.selector).await?;
    let version_id = match &args.template_version {
        Some(v) => Some(published_version(&api, &inst, Some(v)).await?.0),
        None => None,
    };
    let p = fetch_preview(&api, &inst, version_id.as_deref()).await?;
    if ctx.json() {
        output::print_json(&p);
    } else {
        print_preview(ctx, &p);
    }
    Ok(())
}

pub async fn upgrade(ctx: &Ctx, args: UpgradeArgs) -> Result<()> {
    if !args.yes && !ctx.interactive {
        return Err(CliError::usage(
            "an upgrade needs confirmation",
            Some("pass --yes when not running in a terminal".into()),
        ));
    }
    let api = client(ctx)?;
    let inst = instance(ctx, &api, &args.selector).await?;
    let (version_id, version) = published_version(&api, &inst, args.template_version.as_deref()).await?;
    if version == inst.template.version {
        output::note(ctx, &format!("{} already runs {}@{version}.", inst.slug, inst.template.name));
        return Ok(());
    }
    let p = fetch_preview(&api, &inst, Some(&version_id)).await?;
    if !ctx.json() {
        print_preview(ctx, &p);
    }
    if p["valid"] == false {
        return Err(CliError::failed(
            format!("{}@{version} cannot be applied to {}", inst.template.name, inst.slug),
            Some("see the issues above".into()),
        ));
    }
    if !args.yes {
        let ok = dialoguer::Confirm::new()
            .with_prompt(format!("Upgrade {} from {} to {version}?", inst.slug, inst.template.version))
            .default(false)
            .interact()
            .map_err(|e| CliError::failed(e.to_string(), None))?;
        if !ok {
            return Err(CliError::failed("cancelled", None));
        }
    }
    let d: Deployment =
        api.post(&format!("/instances/{}/upgrade", inst.id), &json!({ "templateVersionId": version_id })).await?;
    output::note(ctx, &format!("Upgrade of {} to {version} queued as deployment {}.", inst.slug, d.id));
    if !args.wait {
        if ctx.json() {
            output::print_json(&d);
        }
        return Ok(());
    }
    let done = follow(ctx, &api, &d.id, Duration::from_secs(args.timeout)).await?;
    finish(ctx, done)
}

pub async fn incidents(ctx: &Ctx, cmd: IncidentsCommand) -> Result<()> {
    let api = client(ctx)?;
    match cmd {
        IncidentsCommand::List { tenant, instance, all } => {
            let mut q: Vec<(&str, String)> = vec![("limit", "100".into())];
            if !all {
                q.push(("status", "open".into()));
            }
            if let Some(t) = tenant {
                q.push(("tenantId", api.resolve_tenant(&t).await?.id));
            }
            if let Some(i) = instance {
                q.push(("instanceId", api.instance_by_slug(&i).await?.id));
            }
            let page: Page<Value> = api.get("/incidents", &q).await?;
            if ctx.json() {
                output::print_json(&page.items);
                return Ok(());
            }
            if page.items.is_empty() {
                eprintln!("{}", if all { "No incidents." } else { "No open incidents." });
                return Ok(());
            }
            let mut t = output::table(ctx, &["ID", "TITLE", "STATUS", "SEVERITY", "SOURCE", "OPENED"]);
            for i in &page.items {
                t.add_row(vec![
                    comfy_table::Cell::new(text(i, "id")),
                    comfy_table::Cell::new(text(i, "title")),
                    output::status_cell(ctx, text(i, "status")),
                    comfy_table::Cell::new(text(i, "severity")),
                    comfy_table::Cell::new(text(i, "source")),
                    comfy_table::Cell::new(text(i, "openedAt").get(..16).unwrap_or("")),
                ]);
            }
            crate::outln!("{t}");
        }
        IncidentsCommand::Show { id } => {
            let i: Value = api.get(&format!("/incidents/{id}"), &[]).await?;
            if ctx.json() {
                output::print_json(&i);
                return Ok(());
            }
            crate::outln!("{} {}", heading(ctx, "Incident:"), text(&i, "title"));
            crate::outln!(
                "{} {}   {} {}   {} {}",
                heading(ctx, "Status:"),
                text(&i, "status"),
                heading(ctx, "Severity:"),
                text(&i, "severity"),
                heading(ctx, "Impact:"),
                text(&i, "impact")
            );
            crate::outln!("{} {}", heading(ctx, "Opened:"), text(&i, "openedAt"));
            for u in i["updates"].as_array().into_iter().flatten() {
                crate::outln!(
                    "  {} {:13} {}",
                    text(u, "at").get(..19).unwrap_or(""),
                    text(u, "status"),
                    text(u, "message")
                );
            }
        }
        IncidentsCommand::Resolve { id, message } => {
            let r: Value = api
                .post(&format!("/incidents/{id}/updates"), &json!({ "status": "RESOLVED", "message": message }))
                .await?;
            if ctx.json() {
                output::print_json(&r);
            } else {
                eprintln!("Resolved incident {id}.");
            }
        }
    }
    Ok(())
}

pub async fn triggers(ctx: &Ctx, cmd: TriggersCommand) -> Result<()> {
    match cmd {
        TriggersCommand::List(sel) => {
            let api = client(ctx)?;
            let inst = instance(ctx, &api, &sel).await?;
            let list: Vec<Value> = api.get(&format!("/instances/{}/triggers", inst.id), &[]).await?;
            if ctx.json() {
                output::print_json(&list);
                return Ok(());
            }
            if list.is_empty() {
                eprintln!("{} has no triggers (the template declares none).", inst.slug);
                return Ok(());
            }
            let mut t = output::table(ctx, &["NAME", "RESOURCE", "ACTION", "API KEYS", "AVAILABLE", "LAST RUN"]);
            for x in &list {
                let available =
                    if x["available"] == true { "yes".to_string() } else { text(x, "unavailableReason").to_string() };
                let last = x.get("lastRun").filter(|l| !l.is_null());
                t.add_row(vec![
                    comfy_table::Cell::new(text(x, "name")),
                    comfy_table::Cell::new(text(x, "resourceId")),
                    comfy_table::Cell::new(text(x, "action")),
                    comfy_table::Cell::new(if x["on"]["webhook"] == true { "yes" } else { "no" }),
                    comfy_table::Cell::new(available),
                    comfy_table::Cell::new(
                        last.map(|l| format!("{} {}", text(l, "status"), text(l, "queuedAt").get(..16).unwrap_or("")))
                            .unwrap_or_default(),
                    ),
                ]);
            }
            crate::outln!("{t}");
            Ok(())
        }
        TriggersCommand::Run(args) => run_trigger(ctx, args).await,
    }
}

async fn run_trigger(ctx: &Ctx, args: TriggerRunArgs) -> Result<()> {
    let mut params = Map::new();
    for raw in &args.params {
        let (k, v) = parse_var(raw)?;
        params.insert(k, v);
    }
    let api = client(ctx)?;
    let inst = instance(ctx, &api, &args.selector).await?;
    let run: Value =
        api.post(&format!("/instances/{}/triggers/{}/runs", inst.id, args.name), &json!({ "params": params })).await?;
    let id = text(&run, "id").to_string();
    output::note(ctx, &format!("Trigger {} queued on {} (run {id}).", args.name, inst.slug));
    if !args.wait {
        if ctx.json() {
            output::print_json(&run);
        }
        return Ok(());
    }
    let start = Instant::now();
    let finished = loop {
        let page: Page<Value> =
            api.get(&format!("/instances/{}/trigger-runs", inst.id), &[("limit", "50".into())]).await?;
        if let Some(r) = page.items.into_iter().find(|r| text(r, "id") == id)
            && matches!(text(&r, "status"), "SUCCEEDED" | "FAILED")
        {
            break r;
        }
        if start.elapsed() > Duration::from_secs(args.timeout) {
            return Err(CliError::failed(
                format!("timed out after {}s waiting for trigger run {id}", args.timeout),
                Some("the run keeps going; see it in the console".into()),
            ));
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    };
    if ctx.json() {
        output::print_json(&finished);
    }
    let detail = [text(&finished, "message"), text(&finished, "url")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text(&finished, "status") == "SUCCEEDED" {
        output::note(ctx, format!("Trigger {} succeeded. {detail}", args.name).trim_end());
        Ok(())
    } else {
        Err(CliError::failed(
            format!("trigger {} failed", args.name),
            Some(format!("{}: {detail}", text(&finished, "errorClass"))),
        ))
    }
}

pub async fn marketplace(ctx: &Ctx, cmd: MarketplaceCommand) -> Result<()> {
    let api = client(ctx)?;
    match cmd {
        MarketplaceCommand::List => {
            let list: Vec<Value> = api.get("/marketplace/listings", &[]).await?;
            if ctx.json() {
                output::print_json(&list);
                return Ok(());
            }
            let mut t = output::table(ctx, &["SLUG", "TITLE", "BADGES", "PROVIDERS", "TAGS", "RESOURCES"]);
            let join = |v: &Value| {
                v.as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default()
            };
            for l in &list {
                t.add_row(vec![
                    comfy_table::Cell::new(text(l, "slug")),
                    comfy_table::Cell::new(text(l, "title")),
                    comfy_table::Cell::new(join(&l["badges"])),
                    comfy_table::Cell::new(join(&l["providers"])),
                    comfy_table::Cell::new(join(&l["tags"])),
                    comfy_table::Cell::new(l["resourceCount"].to_string()),
                ]);
            }
            crate::outln!("{t}");
            eprintln!("Start a project from one with `ork init --from <slug>`.");
        }
        MarketplaceCommand::Show { slug } => {
            let l: Value = api.get(&format!("/marketplace/listings/{slug}"), &[]).await?;
            if ctx.json() {
                output::print_json(&l);
                return Ok(());
            }
            crate::outln!("{} {} ({})", heading(ctx, "Listing:"), text(&l, "title"), text(&l, "slug"));
            crate::outln!("{} {}@{}", heading(ctx, "Template:"), text(&l, "templateName"), text(&l, "version"));
            crate::outln!("{}", text(&l, "description").trim());
            for p in l["template"]["placements"].as_array().into_iter().flatten() {
                let providers = p["providers"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                crate::outln!("{} {} ({})", heading(ctx, "Slot:"), text(p, "name"), providers);
            }
            for r in l["template"]["resources"].as_array().into_iter().flatten() {
                crate::outln!("  {} {} on {}", text(r, "id"), text(r, "type"), text(r, "placement"));
            }
            if let Some(total) = l["cost"]["total"].as_f64() {
                crate::outln!("{} about {total:.2} {} a month", heading(ctx, "Cost:"), text(&l["cost"], "currency"));
            }
            eprintln!("Start a project from it with `ork init --from {slug}`.");
        }
    }
    Ok(())
}

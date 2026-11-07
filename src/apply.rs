//! Server-side apply of one file's YAML documents, after
//! <https://github.com/kube-rs/kube/blob/main/examples/kubectl.rs>.

use crate::Args;
use anyhow::{Context, Result};
use kube::{
    Api, Client, Discovery, ResourceExt,
    api::{Patch, PatchParams},
    core::{DynamicObject, GroupVersionKind},
    discovery::Scope,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use tracing::{debug, trace, warn};

/// What happened to one file's documents.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Documents that failed (including unknown kinds).
    pub failures: i64,
    /// Documents whose kind discovery didn't know, e.g. a CRD applied earlier
    /// in the same run. Worth a retry with fresh discovery.
    pub unknown_kinds: i64,
}

/// `key=value` matches that value; a bare `key` matches presence.
fn matches_filter(map: Option<&BTreeMap<String, String>>, filter: &str) -> bool {
    let Some(m) = map else { return false };
    match filter.split_once('=') {
        Some((key, val)) => m.get(key).is_some_and(|v| v == val),
        None => m.contains_key(filter),
    }
}

pub async fn file(
    client: &Client,
    discovery: &Discovery,
    path: &str,
    args: &Args,
) -> Result<Outcome> {
    let mut out = Outcome::default();
    let params = PatchParams::apply(&args.user_agent).force();
    let yaml = std::fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    for de in serde_yaml::Deserializer::from_str(&yaml) {
        let doc = serde_yaml::Value::deserialize(de)
            .with_context(|| format!("invalid YAML in {path}"))?;
        if doc.is_null() {
            continue; // empty document, e.g. a trailing `---`
        }
        // never log the object itself: it may be a Secret
        let obj: DynamicObject = match serde_yaml::from_value(doc) {
            Ok(obj) => obj,
            Err(e) => {
                warn!("{path}: invalid document: {e}");
                out.failures += 1;
                continue;
            }
        };
        let name = obj.name_any();
        let Some(gvk) = obj
            .types
            .as_ref()
            .and_then(|tm| GroupVersionKind::try_from(tm).ok())
        else {
            warn!("{path}: {name} has no valid apiVersion/kind");
            out.failures += 1;
            continue;
        };

        if let Some(f) = &args.filter_annotation
            && !matches_filter(obj.metadata.annotations.as_ref(), f)
        {
            trace!("{path}: skipping {} {name} (annotation filter)", gvk.kind);
            continue;
        }
        if let Some(f) = &args.filter_label
            && !matches_filter(obj.metadata.labels.as_ref(), f)
        {
            trace!("{path}: skipping {} {name} (label filter)", gvk.kind);
            continue;
        }

        let Some((ar, caps)) = discovery.resolve_gvk(&gvk) else {
            warn!(
                "{path}: {} {name}: unknown kind {}/{}",
                gvk.kind, gvk.group, gvk.version
            );
            out.failures += 1;
            out.unknown_kinds += 1;
            continue;
        };
        let api: Api<DynamicObject> = match (caps.scope, obj.metadata.namespace.as_deref()) {
            (Scope::Cluster, _) => Api::all_with(client.clone(), &ar),
            (_, Some(ns)) => Api::namespaced_with(client.clone(), ns, &ar),
            (_, None) => Api::default_namespaced_with(client.clone(), &ar),
        };
        match api.patch(&name, &params, &Patch::Apply(&obj)).await {
            Ok(_) => debug!("{path}: applied {} {name}", gvk.kind),
            Err(e) => {
                warn!("{path}: applying {} {name} failed: {e}", gvk.kind);
                out.failures += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::matches_filter;
    use std::collections::BTreeMap;

    #[test]
    fn filter_by_presence_or_value() {
        let m = BTreeMap::from([("app".to_string(), "web".to_string())]);
        assert!(matches_filter(Some(&m), "app"));
        assert!(matches_filter(Some(&m), "app=web"));
        assert!(!matches_filter(Some(&m), "app=db"));
        assert!(!matches_filter(Some(&m), "tier"));
        assert!(!matches_filter(None, "app"));
    }
}

use anyhow::{Context, Result};
use kube::{
    api::{Patch, PatchParams},
    core::{DynamicObject, GroupVersionKind},
    discovery::{ApiCapabilities, ApiResource, Scope},
    Api, Client, Discovery, ResourceExt,
};
use log::{info, trace, warn};

pub async fn run_discovery(client: Client) -> Result<Discovery> {
    Ok(Discovery::new(client).run().await?)
}

// https://github.com/kube-rs/kube/blob/main/examples/kubectl.rs#L249
fn multidoc_deserialize(data: &str) -> Result<Vec<serde_yaml::Value>> {
    use serde::Deserialize;
    let mut docs = vec![];
    for de in serde_yaml::Deserializer::from_str(data) {
        docs.push(serde_yaml::Value::deserialize(de)?);
    }
    Ok(docs)
}

// https://github.com/kube-rs/kube/blob/main/examples/kubectl.rs#L224C16-L224C16
fn dynamic_api(
    ar: ApiResource,
    caps: ApiCapabilities,
    client: Client,
    ns: Option<&str>,
    all: bool,
) -> Api<DynamicObject> {
    if caps.scope == Scope::Cluster || all {
        Api::all_with(client, &ar)
    } else if let Some(namespace) = ns {
        Api::namespaced_with(client, namespace, &ar)
    } else {
        Api::default_namespaced_with(client, &ar)
    }
}

fn metadata_filter(map: Option<&std::collections::BTreeMap<String, String>>, filter: &str) -> bool {
    let m = match map {
        Some(m) => m,
        None => return false,
    };
    match filter.split_once('=') {
        Some((key, val)) => m.get(key).map_or(false, |v| v == val),
        None => m.contains_key(filter),
    }
}

// https://github.com/kube-rs/kube/blob/main/examples/kubectl.rs#L156
pub async fn apply(
    client: Client,
    discovery: &Discovery,
    path: &str,
    user_agent: &str,
    filter_annotation: Option<&str>,
    filter_label: Option<&str>,
) -> Result<i64> {
    let mut failures = 0;
    let ssapply = PatchParams::apply(user_agent).force();
    let yaml = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path))?;
    for doc in multidoc_deserialize(&yaml)? {
        let obj: DynamicObject = match serde_yaml::from_value(doc) {
            Ok(obj) => obj,
            Err(e) => {
                warn!("error deserializing document in {}: {}", path, e);
                failures += 1;
                continue;
            }
        };
        let namespace = obj.metadata.namespace.as_deref();
        let gvk = match &obj.types {
            Some(tm) => match GroupVersionKind::try_from(tm) {
                Ok(gvk) => gvk,
                Err(e) => {
                    warn!("error resolving GVK for {}: {:?}: {}", path, obj, e);
                    failures += 1;
                    continue;
                }
            },
            None => {
                warn!("cannot apply object without valid TypeMeta {}: {:?}", path, obj);
                failures += 1;
                continue;
            }
        };
        let name = obj.name_any();

        if let Some(filter) = filter_annotation {
            if !metadata_filter(obj.metadata.annotations.as_ref(), filter) {
                trace!("skipping {}: {} {} (annotation filter)", path, gvk.kind, name);
                continue;
            }
        }
        if let Some(filter) = filter_label {
            if !metadata_filter(obj.metadata.labels.as_ref(), filter) {
                trace!("skipping {}: {} {} (label filter)", path, gvk.kind, name);
                continue;
            }
        }

        if let Some((ar, caps)) = discovery.resolve_gvk(&gvk) {
            let api = dynamic_api(ar, caps, client.clone(), namespace, false);
            let data: serde_json::Value = match serde_json::to_value(&obj) {
                Ok(data) => data,
                Err(e) => {
                    warn!("error serializing {}: {} {}: {}", path, gvk.kind, name, e);
                    failures += 1;
                    continue;
                }
            };
            if let Err(e) = api.patch(&name, &ssapply, &Patch::Apply(data)).await {
                warn!("error during apply {}: {} {}: {}", path, gvk.kind, name, e);
                failures += 1;
            } else {
                info!("applied {}: {} {}", path, gvk.kind, name);
            }
        } else {
            warn!("cannot apply document for unknown {:?}", gvk);
            failures += 1;
        }
    }
    Ok(failures)
}

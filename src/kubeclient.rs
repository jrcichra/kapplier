use anyhow::{bail, Context, Result};
use clap::Parser;
use kube::{
    api::{Patch, PatchParams},
    core::{DynamicObject, GroupVersionKind},
    discovery::{ApiCapabilities, ApiResource, Scope},
    Api, Client, Discovery, ResourceExt,
};
use log::{info, trace, warn};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "kapplier")]
    user_agent: String,
    #[clap(long, default_value = ".")]
    directory: String,
    #[clap(long, default_values = [".git"])]
    ignore_directories: Vec<String>,
    #[clap(long, default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
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

// https://github.com/kube-rs/kube/blob/main/examples/kubectl.rs#L156
pub async fn apply(
    client: Client,
    discovery: &Discovery,
    path: &str,
    user_agent: &str,
) -> Result<()> {
    let ssapply = PatchParams::apply(user_agent).force();
    let yaml = std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path))?;
    for doc in multidoc_deserialize(&yaml)? {
        let obj: DynamicObject = serde_yaml::from_value(doc)?;
        let namespace = obj.metadata.namespace.as_deref();
        let gvk = if let Some(tm) = &obj.types {
            GroupVersionKind::try_from(tm)?
        } else {
            bail!("Cannot apply object without valid TypeMeta {:?}", obj);
        };
        let name = obj.name_any();
        if let Some((ar, caps)) = discovery.resolve_gvk(&gvk) {
            let api = dynamic_api(ar, caps, client.clone(), namespace, false);
            trace!("Applying {}: \n{}", gvk.kind, serde_yaml::to_string(&obj)?);
            let data: serde_json::Value = serde_json::to_value(&obj)?;
            let _r = api.patch(&name, &ssapply, &Patch::Apply(data)).await?;
            info!("Applied {} {}", gvk.kind, name);
        } else {
            warn!("Cannot apply document for unknown {:?}", gvk);
        }
    }
    Ok(())
}

use anyhow::{bail, Context, Result};
use kube::{
    api::{Patch, PatchParams},
    core::{DynamicObject, GroupVersionKind},
    discovery::{ApiCapabilities, ApiResource, Scope},
    Api, Client, Discovery, ResourceExt,
};
use log::{info, warn};

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
pub async fn apply(client: Client, path: &str, user_agent: &str) -> Result<()> {
    let ssapply = PatchParams::apply(user_agent).force();
    let yaml = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path))?;
    let discovery = Discovery::new(client.clone()).run().await?;
    for doc in multidoc_deserialize(&yaml)? {
        let obj: DynamicObject = serde_yaml::from_value(doc)?;
        let namespace = obj.metadata.namespace.as_deref();
        let gvk = if let Some(tm) = &obj.types {
            GroupVersionKind::try_from(tm)?
        } else {
            bail!("cannot apply object without valid TypeMeta {:?}", obj);
        };
        let name = obj.name_any();
        if let Some((ar, caps)) = discovery.resolve_gvk(&gvk) {
            let api = dynamic_api(ar, caps, client.clone(), namespace, false);
            let data: serde_json::Value = serde_json::to_value(&obj)?;
            let _r = api.patch(&name, &ssapply, &Patch::Apply(data)).await?;
            info!("applied {}: {} {}", path, gvk.kind, name);
        } else {
            warn!("cannot apply document for unknown {:?}", gvk);
        }
    }
    Ok(())
}

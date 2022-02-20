use std::collections::HashMap;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, CustomResource, PartialEq, JsonSchema, Default)]
#[kube(
    group = "api.sherlockholo.io",
    version = "v1",
    kind = "Ddns",
    plural = "ddnss",
    shortname = "dd",
    status = "DdnsStatus",
    namespaced,
    derive = "Default",
    printcolumn = r#"{"name":"DOMAIN", "type":"string", "jsonPath":".spec.domain"}"#,
    printcolumn = r#"{"name":"AGE", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    printcolumn = r#"{"name":"STATUS", "type":"string", "jsonPath":".status.status"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct DdnsSpec {
    pub selector: HashMap<String, String>,
    pub domain: String,
    pub zone: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct DdnsStatus {
    pub status: String,
    pub selector: HashMap<String, String>,
    pub domain: String,
    pub zone: String,
}

impl DdnsStatus {
    pub fn to_patch_status(&self) -> PatchStatus {
        self.clone().into()
    }
}

#[derive(Debug, Serialize)]
pub struct PatchStatus {
    status: DdnsStatus,
}

impl From<DdnsStatus> for PatchStatus {
    fn from(status: DdnsStatus) -> Self {
        Self { status }
    }
}

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, CustomResource, PartialEq, JsonSchema, Default)]
#[kube(
    group = "api.sherlockholo.xyz",
    version = "v1",
    kind = "Ddns",
    plural = "ddnss",
    shortname = "dd",
    status = "DdnsStatus",
    namespaced,
    derive = "Default",
    printcolumn = r#"{"name":"SERVICENAME", "type":"string", "jsonPath":".spec.serviceName"}"#,
    printcolumn = r#"{"name":"DOMAINNAME", "type":"string", "jsonPath":".spec.domainName"}"#,
    printcolumn = r#"{"name":"AGE", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    printcolumn = r#"{"name":"STATUS", "type":"string", "jsonPath":".status.status"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct DdnsSpec {
    pub service_name: String,
    pub domain_name: String,
    pub domain: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct DdnsStatus {
    pub status: String,
    pub service_name: String,
    pub domain_name: String,
    pub domain: String,
}

impl DdnsStatus {
    pub fn to_patch_status(&self) -> PatchStatus {
        PatchStatus::from(self.clone())
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

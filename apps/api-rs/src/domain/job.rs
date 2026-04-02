use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub cluster: String,
    pub namespace: String,
    pub kind: String,
    pub resource_name: String,
    pub job_name: String,
    pub status: String,
    pub health: String,
    pub raw_status: String,
    pub flink_version: Option<String>,
    pub deployment_mode: Option<String>,
    pub last_updated_at: Option<String>,
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flink_job_id: Option<String>,
    pub native_ui_url: Option<String>,
    pub warnings: Vec<String>,
    pub details: Value,
}

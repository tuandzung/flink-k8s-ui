use serde::{Deserialize, Serialize};

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
    pub details: JobDetails,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_summary: Option<JobStatusSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flink_rest_overview: Option<FlinkRestOverview>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatusSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl JobStatusSummary {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlinkRestOverview {
    pub job_id: String,
    pub job_name: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

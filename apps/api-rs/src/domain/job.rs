use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobAction {
    Cancel,
    Suspend,
    Resume,
}

impl JobAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
        }
    }
}

impl FromStr for JobAction {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "cancel" => Ok(Self::Cancel),
            "suspend" => Ok(Self::Suspend),
            "resume" => Ok(Self::Resume),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobActionState {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobActions {
    pub cancel: JobActionState,
    pub suspend: JobActionState,
    pub resume: JobActionState,
}

impl JobActions {
    pub fn for_status(status: &str) -> Self {
        match status {
            "suspended" => Self {
                cancel: enabled_action(),
                suspend: disabled_action("Already suspended"),
                resume: enabled_action(),
            },
            "running" | "reconciling" => Self {
                cancel: enabled_action(),
                suspend: enabled_action(),
                resume: disabled_action("Resume is only available for suspended resources"),
            },
            "failed" | "unknown" => Self {
                cancel: enabled_action(),
                suspend: disabled_action("Suspend is only available for active resources"),
                resume: disabled_action("Resume is only available for suspended resources"),
            },
            _ => Self {
                cancel: enabled_action(),
                suspend: disabled_action("Suspend is not available for this resource state"),
                resume: disabled_action("Resume is not available for this resource state"),
            },
        }
    }

    pub fn disable_all(mut self, reason: &str) -> Self {
        for state in [&mut self.cancel, &mut self.suspend, &mut self.resume] {
            state.enabled = false;
            state.reason = Some(reason.to_owned());
        }
        self
    }

    pub fn state_for(&self, action: JobAction) -> &JobActionState {
        match action {
            JobAction::Cancel => &self.cancel,
            JobAction::Suspend => &self.suspend,
            JobAction::Resume => &self.resume,
        }
    }
}

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
    #[serde(default)]
    pub actions: JobActions,
    pub details: JobDetails,
}

impl Job {
    pub fn derive_actions(&mut self) {
        self.actions = JobActions::for_status(&self.status);
    }

    pub fn disable_actions(&mut self, reason: &str) {
        self.actions = self.actions.clone().disable_all(reason);
    }
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

fn enabled_action() -> JobActionState {
    JobActionState {
        enabled: true,
        reason: None,
    }
}

fn disabled_action(reason: &str) -> JobActionState {
    JobActionState {
        enabled: false,
        reason: Some(reason.to_owned()),
    }
}

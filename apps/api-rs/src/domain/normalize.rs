use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::config::ClusterConfig;
use crate::domain::job::{Job, JobDetails, JobStatusSummary};

pub fn normalize_flink_resource(resource: Value, cluster: &ClusterConfig) -> Job {
    let kind = get_string(&resource, &["kind"]).unwrap_or_else(|| "FlinkDeployment".to_owned());

    if kind == "FlinkSessionJob" {
        normalize_flink_session_job(resource, cluster)
    } else {
        normalize_flink_deployment(resource, cluster)
    }
}

pub fn normalize_flink_deployment(resource: Value, cluster: &ClusterConfig) -> Job {
    let job_name = first_defined(
        &resource,
        &[
            &["spec", "job", "name"],
            &["spec", "job", "entryClass"],
            &["metadata", "labels", "app.kubernetes.io/name"],
            &["metadata", "name"],
        ],
    )
    .unwrap_or_else(|| "unknown".to_owned());

    let raw_status_values = collect_defined_strings(
        &resource,
        &[
            &["status", "jobStatus", "state"],
            &["status", "lifecycleState"],
            &["status", "jobManagerDeploymentStatus"],
            &["status", "reconciliationStatus", "state"],
            &["spec", "job", "state"],
        ],
    );
    let native_ui_url = first_defined(
        &resource,
        &[
            &["status", "jobManagerUrl"],
            &["status", "jobManagerInfo", "url"],
        ],
    );

    normalize_base(
        resource,
        cluster,
        "FlinkDeployment",
        job_name,
        raw_status_values,
        native_ui_url,
    )
}

pub fn normalize_flink_session_job(resource: Value, cluster: &ClusterConfig) -> Job {
    let job_name = first_defined(
        &resource,
        &[
            &["spec", "job", "name"],
            &["spec", "job", "jarURI"],
            &["metadata", "name"],
        ],
    )
    .unwrap_or_else(|| "unknown".to_owned());
    let raw_status_values = collect_defined_strings(
        &resource,
        &[
            &["status", "jobStatus", "state"],
            &["status", "lifecycleState"],
            &["status", "reconciliationStatus", "state"],
            &["spec", "job", "state"],
        ],
    );
    let native_ui_url = first_defined(&resource, &[&["status", "jobManagerUrl"]]);

    normalize_base(
        resource,
        cluster,
        "FlinkSessionJob",
        job_name,
        raw_status_values,
        native_ui_url,
    )
}

fn normalize_base(
    resource: Value,
    cluster: &ClusterConfig,
    kind: &str,
    job_name: String,
    raw_status_values: Vec<String>,
    native_ui_url: Option<String>,
) -> Job {
    let metadata = resource
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let spec = resource
        .get("spec")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let status = resource
        .get("status")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let status_info = canonicalize_status(raw_status_values);
    let namespace =
        value_to_string(metadata.pointer("/namespace")).unwrap_or_else(|| "default".to_owned());
    let resource_name =
        value_to_string(metadata.pointer("/name")).unwrap_or_else(|| job_name.clone());

    let status_summary = JobStatusSummary {
        job_state: value_to_string(status.pointer("/jobStatus/state")),
        lifecycle_state: value_to_string(status.pointer("/lifecycleState")),
        reconciliation_state: value_to_string(status.pointer("/reconciliationStatus/state")),
        last_reconciled_at: status
            .pointer("/reconciliationStatus/lastReconciledAt")
            .and_then(to_iso_or_null),
        error: first_defined_value(&[
            status.pointer("/error"),
            status.pointer("/jobStatus/error"),
            status.pointer("/errorMessage"),
            status.pointer("/reconciliationStatus/error"),
        ])
        .and_then(|value| value_to_string(Some(value))),
    };

    Job {
        id: format!("{}:{}:{}:{}", cluster.name, namespace, kind, resource_name),
        cluster: cluster.name.clone(),
        namespace,
        kind: kind.to_owned(),
        resource_name: resource_name.clone(),
        job_name,
        status: status_info.status,
        health: status_info.health,
        raw_status: status_info.raw_status,
        flink_version: first_defined_value(&[spec.pointer("/flinkVersion")])
            .and_then(|value| value_to_string(Some(value))),
        deployment_mode: first_defined_value(&[
            spec.pointer("/mode"),
            spec.pointer("/deploymentMode"),
            spec.pointer("/job/upgradeMode"),
        ])
        .and_then(|value| value_to_string(Some(value))),
        last_updated_at: first_defined_value(&[
            status.pointer("/reconciliationStatus/lastReconciledAt"),
            status.pointer("/jobStatus/updateTime"),
            metadata.pointer("/creationTimestamp"),
        ])
        .and_then(to_iso_or_null),
        started_at: first_defined_value(&[
            status.pointer("/jobStatus/startTime"),
            status.pointer("/jobManagerDeploymentStatus/startTime"),
            metadata.pointer("/creationTimestamp"),
        ])
        .and_then(to_iso_or_null),
        flink_job_id: None,
        native_ui_url,
        warnings: build_warnings(&status),
        details: JobDetails {
            status_summary: (!status_summary.is_empty()).then_some(status_summary),
            flink_rest_overview: None,
        },
    }
}

struct StatusInfo {
    status: String,
    health: String,
    raw_status: String,
}

fn canonicalize_status(raw_values: Vec<String>) -> StatusInfo {
    let raw = raw_values.join(" / ");
    let normalized = raw.to_ascii_lowercase();

    if normalized.is_empty() {
        return StatusInfo {
            status: "unknown".to_owned(),
            health: "unknown".to_owned(),
            raw_status: "Unknown".to_owned(),
        };
    }

    if ["error", "fail", "missing"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return StatusInfo {
            status: "failed".to_owned(),
            health: "error".to_owned(),
            raw_status: raw,
        };
    }

    if normalized.contains("suspend") {
        return StatusInfo {
            status: "suspended".to_owned(),
            health: "warning".to_owned(),
            raw_status: raw,
        };
    }

    if ["reconcil", "deploy", "progress", "upgrad"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return StatusInfo {
            status: "reconciling".to_owned(),
            health: "warning".to_owned(),
            raw_status: raw,
        };
    }

    if ["run", "ready", "stable"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return StatusInfo {
            status: "running".to_owned(),
            health: "healthy".to_owned(),
            raw_status: raw,
        };
    }

    StatusInfo {
        status: "unknown".to_owned(),
        health: "unknown".to_owned(),
        raw_status: raw,
    }
}

fn build_warnings(status: &Value) -> Vec<String> {
    unique(
        [
            status.pointer("/error"),
            status.pointer("/jobStatus/error"),
            status.pointer("/errorMessage"),
            status.pointer("/reconciliationStatus/error"),
        ]
        .into_iter()
        .filter_map(value_to_string)
        .collect(),
    )
}

fn unique(values: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}

fn collect_defined_strings(resource: &Value, paths: &[&[&str]]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| get_string(resource, path))
        .collect()
}

fn first_defined(resource: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| get_string(resource, path))
}

fn get_string(resource: &Value, path: &[&str]) -> Option<String> {
    let pointer = to_pointer(path);
    value_to_string(resource.pointer(&pointer))
}

fn first_defined_value<'a>(values: &[Option<&'a Value>]) -> Option<&'a Value> {
    values
        .iter()
        .copied()
        .flatten()
        .find(|value| !value.is_null())
}

fn to_pointer(path: &[&str]) -> String {
    let mut pointer = String::new();
    for segment in path {
        pointer.push('/');
        pointer.push_str(segment);
    }
    pointer
}

fn value_to_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn to_iso_or_null(value: &Value) -> Option<String> {
    let Value::String(text) = value else {
        return None;
    };
    OffsetDateTime::parse(text, &Rfc3339)
        .ok()
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
}

#[cfg(test)]
mod tests {
    use super::{normalize_flink_deployment, normalize_flink_session_job};
    use crate::config::ClusterConfig;
    use serde_json::json;

    fn cluster() -> ClusterConfig {
        ClusterConfig {
            name: "demo".to_owned(),
            api_url: "https://kubernetes.default.svc".to_owned(),
            bearer_token: "token".to_owned(),
            ca_cert: None,
            insecure_skip_tls_verify: false,
            namespaces: vec!["analytics".to_owned()],
            flink_api_version: "v1beta1".to_owned(),
            flink_rest_base_url: None,
        }
    }

    #[test]
    fn normalize_flink_deployment_maps_running_state_to_healthy_status() {
        let resource = json!({
            "kind": "FlinkDeployment",
            "metadata": {
                "name": "orders",
                "namespace": "analytics",
                "creationTimestamp": "2026-04-01T00:00:00Z",
                "labels": {
                    "app.kubernetes.io/name": "orders-stream"
                }
            },
            "spec": {
                "flinkVersion": "1.19",
                "mode": "native",
                "job": {
                    "name": "orders-job"
                }
            },
            "status": {
                "jobStatus": { "state": "RUNNING" },
                "reconciliationStatus": {
                    "state": "READY",
                    "lastReconciledAt": "2026-04-02T00:00:00Z"
                },
                "jobManagerUrl": "https://example/jobs/orders"
            }
        });

        let normalized = normalize_flink_deployment(resource, &cluster());

        assert_eq!(normalized.status, "running");
        assert_eq!(normalized.health, "healthy");
        assert_eq!(normalized.cluster, "demo");
        assert_eq!(normalized.namespace, "analytics");
        assert_eq!(normalized.job_name, "orders-job");
        assert_eq!(
            normalized
                .details
                .status_summary
                .as_ref()
                .and_then(|summary| summary.job_state.as_deref()),
            Some("RUNNING")
        );
        assert!(normalized.details.flink_rest_overview.is_none());
        assert_eq!(
            normalized.last_updated_at.as_deref(),
            Some("2026-04-02T00:00:00Z")
        );
    }

    #[test]
    fn normalize_flink_session_job_maps_suspended_state_to_warning_health() {
        let resource = json!({
            "kind": "FlinkSessionJob",
            "metadata": {
                "name": "settlement",
                "namespace": "payments",
                "creationTimestamp": "2026-04-01T00:00:00Z"
            },
            "spec": {
                "job": {
                    "name": "settlement-job",
                    "state": "SUSPENDED"
                }
            },
            "status": {
                "jobStatus": { "state": "SUSPENDED" }
            }
        });

        let normalized = normalize_flink_session_job(resource, &cluster());

        assert_eq!(normalized.status, "suspended");
        assert_eq!(normalized.health, "warning");
        assert_eq!(normalized.job_name, "settlement-job");
        assert_eq!(
            normalized
                .details
                .status_summary
                .as_ref()
                .and_then(|summary| summary.job_state.as_deref()),
            Some("SUSPENDED")
        );
    }
}

use std::time::Duration;

use reqwest::{Client, Url};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::domain::job::{FlinkRestOverview, Job};

pub async fn enrich_jobs(jobs: Vec<Job>, request_timeout_ms: u64) -> Vec<Job> {
    let client = Client::builder()
        .timeout(Duration::from_millis(request_timeout_ms))
        .build();

    match client {
        Ok(client) => {
            let mut enriched = Vec::with_capacity(jobs.len());
            for job in jobs {
                enriched.push(enrich_job(&client, job).await);
            }
            enriched
        }
        Err(error) => jobs
            .into_iter()
            .map(|job| with_warning(job, format!("Flink REST enrichment failed: {}", error)))
            .collect(),
    }
}

async fn enrich_job(client: &Client, job: Job) -> Job {
    let Some(native_ui_url) = &job.native_ui_url else {
        return with_warning(
            job,
            "Flink REST enrichment unavailable: no JobManager URL".to_owned(),
        );
    };

    let overview_url = match Url::parse(native_ui_url).and_then(|url| url.join("/jobs/overview")) {
        Ok(url) => url,
        Err(error) => {
            return with_warning(job, format!("Flink REST enrichment failed: {}", error));
        }
    };

    match request_json(client, overview_url).await {
        Ok(payload) => merge_overview(job, payload),
        Err(error) => with_warning(job, format!("Flink REST enrichment failed: {}", error)),
    }
}

async fn request_json(client: &Client, url: Url) -> Result<Value, String> {
    let response = client
        .get(url.clone())
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if status.is_client_error() || status.is_server_error() {
        return Err(format!(
            "Flink REST {}: {}",
            status.as_u16(),
            body.chars().take(200).collect::<String>()
        ));
    }

    serde_json::from_str(&body).map_err(|error| format!("Invalid Flink REST payload: {}", error))
}

fn merge_overview(mut job: Job, payload: Value) -> Job {
    let candidate = payload
        .get("jobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| {
            item.get("name").and_then(Value::as_str) == Some(job.job_name.as_str())
                || item
                    .get("state")
                    .and_then(Value::as_str)
                    .map(|state| state.eq_ignore_ascii_case(&job.status))
                    .unwrap_or(false)
        })
        .cloned();

    let Some(candidate) = candidate else {
        return with_warning(
            job,
            "Flink REST enrichment returned no matching job".to_owned(),
        );
    };

    job.flink_job_id = candidate
        .get("jid")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    job.started_at = candidate
        .get("start-time")
        .and_then(Value::as_i64)
        .and_then(timestamp_millis_to_rfc3339)
        .or(job.started_at);
    job.raw_status = candidate
        .get("state")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or(job.raw_status);
    job.details.flink_rest_overview = Some(FlinkRestOverview {
        job_id: candidate
            .get("jid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        job_name: candidate
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(job.job_name.as_str())
            .to_owned(),
        state: candidate
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or(job.raw_status.as_str())
            .to_owned(),
        started_at: candidate
            .get("start-time")
            .and_then(Value::as_i64)
            .and_then(timestamp_millis_to_rfc3339),
    });
    job
}

fn with_warning(mut job: Job, warning: String) -> Job {
    job.warnings.push(warning);
    job
}

fn timestamp_millis_to_rfc3339(timestamp_ms: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp_nanos((timestamp_ms as i128) * 1_000_000)
        .ok()
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::extract::OriginalUri;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use crate::domain::job::JobDetails;

    #[tokio::test]
    async fn enrich_jobs_warns_when_no_jobmanager_url_exists() {
        let jobs = enrich_jobs(
            vec![Job {
                id: "demo:analytics:FlinkDeployment:orders-stream".to_owned(),
                cluster: "demo".to_owned(),
                namespace: "analytics".to_owned(),
                kind: "FlinkDeployment".to_owned(),
                resource_name: "orders-stream".to_owned(),
                job_name: "orders-stream".to_owned(),
                status: "running".to_owned(),
                health: "healthy".to_owned(),
                raw_status: "RUNNING".to_owned(),
                flink_version: None,
                deployment_mode: None,
                last_updated_at: None,
                started_at: None,
                flink_job_id: None,
                native_ui_url: None,
                warnings: Vec::new(),
                details: JobDetails::default(),
            }],
            500,
        )
        .await;

        assert_eq!(
            jobs[0].warnings,
            vec!["Flink REST enrichment unavailable: no JobManager URL".to_owned()]
        );
    }

    #[tokio::test]
    async fn enrich_jobs_merges_matching_job_overview() {
        let mock = start_mock_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({
              "jobs": [{
                "jid": "job-123",
                "name": "orders-stream",
                "state": "RUNNING",
                "start-time": 1770000000000_i64
              }]
            }),
        )])
        .await;

        let jobs = enrich_jobs(
            vec![Job {
                id: "demo:analytics:FlinkDeployment:orders-stream".to_owned(),
                cluster: "demo".to_owned(),
                namespace: "analytics".to_owned(),
                kind: "FlinkDeployment".to_owned(),
                resource_name: "orders-stream".to_owned(),
                job_name: "orders-stream".to_owned(),
                status: "running".to_owned(),
                health: "healthy".to_owned(),
                raw_status: "READY".to_owned(),
                flink_version: None,
                deployment_mode: None,
                last_updated_at: None,
                started_at: Some("2026-04-01T00:00:00Z".to_owned()),
                flink_job_id: None,
                native_ui_url: Some(format!("{}/orders-stream/", mock.base_url)),
                warnings: Vec::new(),
                details: JobDetails::default(),
            }],
            500,
        )
        .await;

        assert_eq!(jobs[0].flink_job_id.as_deref(), Some("job-123"));
        assert_eq!(jobs[0].raw_status, "RUNNING");
        assert!(
            jobs[0]
                .details
                .flink_rest_overview
                .as_ref()
                .map(|overview| {
                    (
                        overview.job_id.as_str(),
                        overview.job_name.as_str(),
                        overview.state.as_str(),
                    )
                })
                == Some(("job-123", "orders-stream", "RUNNING"))
        );

        mock.shutdown();
    }

    #[tokio::test]
    async fn enrich_jobs_warns_when_no_matching_job_is_returned() {
        let mock = start_mock_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({
              "jobs": [{
                "jid": "job-999",
                "name": "another-job",
                "state": "FAILED"
              }]
            }),
        )])
        .await;

        let jobs = enrich_jobs(
            vec![Job {
                id: "demo:analytics:FlinkDeployment:orders-stream".to_owned(),
                cluster: "demo".to_owned(),
                namespace: "analytics".to_owned(),
                kind: "FlinkDeployment".to_owned(),
                resource_name: "orders-stream".to_owned(),
                job_name: "orders-stream".to_owned(),
                status: "running".to_owned(),
                health: "healthy".to_owned(),
                raw_status: "READY".to_owned(),
                flink_version: None,
                deployment_mode: None,
                last_updated_at: None,
                started_at: None,
                flink_job_id: None,
                native_ui_url: Some(format!("{}/orders-stream/", mock.base_url)),
                warnings: Vec::new(),
                details: JobDetails::default(),
            }],
            500,
        )
        .await;

        assert_eq!(
            jobs[0].warnings,
            vec!["Flink REST enrichment returned no matching job".to_owned()]
        );

        mock.shutdown();
    }

    #[tokio::test]
    async fn enrich_jobs_warns_when_flink_rest_returns_an_error() {
        let mock = start_mock_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
              "error": "upstream unavailable"
            }),
        )])
        .await;

        let jobs = enrich_jobs(
            vec![Job {
                id: "demo:analytics:FlinkDeployment:orders-stream".to_owned(),
                cluster: "demo".to_owned(),
                namespace: "analytics".to_owned(),
                kind: "FlinkDeployment".to_owned(),
                resource_name: "orders-stream".to_owned(),
                job_name: "orders-stream".to_owned(),
                status: "running".to_owned(),
                health: "healthy".to_owned(),
                raw_status: "READY".to_owned(),
                flink_version: None,
                deployment_mode: None,
                last_updated_at: None,
                started_at: None,
                flink_job_id: None,
                native_ui_url: Some(format!("{}/orders-stream/", mock.base_url)),
                warnings: Vec::new(),
                details: JobDetails::default(),
            }],
            500,
        )
        .await;

        assert!(jobs[0].warnings[0].starts_with("Flink REST enrichment failed: Flink REST 503:"));

        mock.shutdown();
    }

    struct MockServer {
        base_url: String,
        task: JoinHandle<()>,
    }

    impl MockServer {
        fn shutdown(self) {
            self.task.abort();
        }
    }

    async fn start_mock_server(responses: Vec<(String, StatusCode, Value)>) -> MockServer {
        let shared = Arc::new(responses);
        let app = Router::new().fallback(get({
            let shared = Arc::clone(&shared);
            move |uri: OriginalUri| {
                let shared = Arc::clone(&shared);
                async move {
                    let path = uri.path().to_owned();
                    let maybe = shared.iter().find(|(candidate, _, _)| candidate == &path);
                    match maybe {
                        Some((_, status, payload)) => {
                            (*status, Json(payload.clone())).into_response()
                        }
                        None => (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})))
                            .into_response(),
                    }
                }
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should run");
        });

        MockServer {
            base_url: format!("http://{}", address),
            task,
        }
    }
}

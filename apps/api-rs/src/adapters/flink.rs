use std::time::Duration;

use reqwest::{Client, Url};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::config::ClusterConfig;
use crate::domain::job::{FlinkRestOverview, Job};

pub async fn enrich_jobs(
    jobs: Vec<Job>,
    cluster: &ClusterConfig,
    request_timeout_ms: u64,
) -> Vec<Job> {
    let client = Client::builder()
        .timeout(Duration::from_millis(request_timeout_ms))
        .build();

    match client {
        Ok(client) => {
            let mut enriched = Vec::with_capacity(jobs.len());
            for job in jobs {
                enriched.push(enrich_job(&client, cluster, job).await);
            }
            enriched
        }
        Err(error) => jobs
            .into_iter()
            .map(|job| with_warning(job, format!("Flink REST enrichment failed: {}", error)))
            .collect(),
    }
}

async fn enrich_job(client: &Client, cluster: &ClusterConfig, job: Job) -> Job {
    let Some(trusted_base_url) = trusted_base_url(cluster, &job) else {
        let warning = missing_trusted_base_warning(cluster, &job);
        return with_warning(job, warning);
    };

    let overview_url = match overview_url(&trusted_base_url) {
        Ok(url) => url,
        Err(error) => return with_warning(job, format!("Flink REST enrichment failed: {}", error)),
    };
    let native_ui_warning = native_ui_url_warning(&trusted_base_url, &job);
    let job = with_native_ui_url_warning(job, native_ui_warning);

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

    let overview_job_id = candidate
        .get("jid")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let overview_job_name = candidate
        .get("name")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let overview_state = candidate
        .get("state")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let overview_started_at = candidate
        .get("start-time")
        .and_then(Value::as_i64)
        .and_then(timestamp_millis_to_rfc3339);

    job.flink_job_id = overview_job_id.clone();
    job.started_at = overview_started_at.clone().or(job.started_at);
    if let Some(state) = overview_state.clone() {
        job.raw_status = state;
    }
    job.details.flink_rest_overview = Some(FlinkRestOverview {
        job_id: overview_job_id.unwrap_or_default(),
        job_name: overview_job_name.unwrap_or_else(|| job.job_name.clone()),
        state: overview_state.unwrap_or_else(|| job.raw_status.clone()),
        started_at: overview_started_at,
    });
    job
}

fn with_warning(mut job: Job, warning: String) -> Job {
    job.warnings.push(warning);
    job
}

fn overview_url(base_url: &Url) -> Result<Url, String> {
    let mut overview_url = base_url.clone();
    let mut segments = overview_url
        .path_segments_mut()
        .map_err(|_| "trusted Flink REST base URL must be a base URL".to_owned())?;
    segments.pop_if_empty();
    segments.push("jobs");
    segments.push("overview");
    drop(segments);
    Ok(overview_url)
}

fn with_native_ui_url_warning(job: Job, warning: Option<String>) -> Job {
    match warning {
        Some(warning) => with_warning(job, warning),
        None => job,
    }
}

fn trusted_base_url(cluster: &ClusterConfig, job: &Job) -> Option<Url> {
    cluster
        .flink_rest_base_url
        .as_deref()
        .and_then(parse_base_url)
        .or_else(|| derived_in_cluster_base_url(cluster, job))
}

fn parse_base_url(value: &str) -> Option<Url> {
    Url::parse(value).ok()
}

fn derived_in_cluster_base_url(cluster: &ClusterConfig, job: &Job) -> Option<Url> {
    if !cluster.derive_jobmanager_url_in_cluster || job.kind != "FlinkDeployment" {
        return None;
    }

    parse_base_url(&format!(
        "http://{}-rest.{}.svc:8081/",
        job.resource_name, job.namespace
    ))
}

fn native_ui_url_warning(trusted_base_url: &Url, job: &Job) -> Option<String> {
    let native_ui_url = job.native_ui_url.as_deref()?;
    match Url::parse(native_ui_url) {
        Ok(url) if url.origin() == trusted_base_url.origin() => None,
        Ok(_) => Some(
            "Flink REST enrichment ignored a JobManager URL outside the trusted origin".to_owned(),
        ),
        Err(_) => Some("Flink REST enrichment ignored an invalid JobManager URL".to_owned()),
    }
}

fn missing_trusted_base_warning(cluster: &ClusterConfig, job: &Job) -> String {
    if job.native_ui_url.is_some() {
        "Flink REST enrichment skipped: no trusted Flink REST base URL is configured".to_owned()
    } else if cluster.derive_jobmanager_url_in_cluster && job.kind == "FlinkSessionJob" {
        "Flink REST enrichment unavailable: no trusted Flink REST base URL for FlinkSessionJob"
            .to_owned()
    } else {
        "Flink REST enrichment unavailable: no trusted Flink REST base URL".to_owned()
    }
}

fn timestamp_millis_to_rfc3339(timestamp_ms: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp_nanos((timestamp_ms as i128) * 1_000_000)
        .ok()
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

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
    async fn enrich_jobs_warns_when_no_trusted_base_url_exists() {
        let jobs = enrich_jobs(vec![test_job(None)], &test_cluster(None), 500).await;

        assert_eq!(
            jobs[0].warnings,
            vec!["Flink REST enrichment unavailable: no trusted Flink REST base URL".to_owned()]
        );
    }

    #[tokio::test]
    async fn enrich_jobs_uses_trusted_configured_origin_instead_of_status_url() {
        let trusted = start_mock_server(vec![(
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
        let untrusted_hits = Arc::new(AtomicUsize::new(0));
        let untrusted = start_counting_server(Arc::clone(&untrusted_hits)).await;

        let jobs = enrich_jobs(
            vec![test_job(Some(format!(
                "{}/orders-stream/",
                untrusted.base_url
            )))],
            &test_cluster(Some(trusted.base_url.clone())),
            500,
        )
        .await;

        assert_eq!(
            jobs[0].warnings,
            vec![
                "Flink REST enrichment ignored a JobManager URL outside the trusted origin"
                    .to_owned()
            ]
        );

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
        assert_eq!(untrusted_hits.load(Ordering::SeqCst), 0);

        trusted.shutdown();
        untrusted.shutdown();
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
            vec![test_job(Some(format!("{}/orders-stream/", mock.base_url)))],
            &test_cluster(Some(mock.base_url.clone())),
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
            vec![test_job(Some(format!("{}/orders-stream/", mock.base_url)))],
            &test_cluster(Some(mock.base_url.clone())),
            500,
        )
        .await;

        assert!(jobs[0].warnings[0].starts_with("Flink REST enrichment failed: Flink REST 503:"));

        mock.shutdown();
    }

    #[tokio::test]
    async fn enrich_jobs_preserves_trusted_base_path_prefixes() {
        let mock = start_mock_server(vec![(
            "/proxy/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({
              "jobs": [{
                "jid": "job-123",
                "name": "orders-stream",
                "state": "RUNNING"
              }]
            }),
        )])
        .await;

        let jobs = enrich_jobs(
            vec![test_job(Some(
                "https://status.example.com/orders-stream/".to_owned(),
            ))],
            &test_cluster(Some(format!("{}/proxy", mock.base_url))),
            500,
        )
        .await;

        assert_eq!(jobs[0].flink_job_id.as_deref(), Some("job-123"));

        mock.shutdown();
    }

    #[tokio::test]
    async fn enrich_jobs_does_not_auto_trust_derived_in_cluster_urls_for_session_jobs() {
        let mut job = test_job(None);
        job.kind = "FlinkSessionJob".to_owned();

        let mut cluster = test_cluster(None);
        cluster.derive_jobmanager_url_in_cluster = true;

        let jobs = enrich_jobs(vec![job], &cluster, 500).await;

        assert_eq!(
            jobs[0].warnings,
            vec![
                "Flink REST enrichment unavailable: no trusted Flink REST base URL for FlinkSessionJob"
                    .to_owned()
            ]
        );
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

    fn test_job(native_ui_url: Option<String>) -> Job {
        Job {
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
            native_ui_url,
            warnings: Vec::new(),
            details: JobDetails::default(),
        }
    }

    fn test_cluster(flink_rest_base_url: Option<String>) -> ClusterConfig {
        ClusterConfig {
            name: "demo".to_owned(),
            api_url: "https://kubernetes.example.com".to_owned(),
            bearer_token: "token".to_owned(),
            ca_cert: None,
            insecure_skip_tls_verify: false,
            namespaces: vec!["analytics".to_owned()],
            flink_api_version: "v1beta1".to_owned(),
            derive_jobmanager_url_in_cluster: false,
            flink_rest_base_url,
        }
    }

    #[test]
    fn overview_url_preserves_existing_base_path() {
        let trusted_base_url = Url::parse("https://flink.example.com/platform/proxy/")
            .expect("trusted base URL should parse");

        let result = overview_url(&trusted_base_url).expect("overview URL should build");

        assert_eq!(
            result.as_str(),
            "https://flink.example.com/platform/proxy/jobs/overview"
        );
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

    async fn start_counting_server(counter: Arc<AtomicUsize>) -> MockServer {
        let app = Router::new().fallback(get(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(json!({"jobs": []}))).into_response()
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
                .expect("counting server should run");
        });

        MockServer {
            base_url: format!("http://{}", address),
            task,
        }
    }
}

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};

use crate::config::ClusterConfig;
use crate::domain::job::{Job, JobAction};
use crate::domain::normalize::normalize_flink_resource;
use crate::error::UpstreamHttpError;

pub async fn list_cluster_jobs(
    cluster: &ClusterConfig,
    request_timeout_ms: u64,
) -> Result<Vec<Job>> {
    let deployments =
        list_resources_for_plural(cluster, "flinkdeployments", request_timeout_ms).await?;
    let session_jobs =
        match list_resources_for_plural(cluster, "flinksessionjobs", request_timeout_ms).await {
            Ok(resources) => resources,
            Err(_) => Vec::new(),
        };

    Ok(deployments
        .into_iter()
        .chain(session_jobs.into_iter())
        .map(|resource| normalize_flink_resource(resource, cluster))
        .collect())
}

pub async fn apply_job_action(
    cluster: &ClusterConfig,
    namespace: &str,
    kind: &str,
    name: &str,
    action: JobAction,
    request_timeout_ms: u64,
) -> Result<()> {
    let client = build_client(cluster, request_timeout_ms)?;
    let plural = resource_plural(kind)?;
    let path = format!(
        "/apis/flink.apache.org/{}/namespaces/{}/{}/{}",
        cluster.flink_api_version, namespace, plural, name
    );
    let url = format!("{}{}", cluster.api_url.trim_end_matches('/'), path);

    let request = match action {
        JobAction::Cancel => client.delete(&url),
        JobAction::Suspend | JobAction::Resume => {
            let state = match action {
                JobAction::Suspend => "suspended",
                JobAction::Resume => "running",
                JobAction::Cancel => unreachable!("cancel is handled in a separate match arm"),
            };
            let patch_body = json!({
                "spec": {
                    "job": {
                        "state": state,
                    }
                }
            });

            client
                .patch(&url)
                .header("Content-Type", "application/merge-patch+json")
                .json(&patch_body)
        }
    };

    let response = request
        .header("Accept", "application/json")
        .bearer_auth(&cluster.bearer_token)
        .send()
        .await
        .with_context(|| format!("failed to {} {}", action.as_str(), path))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if status.is_client_error() || status.is_server_error() {
        return Err(UpstreamHttpError {
            status_code: status.as_u16(),
            message: format!(
                "Kubernetes API {} for {} {}: {}",
                status.as_u16(),
                action.as_str().to_ascii_uppercase(),
                path,
                body.chars().take(200).collect::<String>()
            ),
        }
        .into());
    }

    Ok(())
}

async fn list_resources_for_plural(
    cluster: &ClusterConfig,
    plural: &str,
    request_timeout_ms: u64,
) -> Result<Vec<Value>> {
    let client = build_client(cluster, request_timeout_ms)?;
    let mut resources = Vec::new();

    for path in build_namespace_paths(cluster, plural) {
        let response = request_json(&client, cluster, &path).await?;
        resources.extend(
            response
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
    }

    Ok(resources)
}

fn build_namespace_paths(cluster: &ClusterConfig, plural: &str) -> Vec<String> {
    if cluster
        .namespaces
        .iter()
        .any(|namespace| namespace == "*" || namespace.eq_ignore_ascii_case("all"))
    {
        return vec![format!(
            "/apis/flink.apache.org/{}/{}",
            cluster.flink_api_version, plural
        )];
    }

    cluster
        .namespaces
        .iter()
        .map(|namespace| {
            format!(
                "/apis/flink.apache.org/{}/namespaces/{}/{}",
                cluster.flink_api_version, namespace, plural
            )
        })
        .collect()
}

fn resource_plural(kind: &str) -> Result<&'static str> {
    match kind {
        "FlinkDeployment" => Ok("flinkdeployments"),
        "FlinkSessionJob" => Ok("flinksessionjobs"),
        _ => anyhow::bail!("unsupported Flink resource kind `{kind}`"),
    }
}

fn build_client(cluster: &ClusterConfig, request_timeout_ms: u64) -> Result<Client> {
    cluster
        .validate_kubernetes_tls_policy()
        .context("refusing to build Kubernetes client")?;
    let mut builder =
        Client::builder().timeout(std::time::Duration::from_millis(request_timeout_ms));

    if let Some(ca_cert) = &cluster.ca_cert {
        let certificate = reqwest::Certificate::from_pem(ca_cert.as_bytes())
            .context("failed to parse cluster CA certificate")?;
        builder = builder.add_root_certificate(certificate);
    }

    if cluster.insecure_skip_tls_verify {
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder.build().context("failed to build HTTP client")
}

async fn request_json(client: &Client, cluster: &ClusterConfig, path: &str) -> Result<Value> {
    let url = format!("{}{}", cluster.api_url.trim_end_matches('/'), path);
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .bearer_auth(&cluster.bearer_token)
        .send()
        .await
        .with_context(|| format!("failed to fetch {}", path))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if status.is_client_error() || status.is_server_error() {
        return Err(UpstreamHttpError {
            status_code: status.as_u16(),
            message: format!(
                "Kubernetes API {} for {}: {}",
                status.as_u16(),
                path,
                body.chars().take(200).collect::<String>()
            ),
        }
        .into());
    }

    serde_json::from_str(&body).with_context(|| format!("invalid JSON from {}", path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::extract::{OriginalUri, Request};
    use axum::http::{Method, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{any, get};
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    fn cluster(base_url: &str) -> ClusterConfig {
        ClusterConfig {
            name: "demo".to_owned(),
            api_url: base_url.to_owned(),
            bearer_token: "token".to_owned(),
            ca_cert: None,
            insecure_skip_tls_verify: false,
            namespaces: vec!["analytics".to_owned()],
            flink_api_version: "v1beta1".to_owned(),
            derive_jobmanager_url_in_cluster: false,
            flink_rest_base_url: None,
        }
    }

    #[tokio::test]
    async fn list_cluster_jobs_normalizes_deployments_from_mock_kubernetes() {
        let mock = start_mock_server(vec![(
            "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
            StatusCode::OK,
            json!({
              "items": [{
                "kind": "FlinkDeployment",
                "metadata": {
                  "name": "orders-stream",
                  "namespace": "analytics",
                  "creationTimestamp": "2026-04-01T00:00:00Z",
                  "labels": {"app.kubernetes.io/name": "orders-stream"}
                },
                "spec": {
                  "flinkVersion": "1.19",
                  "mode": "native",
                  "job": {"name": "orders-stream"}
                },
                "status": {
                  "jobStatus": {"state": "RUNNING"},
                  "reconciliationStatus": {
                    "state": "READY",
                    "lastReconciledAt": "2026-04-02T00:00:00Z"
                  },
                  "jobManagerUrl": "https://flink.example.com/orders-stream/"
                }
              }]
            }),
        )])
        .await;
        let cluster = cluster(&mock.base_url);

        let jobs = list_cluster_jobs(&cluster, 1_000)
            .await
            .expect("jobs should load");

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "running");
        assert_eq!(jobs[0].resource_name, "orders-stream");
        assert_eq!(
            jobs[0].native_ui_url.as_deref(),
            Some("https://flink.example.com/orders-stream/")
        );

        mock.shutdown();
    }

    #[tokio::test]
    async fn list_cluster_jobs_treats_session_job_failures_as_best_effort() {
        let mock = start_mock_server(vec![
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "status": {"jobStatus": {"state": "RUNNING"}}
                  }]
                }),
            ),
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs".to_owned(),
                StatusCode::NOT_FOUND,
                json!({"error":"missing"}),
            ),
        ])
        .await;
        let cluster = cluster(&mock.base_url);

        let jobs = list_cluster_jobs(&cluster, 1_000)
            .await
            .expect("jobs should load");

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "FlinkDeployment");

        mock.shutdown();
    }

    #[tokio::test]
    async fn list_cluster_jobs_derives_in_cluster_jobmanager_url_when_missing() {
        let mock = start_mock_server(vec![
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "status": {"jobStatus": {"state": "RUNNING"}}
                  }]
                }),
            ),
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs".to_owned(),
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let mut cluster = cluster(&mock.base_url);
        cluster.derive_jobmanager_url_in_cluster = true;

        let jobs = list_cluster_jobs(&cluster, 1_000)
            .await
            .expect("jobs should load");

        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].native_ui_url.as_deref(),
            Some("http://orders-stream-rest.analytics.svc:8081/")
        );

        mock.shutdown();
    }

    #[test]
    fn build_client_rejects_insecure_tls_bypass_for_non_loopback_cluster() {
        let mut cluster = cluster("https://kubernetes.example.com");
        cluster.insecure_skip_tls_verify = true;

        let error = build_client(&cluster, 1_000)
            .expect_err("non-loopback insecure TLS bypass should be rejected");

        assert!(format!("{error:#}").contains("K8S_INSECURE_SKIP_TLS_VERIFY"));
    }

    #[test]
    fn build_client_reports_invalid_k8s_api_url_for_insecure_tls_bypass() {
        let mut cluster = cluster("not a url");
        cluster.insecure_skip_tls_verify = true;

        let error = build_client(&cluster, 1_000)
            .expect_err("invalid Kubernetes API URL should be reported explicitly");

        assert!(format!("{error:#}").contains("invalid Kubernetes API URL"));
        assert!(!format!("{error:#}").contains("only allowed for localhost or loopback"));
    }

    #[tokio::test]
    async fn apply_job_action_patches_suspend_state_for_deployments() {
        let recorder = Arc::new(Mutex::new(Vec::new()));
        let mock = start_recording_server(
            vec![(
                Method::PATCH,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments/orders-stream"
                    .to_owned(),
                StatusCode::OK,
                json!({}),
            )],
            Arc::clone(&recorder),
        )
        .await;
        let cluster = cluster(&mock.base_url);

        apply_job_action(
            &cluster,
            "analytics",
            "FlinkDeployment",
            "orders-stream",
            JobAction::Suspend,
            1_000,
        )
        .await
        .expect("suspend should succeed");

        let captured = recorder.lock().await;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, Method::PATCH);
        assert_eq!(
            captured[0].1,
            "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments/orders-stream"
        );
        assert_eq!(captured[0].2, r#"{"spec":{"job":{"state":"suspended"}}}"#);

        mock.shutdown();
    }

    #[tokio::test]
    async fn apply_job_action_patches_running_state_for_session_jobs() {
        let recorder = Arc::new(Mutex::new(Vec::new()));
        let mock = start_recording_server(
            vec![(
                Method::PATCH,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs/settlement-job"
                    .to_owned(),
                StatusCode::OK,
                json!({}),
            )],
            Arc::clone(&recorder),
        )
        .await;
        let cluster = cluster(&mock.base_url);

        apply_job_action(
            &cluster,
            "analytics",
            "FlinkSessionJob",
            "settlement-job",
            JobAction::Resume,
            1_000,
        )
        .await
        .expect("resume should succeed");

        let captured = recorder.lock().await;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, Method::PATCH);
        assert_eq!(captured[0].2, r#"{"spec":{"job":{"state":"running"}}}"#);

        mock.shutdown();
    }

    #[tokio::test]
    async fn apply_job_action_deletes_resources_for_cancel() {
        let recorder = Arc::new(Mutex::new(Vec::new()));
        let mock = start_recording_server(
            vec![(
                Method::DELETE,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments/orders-stream"
                    .to_owned(),
                StatusCode::OK,
                json!({}),
            )],
            Arc::clone(&recorder),
        )
        .await;
        let cluster = cluster(&mock.base_url);

        apply_job_action(
            &cluster,
            "analytics",
            "FlinkDeployment",
            "orders-stream",
            JobAction::Cancel,
            1_000,
        )
        .await
        .expect("cancel should succeed");

        let captured = recorder.lock().await;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, Method::DELETE);
        assert_eq!(captured[0].2, "");

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

    async fn start_recording_server(
        responses: Vec<(Method, String, StatusCode, Value)>,
        recorder: Arc<Mutex<Vec<(Method, String, String)>>>,
    ) -> MockServer {
        let responses = Arc::new(responses);
        let app = Router::new().fallback(any({
            let responses = Arc::clone(&responses);
            move |request: Request| {
                let responses = Arc::clone(&responses);
                let recorder = Arc::clone(&recorder);
                async move {
                    let method = request.method().clone();
                    let path = request.uri().path().to_owned();
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read");
                    recorder.lock().await.push((
                        method.clone(),
                        path.clone(),
                        String::from_utf8(body.to_vec()).expect("body should be utf-8"),
                    ));
                    match responses
                        .iter()
                        .find(|(candidate_method, candidate_path, _, _)| {
                            candidate_method == &method && candidate_path == &path
                        }) {
                        Some((_, _, status, payload)) => {
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
                .expect("recording mock server should run");
        });

        MockServer {
            base_url: format!("http://{}", address),
            task,
        }
    }
}

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;

use crate::config::ClusterConfig;
use crate::domain::job::Job;
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

fn build_client(cluster: &ClusterConfig, request_timeout_ms: u64) -> Result<Client> {
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

    use axum::extract::OriginalUri;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

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
        let cluster = ClusterConfig {
            name: "demo".to_owned(),
            api_url: mock.base_url.clone(),
            bearer_token: "token".to_owned(),
            ca_cert: None,
            insecure_skip_tls_verify: false,
            namespaces: vec!["analytics".to_owned()],
            flink_api_version: "v1beta1".to_owned(),
            flink_rest_base_url: None,
        };

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
        let cluster = ClusterConfig {
            name: "demo".to_owned(),
            api_url: mock.base_url.clone(),
            bearer_token: "token".to_owned(),
            ca_cert: None,
            insecure_skip_tls_verify: false,
            namespaces: vec!["analytics".to_owned()],
            flink_api_version: "v1beta1".to_owned(),
            flink_rest_base_url: None,
        };

        let jobs = list_cluster_jobs(&cluster, 1_000)
            .await
            .expect("jobs should load");

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "FlinkDeployment");

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

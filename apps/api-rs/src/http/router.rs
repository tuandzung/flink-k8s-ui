use axum::extract::{MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::services::ServeDir;

use crate::http::handlers::health::{healthz, readyz};
use crate::http::handlers::jobs::{get_clusters, get_job_by_id, get_job_by_locator, list_jobs};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let static_dir = state.config.static_dir();
    let protected_routes = Router::new()
        .route("/api/jobs", get(list_jobs))
        .route("/api/clusters", get(get_clusters))
        .route(
            "/api/jobs/{cluster}/{namespace}/{kind}/{name}",
            get(get_job_by_locator),
        )
        .route("/api/jobs/{id}", get(get_job_by_id))
        .route("/metrics", get(metrics))
        .route_layer(from_fn_with_state(state.clone(), require_trusted_auth));

    Router::new()
        .merge(protected_routes)
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .layer(from_fn_with_state(state.clone(), track_requests))
        .with_state(state)
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.metrics.snapshot().await)
}

#[derive(Serialize)]
struct AuthErrorResponse {
    error: &'static str,
}

async fn require_trusted_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if state.config.fixture_mode
        || state.config.trusted_auth_headers.iter().any(|header_name| {
            request
                .headers()
                .get(header_name.as_str())
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| !value.trim().is_empty())
        })
    {
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(AuthErrorResponse {
            error: "Missing trusted auth header",
        }),
    )
        .into_response()
}

async fn track_requests(State(state): State<AppState>, request: Request, next: Next) -> Response {
    state.metrics.increment_requests().await;
    let route = route_label(
        request.method().as_str(),
        request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str),
    );
    let response = next.run(request).await;
    let status = response.status();
    state.metrics.record_response(&route, status.as_u16()).await;

    if status == StatusCode::NOT_FOUND {
        return response;
    }

    response
}

fn route_label(method: &str, matched_path: Option<&str>) -> String {
    match (method, matched_path) {
        ("GET", Some("/api/jobs")) => "listJobs",
        ("GET", Some("/api/clusters")) => "getClusters",
        ("GET", Some("/api/jobs/{id}"))
        | ("GET", Some("/api/jobs/{cluster}/{namespace}/{kind}/{name}")) => "getJob",
        ("GET", Some("/healthz")) => "healthz",
        ("GET", Some("/readyz")) => "readyz",
        ("GET", Some("/metrics")) => "metrics",
        _ => "static",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::extract::OriginalUri;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::{Json, Router};
    use reqwest::{Client, RequestBuilder};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::build_router;
    use crate::config::{AppConfig, ClusterConfig};
    use crate::state::AppState;

    const TRUSTED_AUTH_HEADER: &str = "x-auth-request-user";
    const TRUSTED_AUTH_VALUE: &str = "developer@example.com";

    #[tokio::test]
    async fn live_mode_end_to_end_preserves_api_contract_against_mocked_upstreams() {
        let flink = start_mock_json_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({
              "jobs": [{
                "jid": "job-123",
                "name": "orders-stream",
                "state": "RUNNING",
                "start-time": 1_775_000_000_000_i64
              }]
            }),
        )])
        .await;
        let kubernetes = start_mock_json_server(vec![
            (
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
                      "jobStatus": {"state": "READY"},
                      "reconciliationStatus": {
                        "state": "READY",
                        "lastReconciledAt": "2026-04-02T00:00:00Z"
                      },
                      "jobManagerUrl": format!("{}/orders-stream/", flink.base_url)
                    }
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
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let jobs_response = authorized(client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("jobs response should succeed");
        assert_eq!(jobs_response.status(), StatusCode::OK);
        let jobs_payload: Value = jobs_response
            .json()
            .await
            .expect("jobs payload should be JSON");
        assert_eq!(jobs_payload["meta"]["total"], 1);
        assert_eq!(jobs_payload["jobs"][0]["resourceName"], "orders-stream");
        assert_eq!(jobs_payload["jobs"][0]["flinkJobId"], "job-123");
        assert_eq!(jobs_payload["jobs"][0]["rawStatus"], "RUNNING");
        assert_eq!(jobs_payload["jobs"][0]["warnings"], json!([]));
        assert!(jobs_payload["jobs"][0]["details"]["flinkRestOverview"].is_object());

        let clusters_response = authorized(client.get(format!("{}/api/clusters", app.base_url)))
            .send()
            .await
            .expect("clusters response should succeed");
        assert_eq!(clusters_response.status(), StatusCode::OK);
        let clusters_payload: Value = clusters_response
            .json()
            .await
            .expect("clusters payload should be JSON");
        assert_eq!(clusters_payload, json!({"clusters":[{"name":"demo"}]}));

        let detail_response = authorized(client.get(format!(
            "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
            app.base_url
        )))
        .send()
        .await
        .expect("detail response should succeed");
        assert_eq!(detail_response.status(), StatusCode::OK);
        let detail_payload: Value = detail_response
            .json()
            .await
            .expect("detail payload should be JSON");
        assert_eq!(detail_payload["job"]["flinkJobId"], "job-123");

        for path in ["/healthz", "/readyz"] {
            let response = client
                .get(format!("{}{}", app.base_url, path))
                .send()
                .await
                .expect("health endpoints should succeed");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let metrics_response = authorized(client.get(format!("{}/metrics", app.base_url)))
            .send()
            .await
            .expect("metrics response should succeed");
        assert_eq!(metrics_response.status(), StatusCode::OK);
        let metrics_payload: Value = metrics_response
            .json()
            .await
            .expect("metrics payload should be JSON");
        assert_eq!(metrics_payload["requestsTotal"], 6);
        assert_eq!(metrics_payload["errorsTotal"], 0);
        assert_eq!(
            metrics_payload["routes"],
            json!({
              "getClusters": 1,
              "getJob": 1,
              "healthz": 1,
              "listJobs": 1,
              "readyz": 1
            })
        );

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_propagates_kubernetes_error_status_codes() {
        let kubernetes = start_mock_json_server(vec![(
            "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
            StatusCode::FORBIDDEN,
            json!({"error":"forbidden"}),
        )])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Failed to list jobs");
        assert!(
            payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("Kubernetes API 403")
        );

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn fixture_mode_preserves_http_contract() {
        let fixture_jobs = load_fixture_jobs();
        let app = start_app(fixture_config()).await;
        let client = Client::new();

        let jobs_response = client
            .get(format!("{}/api/jobs", app.base_url))
            .send()
            .await
            .expect("jobs response should succeed");
        assert_eq!(jobs_response.status(), StatusCode::OK);
        let jobs_payload: Value = jobs_response
            .json()
            .await
            .expect("jobs payload should be JSON");
        assert_eq!(
            jobs_payload["jobs"],
            Value::Array(sort_fixture_jobs(fixture_jobs.clone()))
        );
        assert_eq!(jobs_payload["meta"]["total"], fixture_jobs.len());
        assert!(
            jobs_payload["meta"]["generatedAt"]
                .as_str()
                .expect("generatedAt should be a string")
                .starts_with("20")
        );

        let clusters_response = client
            .get(format!("{}/api/clusters", app.base_url))
            .send()
            .await
            .expect("clusters response should succeed");
        assert_eq!(clusters_response.status(), StatusCode::OK);
        let clusters_payload: Value = clusters_response
            .json()
            .await
            .expect("clusters payload should be JSON");
        assert_eq!(
            clusters_payload,
            json!({"clusters":[{"name":"demo"},{"name":"prod"}]})
        );

        let detail_response = client
            .get(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
                app.base_url
            ))
            .send()
            .await
            .expect("detail response should succeed");
        assert_eq!(detail_response.status(), StatusCode::OK);
        let detail_payload: Value = detail_response
            .json()
            .await
            .expect("detail payload should be JSON");
        assert_eq!(
            detail_payload,
            json!({
                "job": sort_fixture_jobs(fixture_jobs)
                    .into_iter()
                    .find(|job| {
                        job["cluster"] == "demo"
                            && job["namespace"] == "analytics"
                            && job["kind"] == "FlinkDeployment"
                            && job["resourceName"] == "orders-stream"
                    })
                    .expect("fixture job should exist")
            })
        );

        let healthz_response = client
            .get(format!("{}/healthz", app.base_url))
            .send()
            .await
            .expect("healthz response should succeed");
        assert_eq!(healthz_response.status(), StatusCode::OK);
        let healthz_payload: Value = healthz_response
            .json()
            .await
            .expect("healthz payload should be JSON");
        assert_eq!(healthz_payload["status"], "ok");

        let readyz_response = client
            .get(format!("{}/readyz", app.base_url))
            .send()
            .await
            .expect("readyz response should succeed");
        assert_eq!(readyz_response.status(), StatusCode::OK);
        let readyz_payload: Value = readyz_response
            .json()
            .await
            .expect("readyz payload should be JSON");
        assert_eq!(readyz_payload["status"], "ready");

        let metrics_response = client
            .get(format!("{}/metrics", app.base_url))
            .send()
            .await
            .expect("metrics response should succeed");
        assert_eq!(metrics_response.status(), StatusCode::OK);
        let metrics_payload: Value = metrics_response
            .json()
            .await
            .expect("metrics payload should be JSON");
        assert_eq!(metrics_payload["requestsTotal"], 6);
        assert_eq!(metrics_payload["errorsTotal"], 0);
        assert_eq!(
            metrics_payload["routes"],
            json!({
              "getClusters": 1,
              "getJob": 1,
              "healthz": 1,
              "listJobs": 1,
              "readyz": 1
            })
        );

        app.shutdown();
    }

    #[tokio::test]
    async fn fixture_mode_serves_static_ui_and_metrics() {
        let app = start_app(fixture_config()).await;
        let client = Client::new();

        let page_response = client
            .get(format!("{}/", app.base_url))
            .send()
            .await
            .expect("page response should succeed");
        assert_eq!(page_response.status(), StatusCode::OK);
        let page_html = page_response.text().await.expect("page HTML should load");
        assert!(page_html.contains("Jobs + Status Dashboard"));

        let jobs_response = client
            .get(format!("{}/api/jobs", app.base_url))
            .send()
            .await
            .expect("jobs response should succeed");
        assert_eq!(jobs_response.status(), StatusCode::OK);
        let jobs_payload: Value = jobs_response
            .json()
            .await
            .expect("jobs payload should be JSON");
        assert_eq!(jobs_payload["meta"]["total"], 4);

        let detail_response = client
            .get(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
                app.base_url
            ))
            .send()
            .await
            .expect("detail response should succeed");
        assert_eq!(detail_response.status(), StatusCode::OK);
        let detail_payload: Value = detail_response
            .json()
            .await
            .expect("detail payload should be JSON");
        assert_eq!(detail_payload["job"]["resourceName"], "orders-stream");

        let metrics_response = client
            .get(format!("{}/metrics", app.base_url))
            .send()
            .await
            .expect("metrics response should succeed");
        assert_eq!(metrics_response.status(), StatusCode::OK);
        let metrics_payload: Value = metrics_response
            .json()
            .await
            .expect("metrics payload should be JSON");
        assert!(
            metrics_payload["requestsTotal"]
                .as_u64()
                .expect("requestsTotal should be numeric")
                >= 3
        );

        app.shutdown();
    }

    #[tokio::test]
    async fn live_mode_requires_trusted_auth_headers_for_protected_routes() {
        let flink = start_mock_json_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({"jobs":[]}),
        )])
        .await;
        let kubernetes = start_mock_json_server(vec![
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
                StatusCode::OK,
                json!({"items":[]}),
            ),
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs".to_owned(),
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, Some(&flink.base_url))).await;
        let client = Client::new();

        for path in ["/api/jobs", "/api/clusters", "/metrics"] {
            let response = client
                .get(format!("{}{}", app.base_url, path))
                .send()
                .await
                .expect("unauthorized response should succeed");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let payload: Value = response.json().await.expect("payload should be JSON");
            assert_eq!(payload, json!({"error":"Missing trusted auth header"}));
        }

        let detail_response = client
            .get(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
                app.base_url
            ))
            .send()
            .await
            .expect("detail unauthorized response should succeed");
        assert_eq!(detail_response.status(), StatusCode::UNAUTHORIZED);

        let healthz_response = client
            .get(format!("{}/healthz", app.base_url))
            .send()
            .await
            .expect("healthz response should succeed");
        assert_eq!(healthz_response.status(), StatusCode::OK);

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    struct RunningServer {
        base_url: String,
        task: JoinHandle<()>,
    }

    impl RunningServer {
        fn shutdown(self) {
            self.task.abort();
        }
    }

    async fn start_app(config: AppConfig) -> RunningServer {
        let state = AppState::new(config);
        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have local address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("app server should run");
        });

        RunningServer {
            base_url: format!("http://{}", address),
            task,
        }
    }

    async fn start_mock_json_server(responses: Vec<(String, StatusCode, Value)>) -> RunningServer {
        let responses = Arc::new(
            responses
                .into_iter()
                .map(|(path, status, payload)| (path, (status, payload)))
                .collect::<BTreeMap<String, (StatusCode, Value)>>(),
        );
        let app = Router::new().fallback({
            let responses = Arc::clone(&responses);
            move |uri: OriginalUri| {
                let responses = Arc::clone(&responses);
                async move {
                    match responses.get(uri.path()) {
                        Some((status, payload)) => (*status, Json(payload.clone())).into_response(),
                        None => (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})))
                            .into_response(),
                    }
                }
            }
        });
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

        RunningServer {
            base_url: format!("http://{}", address),
            task,
        }
    }

    fn fixture_config() -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 0,
            root_dir: workspace_root(),
            fixture_mode: true,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            trusted_auth_headers: vec![TRUSTED_AUTH_HEADER.to_owned()],
            clusters: Vec::new(),
        }
    }

    fn live_config(kubernetes_base_url: &str, flink_base_url: Option<&str>) -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 0,
            root_dir: workspace_root(),
            fixture_mode: false,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            trusted_auth_headers: vec![TRUSTED_AUTH_HEADER.to_owned()],
            clusters: vec![ClusterConfig {
                name: "demo".to_owned(),
                api_url: kubernetes_base_url.to_owned(),
                bearer_token: "token".to_owned(),
                ca_cert: None,
                insecure_skip_tls_verify: false,
                namespaces: vec!["analytics".to_owned()],
                flink_api_version: "v1beta1".to_owned(),
                flink_rest_base_url: flink_base_url.map(ToOwned::to_owned),
            }],
        }
    }

    fn authorized(request: RequestBuilder) -> RequestBuilder {
        request.header(TRUSTED_AUTH_HEADER, TRUSTED_AUTH_VALUE)
    }

    fn load_fixture_jobs() -> Vec<Value> {
        let fixture_path = workspace_root().join("fixtures/jobs.json");
        let payload: Value = serde_json::from_str(
            &fs::read_to_string(fixture_path).expect("fixture file should be readable"),
        )
        .expect("fixture JSON should parse");
        payload["jobs"]
            .as_array()
            .expect("fixture jobs should be an array")
            .clone()
    }

    fn sort_fixture_jobs(mut jobs: Vec<Value>) -> Vec<Value> {
        jobs.sort_by(|left, right| {
            left["cluster"]
                .as_str()
                .cmp(&right["cluster"].as_str())
                .then_with(|| left["namespace"].as_str().cmp(&right["namespace"].as_str()))
                .then_with(|| {
                    left["resourceName"]
                        .as_str()
                        .cmp(&right["resourceName"].as_str())
                })
        });
        jobs
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate should be nested in apps/")
            .parent()
            .expect("workspace root should exist")
            .to_path_buf()
    }
}

use std::io::ErrorKind;

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::http::handlers::auth::{callback, login, logout, session_status};
use crate::http::handlers::health::{healthz, readyz};
use crate::http::handlers::jobmanager_proxy::{proxy_jobmanager_path, proxy_jobmanager_root};
use crate::http::handlers::jobs::{
    get_clusters, get_job_by_id, get_job_by_locator, list_jobs, post_job_action,
};
use crate::state::AppState;

const SHELL_HTML_CACHE_CONTROL: &str = "no-store";
const VERSIONED_SHELL_ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
const FAVICON_CACHE_CONTROL: &str = "no-cache";

pub fn build_router(state: AppState) -> Router {
    let public_shell = Router::new()
        .route("/", get(shell_index_html))
        .route("/app.js", get(shell_app_js))
        .route("/render.js", get(shell_render_js))
        .route("/styles.css", get(shell_styles_css))
        .route("/favicon.ico", get(shell_favicon));
    let protected_api = Router::new()
        .route("/api/jobs", get(list_jobs))
        .route("/api/clusters", get(get_clusters))
        .route(
            "/api/jobs/{cluster}/{namespace}/{kind}/{name}",
            get(get_job_by_locator),
        )
        .route(
            "/api/jobs/{cluster}/{namespace}/{kind}/{name}/actions/{action}",
            post(post_job_action),
        )
        .route("/api/jobs/{id}", get(get_job_by_id))
        .route(
            "/api/jobs/{id}/jobmanager-proxy",
            get(proxy_jobmanager_root),
        )
        .route(
            "/api/jobs/{id}/jobmanager-proxy/",
            get(proxy_jobmanager_root),
        )
        .route(
            "/api/jobs/{id}/jobmanager-proxy/{*path}",
            get(proxy_jobmanager_path),
        )
        .route_layer(from_fn_with_state(state.clone(), require_session_for_api));

    Router::new()
        .merge(public_shell)
        .merge(protected_api)
        .route("/api/session", get(session_status))
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .layer(from_fn_with_state(state.clone(), track_requests))
        .with_state(state)
}

async fn shell_index_html(State(state): State<AppState>) -> Response {
    let template = match tokio::fs::read_to_string(state.config.index_html_path()).await {
        Ok(template) => template,
        Err(error) => return file_error_response(error),
    };
    let html = render_index_html(&template, &state.shell_asset_version);

    build_static_response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        SHELL_HTML_CACHE_CONTROL,
        html.into_bytes(),
    )
}

async fn shell_app_js(State(state): State<AppState>) -> Response {
    match tokio::fs::read_to_string(state.config.static_dir().join("app.js")).await {
        Ok(source) => build_static_response(
            StatusCode::OK,
            "text/javascript; charset=utf-8",
            VERSIONED_SHELL_ASSET_CACHE_CONTROL,
            rewrite_app_js(&source, &state.shell_asset_version).into_bytes(),
        ),
        Err(error) => file_error_response(error),
    }
}

async fn shell_render_js(State(state): State<AppState>) -> Response {
    serve_static_asset(
        state.config.static_dir().join("render.js"),
        "text/javascript; charset=utf-8",
        VERSIONED_SHELL_ASSET_CACHE_CONTROL,
    )
    .await
}

async fn shell_styles_css(State(state): State<AppState>) -> Response {
    serve_static_asset(
        state.config.static_dir().join("styles.css"),
        "text/css; charset=utf-8",
        VERSIONED_SHELL_ASSET_CACHE_CONTROL,
    )
    .await
}

async fn shell_favicon(State(state): State<AppState>) -> Response {
    serve_static_asset(
        state.config.static_dir().join("favicon.ico"),
        "image/x-icon",
        FAVICON_CACHE_CONTROL,
    )
    .await
}

async fn serve_static_asset(
    path: std::path::PathBuf,
    content_type: &'static str,
    cache_control: &'static str,
) -> Response {
    match tokio::fs::read(path).await {
        Ok(contents) => {
            build_static_response(StatusCode::OK, content_type, cache_control, contents)
        }
        Err(error) => file_error_response(error),
    }
}

fn render_index_html(template: &str, asset_version: &str) -> String {
    let styles_href = format!(r#"href="/styles.css?v={asset_version}""#);
    let script_src = format!(r#"<script type="module" src="/app.js?v={asset_version}"></script>"#);

    template
        .replace(r#"href="/styles.css""#, &styles_href)
        .replace(
            r#"<script type="module" src="/app.js"></script>"#,
            &script_src,
        )
}

fn rewrite_app_js(source: &str, asset_version: &str) -> String {
    source.replacen("./render.js", &format!("./render.js?v={asset_version}"), 1)
}

fn build_static_response(
    status: StatusCode,
    content_type: &'static str,
    cache_control: &'static str,
    body: Vec<u8>,
) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
        ],
        Body::from(body),
    )
        .into_response()
}

fn file_error_response(error: std::io::Error) -> Response {
    match error.kind() {
        ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.metrics.snapshot().await)
}

#[derive(Serialize)]
struct AuthErrorResponse {
    error: &'static str,
}

async fn require_session_for_api(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if state.config.fixture_mode {
        return next.run(request).await;
    }

    match state
        .auth
        .session_from_headers(&state.config, request.headers())
        .await
    {
        Ok(Some(_)) => next.run(request).await,
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(AuthErrorResponse {
                error: "Missing or expired session",
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AuthErrorResponse {
                error: "Failed to validate session",
            }),
        )
            .into_response(),
    }
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
        ("POST", Some("/api/jobs/{cluster}/{namespace}/{kind}/{name}/actions/{action}")) => {
            "jobAction"
        }
        ("GET", Some("/api/jobs/{id}/jobmanager-proxy"))
        | ("GET", Some("/api/jobs/{id}/jobmanager-proxy/"))
        | ("GET", Some("/api/jobs/{id}/jobmanager-proxy/{*path}")) => "jobManagerProxy",
        ("GET", Some("/api/session")) => "sessionStatus",
        ("GET", Some("/auth/login")) => "authLogin",
        ("GET", Some("/auth/callback")) => "authCallback",
        ("POST", Some("/auth/logout")) => "authLogout",
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
    use std::net::TcpListener as StdTcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::extract::{OriginalUri, Request};
    use axum::http::{Method, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::{Json, Router};
    use reqwest::{Client, RequestBuilder, redirect::Policy};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    use super::build_router;
    use crate::config::{AppConfig, ClusterConfig, OidcConfig};
    use crate::state::AppState;

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
        let app = start_app(live_config(&kubernetes.base_url, Some(&flink.base_url))).await;
        let client = Client::new();

        let jobs_response = authorized(&app, client.get(format!("{}/api/jobs", app.base_url)))
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
        assert_eq!(
            jobs_payload["jobs"][0]["details"]["statusSummary"]["jobState"],
            "READY"
        );
        assert_eq!(
            jobs_payload["jobs"][0]["details"]["statusSummary"]["reconciliationState"],
            "READY"
        );
        assert_eq!(
            jobs_payload["jobs"][0]["details"]["flinkRestOverview"]["jobId"],
            "job-123"
        );
        assert!(jobs_payload["jobs"][0]["details"].get("metadata").is_none());
        assert!(jobs_payload["jobs"][0]["details"].get("spec").is_none());
        assert!(jobs_payload["jobs"][0]["details"].get("status").is_none());

        let clusters_response =
            authorized(&app, client.get(format!("{}/api/clusters", app.base_url)))
                .send()
                .await
                .expect("clusters response should succeed");
        assert_eq!(clusters_response.status(), StatusCode::OK);
        let clusters_payload: Value = clusters_response
            .json()
            .await
            .expect("clusters payload should be JSON");
        assert_eq!(clusters_payload, json!({"clusters":[{"name":"demo"}]}));

        let detail_response = authorized(
            &app,
            client.get(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
                app.base_url
            )),
        )
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
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_surfaces_warning_when_only_status_derived_jobmanager_url_exists() {
        let flink = start_mock_json_server(vec![(
            "/jobs/overview".to_owned(),
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
        let kubernetes = start_mock_json_server(vec![
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {
                      "name": "orders-stream",
                      "namespace": "analytics"
                    },
                    "spec": {
                      "job": {"name": "orders-stream"}
                    },
                    "status": {
                      "jobStatus": {"state": "READY"},
                      "jobManagerUrl": format!("{}/orders-stream/", flink.base_url)
                    }
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
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let jobs_response = authorized(&app, client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("jobs response should succeed");
        assert_eq!(jobs_response.status(), StatusCode::OK);
        let jobs_payload: Value = jobs_response
            .json()
            .await
            .expect("jobs payload should be JSON");

        assert_eq!(jobs_payload["jobs"][0]["flinkJobId"], Value::Null);
        assert_eq!(
            jobs_payload["jobs"][0]["details"]["flinkRestOverview"],
            Value::Null
        );
        assert_eq!(
            jobs_payload["jobs"][0]["warnings"],
            json!(["Flink REST enrichment skipped: no trusted Flink REST base URL is configured"])
        );

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_surfaces_warning_when_status_jobmanager_url_is_outside_trusted_origin() {
        let trusted = start_mock_json_server(vec![(
            "/jobs/overview".to_owned(),
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
        let untrusted = start_mock_json_server(vec![(
            "/jobs/overview".to_owned(),
            StatusCode::OK,
            json!({"jobs":[]}),
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
                      "namespace": "analytics"
                    },
                    "spec": {
                      "job": {"name": "orders-stream"}
                    },
                    "status": {
                      "jobStatus": {"state": "READY"},
                      "jobManagerUrl": format!("{}/orders-stream/", untrusted.base_url)
                    }
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
        let app = start_app(live_config(&kubernetes.base_url, Some(&trusted.base_url))).await;
        let client = Client::new();

        let jobs_response = authorized(&app, client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("jobs response should succeed");
        assert_eq!(jobs_response.status(), StatusCode::OK);
        let jobs_payload: Value = jobs_response
            .json()
            .await
            .expect("jobs payload should be JSON");

        assert_eq!(jobs_payload["jobs"][0]["flinkJobId"], "job-123");
        assert_eq!(
            jobs_payload["jobs"][0]["warnings"],
            json!(["Flink REST enrichment ignored a JobManager URL outside the trusted origin"])
        );

        app.shutdown();
        kubernetes.shutdown();
        untrusted.shutdown();
        trusted.shutdown();
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

        let response = authorized(&app, client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Failed to list jobs");
        assert_eq!(
            payload["details"],
            "The upstream request failed; check server logs for details"
        );
        assert!(
            !payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("Kubernetes API 403")
        );
        assert!(
            !payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("/apis/flink.apache.org")
        );
        assert!(
            !payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("forbidden")
        );

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_sanitizes_network_failures_from_job_list_responses() {
        let app = start_app(live_config(&unused_local_url(), None)).await;
        let client = Client::new();

        let response = authorized(&app, client.get(format!("{}/api/jobs", app.base_url)))
            .send()
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Failed to list jobs");
        assert_eq!(
            payload["details"],
            "The server could not complete the request; check server logs for details"
        );
        assert!(
            !payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("Connection refused")
        );
        assert!(
            !payload["details"]
                .as_str()
                .expect("details should be a string")
                .contains("failed to fetch")
        );

        app.shutdown();
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
        assert!(detail_payload["job"]["details"].get("metadata").is_none());
        assert!(detail_payload["job"]["details"].get("spec").is_none());
        assert!(detail_payload["job"]["details"].get("status").is_none());
        assert_eq!(detail_payload["job"]["actions"]["cancel"]["enabled"], false);
        assert_eq!(
            detail_payload["job"]["actions"]["cancel"]["reason"],
            "Actions are unavailable in fixture mode"
        );

        let action_response = client
            .post(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/suspend",
                app.base_url
            ))
            .send()
            .await
            .expect("fixture action response should succeed");
        assert_eq!(action_response.status(), StatusCode::CONFLICT);
        let action_payload: Value = action_response
            .json()
            .await
            .expect("fixture action payload should be JSON");
        assert_eq!(action_payload["error"], "Action unavailable");

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
        assert_eq!(metrics_payload["requestsTotal"], 7);
        assert_eq!(metrics_payload["errorsTotal"], 1);
        assert_eq!(
            metrics_payload["routes"],
            json!({
              "jobAction": 1,
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
    async fn auth_login_redirects_after_successful_discovery() {
        let kubernetes = start_empty_kubernetes_server().await;
        let oidc = start_mock_json_server(vec![(
            "/.well-known/openid-configuration".to_owned(),
            StatusCode::OK,
            json!({
              "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
              "token_endpoint": "https://oauth2.googleapis.com/token",
              "userinfo_endpoint": "https://openidconnect.googleapis.com/v1/userinfo"
            }),
        )])
        .await;
        let app = start_app(live_config_with_oidc(
            &kubernetes.base_url,
            None,
            oidc_config(&oidc.base_url, "http://localhost"),
        ))
        .await;
        let client = client_without_redirects();

        let response = client
            .get(format!("{}/auth/login", app.base_url))
            .send()
            .await
            .expect("login response should succeed");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("login redirect should include a location header");
        assert!(location.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(location.contains("response_type=code"));
        assert!(location.contains("client_id=client-id"));
        assert!(location.contains("redirect_uri=http%3A%2F%2Flocalhost%2Fauth%2Fcallback"));
        assert!(
            response
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .any(|value| value
                    .to_str()
                    .expect("set-cookie should be text")
                    .contains("test-auth-flow="))
        );

        app.shutdown();
        oidc.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn auth_login_returns_bad_gateway_when_discovery_fetch_fails() {
        let kubernetes = start_empty_kubernetes_server().await;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let unavailable_issuer = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let app = start_app(live_config_with_oidc(
            &kubernetes.base_url,
            None,
            oidc_config(&unavailable_issuer, "http://localhost"),
        ))
        .await;
        let client = client_without_redirects();

        let response = client
            .get(format!("{}/auth/login", app.base_url))
            .send()
            .await
            .expect("login response should succeed");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(
            payload["error"],
            "OIDC discovery is unavailable; check server logs for details"
        );

        app.shutdown();
        kubernetes.shutdown();
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
    async fn live_mode_requires_session_for_protected_routes() {
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

        for path in [
            "/api/jobs",
            "/api/clusters",
            "/api/jobs/demo:analytics:FlinkDeployment:orders-stream/jobmanager-proxy/",
        ] {
            let response = client
                .get(format!("{}{}", app.base_url, path))
                .send()
                .await
                .expect("unauthorized response should succeed");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let payload: Value = response.json().await.expect("payload should be JSON");
            assert_eq!(payload, json!({"error":"Missing or expired session"}));
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

        let action_response = client
            .post(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/suspend",
                app.base_url
            ))
            .send()
            .await
            .expect("action unauthorized response should succeed");
        assert_eq!(action_response.status(), StatusCode::UNAUTHORIZED);

        let healthz_response = client
            .get(format!("{}/healthz", app.base_url))
            .send()
            .await
            .expect("healthz response should succeed");
        assert_eq!(healthz_response.status(), StatusCode::OK);

        let metrics_response = client
            .get(format!("{}/metrics", app.base_url))
            .send()
            .await
            .expect("metrics response should succeed");
        assert_eq!(metrics_response.status(), StatusCode::OK);

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_action_route_updates_resource_and_returns_refreshed_job() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "running"}},
                    "status": {"jobStatus": {"state": "RUNNING"}}
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
            MethodJsonResponse::new(
                Method::PATCH,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments/orders-stream",
                StatusCode::OK,
                json!({}),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "suspended"}},
                    "status": {"jobStatus": {"state": "SUSPENDED"}}
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.post(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/suspend",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("action response should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value = response
            .json()
            .await
            .expect("action payload should be JSON");
        assert_eq!(payload["action"], "suspend");
        assert_eq!(payload["deleted"], false);
        assert_eq!(payload["job"]["status"], "suspended");
        assert_eq!(payload["job"]["actions"]["resume"]["enabled"], true);

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_action_route_returns_conflict_for_disabled_actions() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({"items":[]}),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkSessionJob",
                    "metadata": {"name": "settlement-job", "namespace": "analytics"},
                    "spec": {"job": {"state": "suspended", "jarURI": "local:///example.jar"}},
                    "status": {"jobStatus": {"state": "SUSPENDED"}}
                  }]
                }),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.post(format!(
                "{}/api/jobs/demo/analytics/FlinkSessionJob/settlement-job/actions/suspend",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("action response should succeed");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Action not allowed");
        assert_eq!(payload["details"], "Already suspended");

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_resume_allows_suspended_resources_with_missing_jobmanager_signals() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "suspended"}},
                    "status": {
                      "jobStatus": {"state": "FINISHED"},
                      "lifecycleState": "SUSPENDED",
                      "jobManagerDeploymentStatus": "MISSING",
                      "reconciliationStatus": {"state": "DEPLOYED"}
                    }
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
            MethodJsonResponse::new(
                Method::PATCH,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments/orders-stream",
                StatusCode::OK,
                json!({}),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "running"}},
                    "status": {
                      "jobStatus": {"state": "RUNNING"},
                      "lifecycleState": "READY",
                      "reconciliationStatus": {"state": "READY"}
                    }
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.post(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/resume",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("resume response should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value = response
            .json()
            .await
            .expect("resume payload should be JSON");
        assert_eq!(payload["action"], "resume");
        assert_eq!(payload["deleted"], false);
        assert_eq!(payload["job"]["status"], "running");

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_action_route_returns_not_found_for_unknown_cluster() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({"items":[]}),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.post(format!(
                "{}/api/jobs/prod/analytics/FlinkDeployment/orders-stream/actions/suspend",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("action response should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Cluster not found");
        assert_eq!(payload["details"], "The specified cluster does not exist");

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_action_route_rejects_missing_csrf_token() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "running"}},
                    "status": {"jobStatus": {"state": "RUNNING"}}
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized_without_csrf(
            &app,
            client.post(format!(
                "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/suspend",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("action response should succeed");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Invalid CSRF token");

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_action_route_rejects_invalid_csrf_token() {
        let kubernetes = start_stateful_json_server(vec![
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments",
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {"name": "orders-stream", "namespace": "analytics"},
                    "spec": {"job": {"name": "orders-stream", "state": "running"}},
                    "status": {"jobStatus": {"state": "RUNNING"}}
                  }]
                }),
            ),
            MethodJsonResponse::new(
                Method::GET,
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs",
                StatusCode::OK,
                json!({"items":[]}),
            ),
        ])
        .await;
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized_without_csrf(
            &app,
            client
                .post(format!(
                    "{}/api/jobs/demo/analytics/FlinkDeployment/orders-stream/actions/suspend",
                    app.base_url
                ))
                .header("x-csrf-token", "wrong-token"),
        )
        .send()
        .await
        .expect("action response should succeed");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "Invalid CSRF token");

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_serves_shell_assets_without_auth_headers() {
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

        let page_response = client
            .get(format!("{}/", app.base_url))
            .send()
            .await
            .expect("page response should succeed");
        assert_eq!(page_response.status(), StatusCode::OK);
        assert_eq!(
            page_response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static(
                super::SHELL_HTML_CACHE_CONTROL
            ))
        );
        let page_html = page_response.text().await.expect("page HTML should load");
        assert!(page_html.contains("Jobs + Status Dashboard"));
        let asset_version = extract_version(&page_html, "/app.js?v=");
        assert!(page_html.contains(&format!("/styles.css?v={asset_version}")));

        let app_js_response = client
            .get(format!("{}/app.js?v={asset_version}", app.base_url))
            .send()
            .await
            .expect("app.js response should succeed");
        assert_eq!(app_js_response.status(), StatusCode::OK);
        assert_eq!(
            app_js_response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static(
                super::VERSIONED_SHELL_ASSET_CACHE_CONTROL
            ))
        );
        let app_js = app_js_response.text().await.expect("app.js should load");
        assert!(app_js.contains("loadJobs"));
        assert!(app_js.contains(&format!("./render.js?v={asset_version}")));

        let styles_response = client
            .get(format!("{}/styles.css?v={asset_version}", app.base_url))
            .send()
            .await
            .expect("styles.css response should succeed");
        assert_eq!(styles_response.status(), StatusCode::OK);
        assert_eq!(
            styles_response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static(
                super::VERSIONED_SHELL_ASSET_CACHE_CONTROL
            ))
        );
        let styles = styles_response
            .text()
            .await
            .expect("styles.css should load");
        assert!(styles.contains(".page-header"));

        let render_response = client
            .get(format!("{}/render.js?v={asset_version}", app.base_url))
            .send()
            .await
            .expect("render.js response should succeed");
        assert_eq!(render_response.status(), StatusCode::OK);
        assert_eq!(
            render_response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static(
                super::VERSIONED_SHELL_ASSET_CACHE_CONTROL
            ))
        );
        let render_js = render_response.text().await.expect("render.js should load");
        assert!(render_js.contains("renderTable"));

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_unauthorized_api_routes_return_json_without_redirect_headers() {
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

        for path in [
            "/api/jobs",
            "/api/clusters",
            "/api/jobs/demo/analytics/FlinkDeployment/orders-stream",
            "/api/jobs/demo:analytics:FlinkDeployment:orders-stream/jobmanager-proxy/",
        ] {
            let response = client
                .get(format!("{}{}", app.base_url, path))
                .send()
                .await
                .expect("unauthorized response should succeed");

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(
                response.headers().get(reqwest::header::LOCATION).is_none(),
                "unauthorized API route should not redirect: {path}"
            );
            assert!(
                response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("application/json")),
                "unauthorized API route should return JSON: {path}"
            );

            let body = response.text().await.expect("body should load");
            assert!(body.contains("Missing or expired session"));
            assert!(!body.contains("<html"));
        }

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_proxy_route_streams_html_assets_and_rewrites_redirects() {
        let flink = start_mock_http_server(vec![
            MockHttpResponse::html(
                "/orders-stream/",
                StatusCode::OK,
                "<html><body><script src=\"assets/main.js\"></script></body></html>",
            ),
            MockHttpResponse::text(
                "/orders-stream/assets/main.js",
                StatusCode::OK,
                "application/javascript",
                "console.log('ok');",
            ),
            MockHttpResponse::redirect(
                "/orders-stream/redirect",
                StatusCode::FOUND,
                "/orders-stream/assets/main.js",
            )
            .with_header(header::SET_COOKIE, "leaked=1; Path=/"),
        ])
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
                      "namespace": "analytics"
                    },
                    "status": {
                      "jobManagerUrl": format!("{}/orders-stream/", flink.base_url),
                      "jobStatus": {"state": "RUNNING"}
                    }
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
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = client_without_redirects();
        let proxy_root = format!(
            "{}/api/jobs/demo:analytics:FlinkDeployment:orders-stream/jobmanager-proxy/",
            app.base_url
        );

        let root_response = authorized(&app, client.get(&proxy_root))
            .send()
            .await
            .expect("proxy root response should succeed");
        assert_eq!(root_response.status(), StatusCode::OK);
        assert!(root_response.headers().get(header::SET_COOKIE).is_none());
        let root_body = root_response
            .text()
            .await
            .expect("proxy root body should load");
        assert!(root_body.contains("assets/main.js"));

        let asset_response = authorized(&app, client.get(format!("{proxy_root}assets/main.js")))
            .send()
            .await
            .expect("proxy asset response should succeed");
        assert_eq!(asset_response.status(), StatusCode::OK);
        assert_eq!(
            asset_response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/javascript")
        );
        let asset_body = asset_response
            .text()
            .await
            .expect("proxy asset body should load");
        assert!(asset_body.contains("console.log('ok');"));

        let redirect_response = authorized(&app, client.get(format!("{proxy_root}redirect")))
            .send()
            .await
            .expect("proxy redirect response should succeed");
        assert_eq!(redirect_response.status(), StatusCode::FOUND);
        assert_eq!(
            redirect_response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(
                "/api/jobs/demo%3Aanalytics%3AFlinkDeployment%3Aorders-stream/jobmanager-proxy/assets/main.js"
            )
        );
        assert!(
            redirect_response
                .headers()
                .get(header::SET_COOKIE)
                .is_none()
        );

        app.shutdown();
        kubernetes.shutdown();
        flink.shutdown();
    }

    #[tokio::test]
    async fn live_mode_proxy_route_fails_clearly_when_jobmanager_url_is_missing() {
        let kubernetes = start_mock_json_server(vec![
            (
                "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
                StatusCode::OK,
                json!({
                  "items": [{
                    "kind": "FlinkDeployment",
                    "metadata": {
                      "name": "orders-stream",
                      "namespace": "analytics"
                    },
                    "status": {
                      "jobStatus": {"state": "RUNNING"}
                    }
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
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.get(format!(
                "{}/api/jobs/demo:analytics:FlinkDeployment:orders-stream/jobmanager-proxy/",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("proxy missing-url response should succeed");

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let payload: Value = response.json().await.expect("payload should be JSON");
        assert_eq!(payload["error"], "JobManager URL unavailable");
        assert!(
            payload["details"]
                .as_str()
                .is_some_and(|details| details.contains("does not expose"))
        );

        app.shutdown();
        kubernetes.shutdown();
    }

    #[tokio::test]
    async fn live_mode_proxy_route_rejects_mutating_methods() {
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
        let app = start_app(live_config(&kubernetes.base_url, None)).await;
        let client = Client::new();

        let response = authorized(
            &app,
            client.post(format!(
                "{}/api/jobs/demo:analytics:FlinkDeployment:orders-stream/jobmanager-proxy/",
                app.base_url
            )),
        )
        .send()
        .await
        .expect("proxy method rejection response should succeed");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        app.shutdown();
        kubernetes.shutdown();
    }

    struct RunningServer {
        base_url: String,
        session_cookie: Option<String>,
        session_csrf_token: Option<String>,
        task: JoinHandle<()>,
    }

    impl RunningServer {
        fn shutdown(self) {
            self.task.abort();
        }
    }

    async fn start_app(config: AppConfig) -> RunningServer {
        let state = AppState::new(config);
        let session = if state.config.fixture_mode {
            None
        } else {
            Some(
                state
                    .auth
                    .issue_test_session(&state.config)
                    .await
                    .expect("test session should be created"),
            )
        };
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
            session_cookie: session.as_ref().map(|(cookie, _)| cookie.clone()),
            session_csrf_token: session.as_ref().map(|(_, csrf_token)| csrf_token.clone()),
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
            session_cookie: None,
            session_csrf_token: None,
            task,
        }
    }

    async fn start_stateful_json_server(responses: Vec<MethodJsonResponse>) -> RunningServer {
        let responses = Arc::new(Mutex::new(responses.into_iter().fold(
            BTreeMap::<String, Vec<(StatusCode, Value)>>::new(),
            |mut entries, response| {
                entries
                    .entry(response.key())
                    .or_default()
                    .push((response.status, response.payload));
                entries
            },
        )));
        let app = Router::new().fallback(any({
            let responses = Arc::clone(&responses);
            move |request: Request| {
                let responses = Arc::clone(&responses);
                async move {
                    let key = format!("{} {}", request.method(), request.uri().path());
                    let _ = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("request body should read");
                    let mut responses = responses.lock().await;
                    match responses.get_mut(&key).and_then(|values| {
                        if values.is_empty() {
                            None
                        } else {
                            Some(values.remove(0))
                        }
                    }) {
                        Some((status, payload)) => (status, Json(payload)).into_response(),
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
                .expect("stateful mock server should run");
        });

        RunningServer {
            base_url: format!("http://{}", address),
            session_cookie: None,
            session_csrf_token: None,
            task,
        }
    }

    async fn start_mock_http_server(responses: Vec<MockHttpResponse>) -> RunningServer {
        let responses = Arc::new(
            responses
                .into_iter()
                .map(|response| (response.path.clone(), response))
                .collect::<BTreeMap<String, MockHttpResponse>>(),
        );
        let app = Router::new().fallback({
            let responses = Arc::clone(&responses);
            move |uri: OriginalUri| {
                let responses = Arc::clone(&responses);
                async move {
                    match responses.get(uri.path()) {
                        Some(response) => {
                            let mut builder =
                                axum::response::Response::builder().status(response.status);
                            for (name, value) in &response.headers {
                                builder = builder.header(name, value);
                            }
                            builder
                                .body(axum::body::Body::from(response.body.clone()))
                                .expect("mock response should build")
                        }
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
            session_cookie: None,
            session_csrf_token: None,
            task,
        }
    }

    async fn start_empty_kubernetes_server() -> RunningServer {
        start_mock_json_server(vec![(
            "/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments".to_owned(),
            StatusCode::OK,
            json!({"items":[]}),
        )])
        .await
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
            oidc_request_timeout_ms: 15_000,
            oidc: None,
            session: test_session_config(),
            allow_loopback_jobmanager_targets: true,
            clusters: Vec::new(),
        }
    }

    fn live_config(kubernetes_base_url: &str, flink_base_url: Option<&str>) -> AppConfig {
        live_config_with_oidc(
            kubernetes_base_url,
            flink_base_url,
            oidc_config("https://issuer.example.com", "http://localhost"),
        )
    }

    fn live_config_with_oidc(
        kubernetes_base_url: &str,
        flink_base_url: Option<&str>,
        oidc: OidcConfig,
    ) -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 0,
            root_dir: workspace_root(),
            fixture_mode: false,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            oidc_request_timeout_ms: 15_000,
            oidc: Some(oidc),
            session: test_session_config(),
            allow_loopback_jobmanager_targets: true,
            clusters: vec![ClusterConfig {
                name: "demo".to_owned(),
                api_url: kubernetes_base_url.to_owned(),
                bearer_token: "token".to_owned(),
                ca_cert: None,
                insecure_skip_tls_verify: false,
                namespaces: vec!["analytics".to_owned()],
                flink_api_version: "v1beta1".to_owned(),
                derive_jobmanager_url_in_cluster: false,
                flink_rest_base_url: flink_base_url.map(ToOwned::to_owned),
            }],
        }
    }

    fn oidc_config(issuer_url: &str, external_base_url: &str) -> OidcConfig {
        OidcConfig {
            issuer_url: issuer_url.to_owned(),
            client_id: "client-id".to_owned(),
            client_secret: "client-secret".to_owned(),
            external_base_url: external_base_url.to_owned(),
            callback_path: "/auth/callback".to_owned(),
            scopes: vec![
                "openid".to_owned(),
                "profile".to_owned(),
                "email".to_owned(),
            ],
        }
    }

    fn test_session_config() -> crate::config::SessionConfig {
        crate::config::SessionConfig {
            cookie_name: "test-session".to_owned(),
            auth_flow_cookie_name: "test-auth-flow".to_owned(),
            cookie_secret: "0123456789abcdef0123456789abcdef".to_owned(),
            ttl_secs: 60,
            auth_flow_ttl_secs: 60,
            secure_cookie: false,
        }
    }

    fn authorized(app: &RunningServer, request: RequestBuilder) -> RequestBuilder {
        if let Some(cookie) = &app.session_cookie {
            let request = request.header(reqwest::header::COOKIE, cookie);
            if let Some(csrf_token) = &app.session_csrf_token {
                request.header("x-csrf-token", csrf_token)
            } else {
                request
            }
        } else {
            request
        }
    }

    fn authorized_without_csrf(app: &RunningServer, request: RequestBuilder) -> RequestBuilder {
        if let Some(cookie) = &app.session_cookie {
            request.header(reqwest::header::COOKIE, cookie)
        } else {
            request
        }
    }

    fn client_without_redirects() -> Client {
        Client::builder()
            .redirect(Policy::none())
            .build()
            .expect("client should build")
    }

    fn extract_version(body: &str, marker: &str) -> String {
        let start = body
            .find(marker)
            .map(|index| index + marker.len())
            .expect("marker should exist");
        let suffix = &body[start..];
        let end = suffix
            .find('"')
            .expect("versioned asset URL should end with quote");
        suffix[..end].to_owned()
    }

    fn unused_local_url() -> String {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("unused port should bind");
        let address = listener
            .local_addr()
            .expect("unused port should have local address");
        drop(listener);
        format!("http://{}", address)
    }

    #[derive(Clone)]
    struct MockHttpResponse {
        path: String,
        status: StatusCode,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl MockHttpResponse {
        fn html(path: &str, status: StatusCode, body: &str) -> Self {
            Self::text(path, status, "text/html; charset=utf-8", body)
        }

        fn text(path: &str, status: StatusCode, content_type: &str, body: &str) -> Self {
            Self {
                path: path.to_owned(),
                status,
                headers: vec![(
                    header::CONTENT_TYPE.as_str().to_owned(),
                    content_type.to_owned(),
                )],
                body: body.to_owned(),
            }
        }

        fn redirect(path: &str, status: StatusCode, location: &str) -> Self {
            Self {
                path: path.to_owned(),
                status,
                headers: vec![(header::LOCATION.as_str().to_owned(), location.to_owned())],
                body: String::new(),
            }
        }

        fn with_header(mut self, name: header::HeaderName, value: &str) -> Self {
            self.headers
                .push((name.as_str().to_owned(), value.to_owned()));
            self
        }
    }

    struct MethodJsonResponse {
        method: Method,
        path: String,
        status: StatusCode,
        payload: Value,
    }

    impl MethodJsonResponse {
        fn new(method: Method, path: &str, status: StatusCode, payload: Value) -> Self {
            Self {
                method,
                path: path.to_owned(),
                status,
                payload,
            }
        }

        fn key(&self) -> String {
            format!("{} {}", self.method, self.path)
        }
    }

    fn load_fixture_jobs() -> Vec<Value> {
        let fixture_path = workspace_root().join("fixtures/jobs.json");
        let mut payload: Value = serde_json::from_str(
            &fs::read_to_string(fixture_path).expect("fixture file should be readable"),
        )
        .expect("fixture JSON should parse");
        if let Some(jobs) = payload["jobs"].as_array_mut() {
            for job in jobs {
                for action_name in ["cancel", "suspend", "resume"] {
                    if let Some(action) = job["actions"].get_mut(action_name) {
                        action["enabled"] = json!(false);
                        action["reason"] = json!("Actions are unavailable in fixture mode");
                    }
                }
            }
        }
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

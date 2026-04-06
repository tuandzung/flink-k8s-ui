use std::borrow::Cow;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{OriginalUri, Path, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use serde::Serialize;
use url::Host;

use crate::domain::job::Job;
use crate::state::AppState;

const ROOT_SENTINEL: &str = "@root";

#[derive(Serialize)]
struct ProxyErrorResponse {
    error: String,
    details: String,
}

pub async fn proxy_jobmanager_root(
    State(state): State<AppState>,
    Path(id): Path<String>,
    method: Method,
    headers: HeaderMap,
    original_uri: OriginalUri,
) -> Response {
    proxy_jobmanager_request(state, id, None, method, headers, original_uri).await
}

pub async fn proxy_jobmanager_path(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    original_uri: OriginalUri,
) -> Response {
    proxy_jobmanager_request(state, id, Some(path), method, headers, original_uri).await
}

async fn proxy_jobmanager_request(
    state: AppState,
    id: String,
    path: Option<String>,
    method: Method,
    headers: HeaderMap,
    original_uri: OriginalUri,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return proxy_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "JobManager proxy only supports GET and HEAD requests",
            "Mutating or upgrade methods are out of scope for this read-only proxy",
        );
    }

    let job = match state.jobs_service.get_job_by_id(&id).await {
        Ok(Some(job)) => job,
        Ok(None) => {
            return proxy_error(StatusCode::NOT_FOUND, "Job not found", "Job not found");
        }
        Err(error) => {
            return proxy_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch job",
                &error.to_string(),
            );
        }
    };

    let upstream_base = match parse_jobmanager_url(&job, cfg!(test)) {
        Ok(url) => url,
        Err(details) => {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                "JobManager URL unavailable",
                &details,
            );
        }
    };

    let upstream_url =
        match build_upstream_url(&upstream_base, path.as_deref(), original_uri.0.query()) {
            Ok(url) => url,
            Err(details) => {
                return proxy_error(
                    StatusCode::BAD_REQUEST,
                    "Rejected JobManager proxy path",
                    &details,
                );
            }
        };

    let client = match Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_millis(state.config.request_timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return proxy_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to initialize JobManager proxy",
                &error.to_string(),
            );
        }
    };

    let upstream_response =
        match forward_request(&client, &method, &headers, upstream_url.clone()).await {
            Ok(response) => response,
            Err(error) => {
                return proxy_error(
                    StatusCode::BAD_GATEWAY,
                    "Failed to reach JobManager UI",
                    &error,
                );
            }
        };

    let proxy_prefix = proxy_prefix(&id);
    map_upstream_response(upstream_response, &upstream_base, &proxy_prefix).await
}

async fn forward_request(
    client: &Client,
    method: &Method,
    headers: &HeaderMap,
    upstream_url: Url,
) -> Result<reqwest::Response, String> {
    let mut upstream_request = client.request(method.clone(), upstream_url);
    for header_name in [
        header::ACCEPT,
        header::ACCEPT_LANGUAGE,
        header::CACHE_CONTROL,
        header::IF_MATCH,
        header::IF_MODIFIED_SINCE,
        header::IF_NONE_MATCH,
        header::IF_UNMODIFIED_SINCE,
        header::USER_AGENT,
    ] {
        if let Some(value) = headers.get(&header_name) {
            upstream_request = upstream_request.header(&header_name, value.clone());
        }
    }

    upstream_request
        .send()
        .await
        .map_err(|error| error.to_string())
}

async fn map_upstream_response(
    upstream_response: reqwest::Response,
    upstream_base: &Url,
    proxy_prefix: &str,
) -> Response {
    let status = upstream_response.status();
    let upstream_headers = upstream_response.headers().clone();

    if status.is_redirection() {
        let Some(location) = upstream_headers.get(header::LOCATION) else {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                "Rejected JobManager redirect",
                "Upstream redirect did not include a location header",
            );
        };

        let location = match location.to_str() {
            Ok(location) => location,
            Err(_) => {
                return proxy_error(
                    StatusCode::BAD_GATEWAY,
                    "Rejected JobManager redirect",
                    "Upstream redirect location was not valid UTF-8",
                );
            }
        };

        let rewritten = match rewrite_location(location, upstream_base, proxy_prefix) {
            Ok(location) => location,
            Err(details) => {
                return proxy_error(
                    StatusCode::BAD_GATEWAY,
                    "Rejected JobManager redirect",
                    &details,
                );
            }
        };

        let mut response = Response::builder().status(status);
        response = response.header(header::LOCATION, rewritten);
        return response
            .body(Body::empty())
            .expect("redirect response should build");
    }

    let content_type = upstream_headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);

    let body = match upstream_response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                "Failed to read JobManager response",
                &error.to_string(),
            );
        }
    };

    let body = maybe_rewrite_body(content_type.as_deref(), &body, proxy_prefix);

    let mut response = Response::builder().status(status);
    copy_response_headers(&mut response, &upstream_headers, content_type.as_deref());
    response
        .body(match body {
            Cow::Borrowed(bytes) => Body::from(bytes.to_vec()),
            Cow::Owned(bytes) => Body::from(bytes),
        })
        .expect("proxy response should build")
}

fn copy_response_headers(
    builder: &mut axum::http::response::Builder,
    upstream_headers: &HeaderMap,
    content_type: Option<&str>,
) {
    for (name, value) in upstream_headers {
        if name == header::CONTENT_LENGTH
            || name == header::LOCATION
            || name == header::SET_COOKIE
            || name == header::CONNECTION
            || name.as_str().eq_ignore_ascii_case("keep-alive")
            || name.as_str().eq_ignore_ascii_case("proxy-authenticate")
            || name.as_str().eq_ignore_ascii_case("proxy-authorization")
            || name.as_str().eq_ignore_ascii_case("te")
            || name.as_str().eq_ignore_ascii_case("trailers")
            || name.as_str().eq_ignore_ascii_case("transfer-encoding")
            || name.as_str().eq_ignore_ascii_case("upgrade")
        {
            continue;
        }

        builder
            .headers_mut()
            .expect("headers should exist")
            .append(name, value.clone());
    }

    if let Some(content_type) = content_type {
        if let Ok(content_type) = HeaderValue::from_str(content_type) {
            builder
                .headers_mut()
                .expect("headers should exist")
                .insert(header::CONTENT_TYPE, content_type);
        }
    }
}

fn maybe_rewrite_body<'a>(
    content_type: Option<&str>,
    body: &'a [u8],
    proxy_prefix: &str,
) -> Cow<'a, [u8]> {
    let Some(content_type) = content_type else {
        return Cow::Borrowed(body);
    };

    let is_rewritable = content_type.starts_with("text/html")
        || content_type.starts_with("text/css")
        || content_type.starts_with("application/javascript")
        || content_type.starts_with("text/javascript");
    if !is_rewritable {
        return Cow::Borrowed(body);
    }

    let Ok(text) = std::str::from_utf8(body) else {
        return Cow::Borrowed(body);
    };

    let root_prefix = format!("{proxy_prefix}/{ROOT_SENTINEL}/");
    let rewritten = text
        .replace("href=\"/", &format!("href=\"{root_prefix}"))
        .replace("src=\"/", &format!("src=\"{root_prefix}"))
        .replace("action=\"/", &format!("action=\"{root_prefix}"))
        .replace("content=\"/", &format!("content=\"{root_prefix}"))
        .replace("url(/", &format!("url({root_prefix}"));

    if rewritten == text {
        Cow::Borrowed(body)
    } else {
        Cow::Owned(rewritten.into_bytes())
    }
}

fn parse_jobmanager_url(job: &Job, allow_loopback: bool) -> Result<Url, String> {
    let native_ui_url = job
        .native_ui_url
        .as_deref()
        .ok_or_else(|| "The selected job does not expose a JobManager URL".to_owned())?;
    let url = normalize_jobmanager_url(native_ui_url)?;
    validate_jobmanager_url(&url, allow_loopback)?;
    Ok(url)
}

fn normalize_jobmanager_url(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value).map_err(|error| error.to_string())?;
    if !url.path().ends_with('/') {
        let normalized_path = format!("{}/", url.path());
        url.set_path(&normalized_path);
    }
    Ok(url)
}

fn validate_jobmanager_url(url: &Url, allow_loopback: bool) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https JobManager URLs are supported".to_owned());
    }

    let Some(host) = url.host() else {
        return Err("JobManager URL is missing a host".to_owned());
    };

    if allow_loopback {
        return Ok(());
    }

    match host {
        Host::Ipv4(ip) => {
            if ip.is_loopback() || ip.is_link_local() {
                return Err("Loopback and link-local JobManager targets are not allowed".to_owned());
            }
        }
        Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unicast_link_local() {
                return Err("Loopback and link-local JobManager targets are not allowed".to_owned());
            }
        }
        Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost") || domain.ends_with(".localhost") {
                return Err("Loopback and link-local JobManager targets are not allowed".to_owned());
            }
        }
    }

    Ok(())
}

fn build_upstream_url(base: &Url, path: Option<&str>, query: Option<&str>) -> Result<Url, String> {
    let path = path.unwrap_or_default();

    if has_dot_segments(path) {
        return Err("Dot-segment traversal is not allowed".to_owned());
    }

    let mut url = if path.is_empty() {
        base.clone()
    } else if path == ROOT_SENTINEL || path.starts_with(&format!("{ROOT_SENTINEL}/")) {
        origin_relative_upstream(base, path.strip_prefix(ROOT_SENTINEL).unwrap_or_default())
    } else {
        base.join(path).map_err(|error| error.to_string())?
    };

    url.set_query(query);
    Ok(url)
}

fn origin_relative_upstream(base: &Url, suffix: &str) -> Url {
    let mut url = base.clone();
    let path = format!("/{}", suffix.trim_start_matches('/'));
    url.set_path(&path);
    url
}

fn rewrite_location(
    location: &str,
    upstream_base: &Url,
    proxy_prefix: &str,
) -> Result<String, String> {
    let target = upstream_base
        .join(location)
        .map_err(|error| error.to_string())?;
    if !same_origin(&target, upstream_base) {
        return Err("Cross-origin redirect escapes are not allowed".to_owned());
    }

    let mut rewritten = if let Some(relative) = target.path().strip_prefix(upstream_base.path()) {
        if relative.is_empty() {
            format!("{proxy_prefix}/")
        } else {
            format!("{proxy_prefix}/{}", relative.trim_start_matches('/'))
        }
    } else {
        format!("{proxy_prefix}/{ROOT_SENTINEL}{}", target.path())
    };
    if let Some(query) = target.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }
    if let Some(fragment) = target.fragment() {
        rewritten.push('#');
        rewritten.push_str(fragment);
    }
    Ok(rewritten)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn proxy_prefix(id: &str) -> String {
    format!(
        "/api/jobs/{}/jobmanager-proxy",
        url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
    )
}

fn has_dot_segments(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

fn proxy_error(status: StatusCode, error: &str, details: &str) -> Response {
    (
        status,
        Json(ProxyErrorResponse {
            error: error.to_owned(),
            details: details.to_owned(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        ROOT_SENTINEL, build_upstream_url, maybe_rewrite_body, normalize_jobmanager_url,
        proxy_prefix, rewrite_location, validate_jobmanager_url,
    };
    use reqwest::Url;

    #[test]
    fn validate_jobmanager_url_rejects_loopback_when_not_explicitly_allowed() {
        let url = Url::parse("http://127.0.0.1:8081").expect("url should parse");
        let error =
            validate_jobmanager_url(&url, false).expect_err("loopback target should be rejected");
        assert!(error.contains("Loopback"));
    }

    #[test]
    fn build_upstream_url_supports_origin_relative_paths() {
        let base = Url::parse("http://jobmanager.cluster.local:8081/orders-stream/")
            .expect("url should parse");
        let upstream =
            build_upstream_url(&base, Some("@root/assets/app.js"), Some("v=1")).expect("build");
        assert_eq!(
            upstream.as_str(),
            "http://jobmanager.cluster.local:8081/assets/app.js?v=1"
        );
    }

    #[test]
    fn normalize_jobmanager_url_adds_a_trailing_slash_for_relative_asset_resolution() {
        let normalized =
            normalize_jobmanager_url("https://example.com/jobs/orders").expect("normalize");
        assert_eq!(normalized.as_str(), "https://example.com/jobs/orders/");
    }

    #[test]
    fn rewrite_location_maps_same_origin_redirects_back_to_proxy_prefix() {
        let upstream = Url::parse("http://jobmanager.cluster.local:8081/orders-stream/")
            .expect("url should parse");
        let proxy = proxy_prefix("demo:analytics:FlinkDeployment:orders-stream");
        let rewritten =
            rewrite_location("/jobs/overview?tab=r", &upstream, &proxy).expect("rewrite");
        assert_eq!(
            rewritten,
            format!("{proxy}/{ROOT_SENTINEL}/jobs/overview?tab=r")
        );
    }

    #[test]
    fn maybe_rewrite_body_rewrites_root_absolute_references() {
        let proxy = proxy_prefix("demo");
        let body = r#"<link href="/assets/app.css"><img src="/assets/logo.svg"><style>body{background:url(/bg.png)}</style>"#;
        let rewritten = maybe_rewrite_body(Some("text/html"), body.as_bytes(), &proxy);
        let rewritten = String::from_utf8(rewritten.into_owned()).expect("utf8");
        assert!(rewritten.contains(&format!(r#"href="{proxy}/{ROOT_SENTINEL}/assets/app.css""#)));
        assert!(rewritten.contains(&format!(r#"src="{proxy}/{ROOT_SENTINEL}/assets/logo.svg""#)));
        assert!(rewritten.contains(&format!(r#"url({proxy}/{ROOT_SENTINEL}/bg.png)"#)));
    }
}

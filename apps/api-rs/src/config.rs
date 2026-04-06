use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const SERVICE_ACCOUNT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const SERVICE_ACCOUNT_CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
const DEFAULT_CALLBACK_PATH: &str = "/auth/callback";
const DEFAULT_SESSION_COOKIE_NAME: &str = "flink_job_ui_session";
const DEFAULT_AUTH_FLOW_COOKIE_NAME: &str = "flink_job_ui_auth_flow";
const DEFAULT_OIDC_REQUEST_TIMEOUT_MS: u64 = 15_000;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub root_dir: PathBuf,
    pub fixture_mode: bool,
    pub fixture_file: PathBuf,
    pub cache_ttl_ms: u64,
    pub request_timeout_ms: u64,
    pub oidc_request_timeout_ms: u64,
    pub oidc: Option<OidcConfig>,
    pub session: SessionConfig,
    pub clusters: Vec<ClusterConfig>,
}

#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub external_base_url: String,
    pub callback_path: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub auth_flow_cookie_name: String,
    pub cookie_secret: String,
    pub ttl_secs: i64,
    pub auth_flow_ttl_secs: i64,
    pub secure_cookie: bool,
}

#[derive(Clone, Debug)]
pub struct ClusterConfig {
    pub name: String,
    pub api_url: String,
    pub bearer_token: String,
    pub ca_cert: Option<String>,
    pub insecure_skip_tls_verify: bool,
    pub namespaces: Vec<String>,
    pub flink_api_version: String,
    #[allow(dead_code)]
    pub flink_rest_base_url: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let root_dir = detect_root_dir()?;
        let configured_clusters = normalize_clusters(parse_json::<Vec<RawClusterConfig>>(
            &env::var("FLINK_UI_CLUSTERS_JSON").ok(),
        )?);
        let clusters = if configured_clusters.is_empty() {
            default_cluster_from_env()?
        } else {
            configured_clusters
        };
        let fixture_mode = parse_bool(env::var("FIXTURE_MODE").ok().as_deref(), false);
        let fixture_file = root_dir
            .join(env::var("FIXTURE_FILE").unwrap_or_else(|_| "fixtures/jobs.json".to_owned()));
        let request_timeout_ms = env::var("REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4_000);
        let oidc = OidcConfig::from_env()?;
        let session = SessionConfig::from_env();

        let config = Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_owned()),
            port: env::var("PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3000),
            root_dir,
            fixture_mode,
            fixture_file,
            cache_ttl_ms: env::var("CACHE_TTL_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5_000),
            request_timeout_ms,
            oidc_request_timeout_ms: env::var("OIDC_REQUEST_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default_oidc_request_timeout_ms(request_timeout_ms)),
            oidc,
            session,
            clusters,
        };

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.fixture_mode && self.clusters.is_empty() {
            bail!("fixture mode is disabled but no Kubernetes clusters are configured")
        }

        if !self.fixture_mode {
            let Some(oidc) = &self.oidc else {
                bail!("fixture mode is disabled but OIDC settings are missing")
            };

            if oidc.scopes.is_empty() {
                bail!("OIDC scopes must include at least openid")
            }
            if self.session.secure_cookie && oidc.external_base_url.starts_with("http://") {
                bail!(
                    "SESSION_SECURE_COOKIE=true requires an https OIDC_EXTERNAL_BASE_URL; disable secure cookies only for local HTTP testing"
                )
            }
            if self.session.cookie_secret.len() < 32 {
                bail!("SESSION_COOKIE_SECRET must be at least 32 characters long")
            }
        }

        Ok(())
    }

    pub fn static_dir(&self) -> PathBuf {
        self.root_dir.join("apps/web/public")
    }

    pub fn index_html_path(&self) -> PathBuf {
        self.static_dir().join("index.html")
    }
    pub fn oidc_callback_url(&self) -> Option<String> {
        self.oidc
            .as_ref()
            .map(|oidc| format!("{}{}", oidc.external_base_url, oidc.callback_path))
    }
}

impl OidcConfig {
    fn from_env() -> Result<Option<Self>> {
        let issuer_url = env::var("OIDC_ISSUER_URL").ok();
        let client_id = env::var("OIDC_CLIENT_ID").ok();
        let client_secret = env::var("OIDC_CLIENT_SECRET").ok();
        let external_base_url = env::var("OIDC_EXTERNAL_BASE_URL").ok();

        if issuer_url.is_none()
            && client_id.is_none()
            && client_secret.is_none()
            && external_base_url.is_none()
        {
            return Ok(None);
        }

        Ok(Some(Self {
            issuer_url: normalize_base_url(
                &issuer_url.context("OIDC_ISSUER_URL is required when OIDC auth is enabled")?,
            )?,
            client_id: client_id.context("OIDC_CLIENT_ID is required when OIDC auth is enabled")?,
            client_secret: client_secret
                .context("OIDC_CLIENT_SECRET is required when OIDC auth is enabled")?,
            external_base_url: normalize_base_url(
                &external_base_url
                    .context("OIDC_EXTERNAL_BASE_URL is required when OIDC auth is enabled")?,
            )?,
            callback_path: normalize_callback_path(
                &env::var("OIDC_CALLBACK_PATH")
                    .unwrap_or_else(|_| DEFAULT_CALLBACK_PATH.to_owned()),
            ),
            scopes: parse_scopes(env::var("OIDC_SCOPES").ok()),
        }))
    }
}

impl SessionConfig {
    fn from_env() -> Self {
        Self {
            cookie_name: env::var("SESSION_COOKIE_NAME")
                .unwrap_or_else(|_| DEFAULT_SESSION_COOKIE_NAME.to_owned()),
            auth_flow_cookie_name: env::var("AUTH_FLOW_COOKIE_NAME")
                .unwrap_or_else(|_| DEFAULT_AUTH_FLOW_COOKIE_NAME.to_owned()),
            cookie_secret: env::var("SESSION_COOKIE_SECRET")
                .unwrap_or_else(|_| "dev-only-session-cookie-secret-change-me-please".to_owned()),
            ttl_secs: env::var("SESSION_TTL_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8 * 60 * 60),
            auth_flow_ttl_secs: env::var("AUTH_FLOW_TTL_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5 * 60),
            secure_cookie: parse_bool(env::var("SESSION_SECURE_COOKIE").ok().as_deref(), true),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClusterConfig {
    name: Option<String>,
    api_url: Option<String>,
    url: Option<String>,
    bearer_token: Option<String>,
    bearer_token_file: Option<String>,
    ca_cert: Option<String>,
    ca_cert_file: Option<String>,
    insecure_skip_tls_verify: Option<bool>,
    namespaces: Option<Vec<String>>,
    flink_api_version: Option<String>,
    flink_rest_base_url: Option<String>,
}

fn parse_bool(value: Option<&str>, default_value: bool) -> bool {
    match value {
        Some(value) if !value.is_empty() => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => default_value,
    }
}

fn parse_json<T>(value: &Option<String>) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match value {
        Some(raw) if !raw.is_empty() => {
            let parsed = serde_json::from_str(raw).context("invalid JSON configuration")?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn parse_scopes(value: Option<String>) -> Vec<String> {
    let scopes = value
        .unwrap_or_else(|| "openid profile email".to_owned())
        .split(|character: char| character == ' ' || character == ',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if scopes.is_empty() {
        vec![
            "openid".to_owned(),
            "profile".to_owned(),
            "email".to_owned(),
        ]
    } else {
        scopes
    }
}

fn default_oidc_request_timeout_ms(request_timeout_ms: u64) -> u64 {
    request_timeout_ms.max(DEFAULT_OIDC_REQUEST_TIMEOUT_MS)
}

fn normalize_base_url(value: &str) -> Result<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("base URL cannot be empty")
    }

    Ok(trimmed.to_owned())
}

fn normalize_callback_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        DEFAULT_CALLBACK_PATH.to_owned()
    } else if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

fn normalize_clusters(raw_clusters: Option<Vec<RawClusterConfig>>) -> Vec<ClusterConfig> {
    raw_clusters
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, cluster)| {
            let api_url = cluster.api_url.or(cluster.url)?;
            let bearer_token = cluster.bearer_token.or_else(|| {
                cluster
                    .bearer_token_file
                    .and_then(|path| read_if_exists(path))
            });

            bearer_token.map(|bearer_token| ClusterConfig {
                name: cluster
                    .name
                    .unwrap_or_else(|| format!("cluster-{}", index + 1)),
                api_url,
                bearer_token,
                ca_cert: cluster
                    .ca_cert
                    .or_else(|| cluster.ca_cert_file.and_then(|path| read_if_exists(path))),
                insecure_skip_tls_verify: cluster.insecure_skip_tls_verify.unwrap_or(false),
                namespaces: match cluster.namespaces {
                    Some(namespaces) if !namespaces.is_empty() => namespaces,
                    _ => vec!["default".to_owned()],
                },
                flink_api_version: cluster
                    .flink_api_version
                    .unwrap_or_else(|| "v1beta1".to_owned()),
                flink_rest_base_url: cluster.flink_rest_base_url,
            })
        })
        .collect()
}

fn default_cluster_from_env() -> Result<Vec<ClusterConfig>> {
    let host = env::var("KUBERNETES_SERVICE_HOST").ok();
    let port = env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_owned());
    let token = env::var("K8S_BEARER_TOKEN")
        .ok()
        .or_else(|| read_if_exists(SERVICE_ACCOUNT_TOKEN_PATH));
    let ca_cert = env::var("K8S_CA_CERT")
        .ok()
        .or_else(|| read_if_exists(SERVICE_ACCOUNT_CA_PATH));
    let api_url = env::var("K8S_API_URL")
        .ok()
        .or_else(|| host.map(|host| format!("https://{}:{}", host, port)));

    let Some(api_url) = api_url else {
        return Ok(Vec::new());
    };
    let Some(bearer_token) = token else {
        return Ok(Vec::new());
    };

    Ok(vec![ClusterConfig {
        name: env::var("K8S_CLUSTER_NAME").unwrap_or_else(|_| "in-cluster".to_owned()),
        api_url,
        bearer_token,
        ca_cert,
        insecure_skip_tls_verify: parse_bool(
            env::var("K8S_INSECURE_SKIP_TLS_VERIFY").ok().as_deref(),
            false,
        ),
        namespaces: env::var("WATCH_NAMESPACES")
            .unwrap_or_else(|_| "default".to_owned())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        flink_api_version: env::var("FLINK_K8S_API_VERSION")
            .unwrap_or_else(|_| "v1beta1".to_owned()),
        flink_rest_base_url: env::var("FLINK_REST_BASE_URL").ok(),
    }])
}

fn read_if_exists<P>(path: P) -> Option<String>
where
    P: AsRef<Path>,
{
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn detect_root_dir() -> Result<PathBuf> {
    let current_dir = env::current_dir().context("failed to read current working directory")?;

    for candidate in current_dir.ancestors() {
        if looks_like_repo_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }

    Ok(current_dir)
}

fn looks_like_repo_root(path: &Path) -> bool {
    path.join("package.json").is_file() && path.join("apps/web/public").is_dir()
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, ClusterConfig, OidcConfig, SessionConfig, default_oidc_request_timeout_ms,
        normalize_base_url, normalize_callback_path, parse_scopes,
    };
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate should be nested in apps/")
            .parent()
            .expect("workspace root should exist")
            .to_path_buf()
    }

    fn session_config() -> SessionConfig {
        SessionConfig {
            cookie_name: "session".to_owned(),
            auth_flow_cookie_name: "session-flow".to_owned(),
            cookie_secret: "0123456789abcdef0123456789abcdef".to_owned(),
            ttl_secs: 60,
            auth_flow_ttl_secs: 60,
            secure_cookie: false,
        }
    }

    #[test]
    fn parse_scopes_uses_openid_defaults() {
        assert_eq!(
            parse_scopes(None),
            vec![
                "openid".to_owned(),
                "profile".to_owned(),
                "email".to_owned()
            ]
        );
    }

    #[test]
    fn normalize_callback_path_always_has_leading_slash() {
        assert_eq!(normalize_callback_path("auth/callback"), "/auth/callback");
        assert_eq!(normalize_callback_path("/auth/callback"), "/auth/callback");
    }

    #[test]
    fn normalize_base_url_trims_trailing_slash() {
        assert_eq!(
            normalize_base_url("https://example.com/").expect("base url should normalize"),
            "https://example.com"
        );
    }

    #[test]
    fn oidc_timeout_default_is_more_forgiving_than_general_request_timeout() {
        assert_eq!(default_oidc_request_timeout_ms(1_000), 15_000);
        assert_eq!(default_oidc_request_timeout_ms(20_000), 20_000);
    }

    #[test]
    fn live_config_requires_clusters_and_oidc() {
        let config = AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 3000,
            root_dir: workspace_root(),
            fixture_mode: false,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            oidc_request_timeout_ms: 15_000,
            oidc: None,
            session: session_config(),
            clusters: Vec::new(),
        };

        let error = config.validate().expect_err("live mode should fail closed");
        assert!(!error.to_string().trim().is_empty());
    }

    #[test]
    fn fixture_config_can_skip_oidc() {
        let config = AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 3000,
            root_dir: workspace_root(),
            fixture_mode: true,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            oidc_request_timeout_ms: 15_000,
            oidc: None,
            session: session_config(),
            clusters: Vec::new(),
        };

        config.validate().expect("fixture mode should be allowed");
    }

    #[test]
    fn live_config_with_oidc_and_clusters_is_valid() {
        let config = AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 3000,
            root_dir: workspace_root(),
            fixture_mode: false,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            oidc_request_timeout_ms: 15_000,
            oidc: Some(OidcConfig {
                issuer_url: "https://issuer.example.com".to_owned(),
                client_id: "client-id".to_owned(),
                client_secret: "client-secret".to_owned(),
                external_base_url: "https://flink.example.com".to_owned(),
                callback_path: "/auth/callback".to_owned(),
                scopes: vec!["openid".to_owned(), "profile".to_owned()],
            }),
            session: session_config(),
            clusters: vec![ClusterConfig {
                name: "demo".to_owned(),
                api_url: "https://kubernetes.example.com".to_owned(),
                bearer_token: "token".to_owned(),
                ca_cert: None,
                insecure_skip_tls_verify: false,
                namespaces: vec!["analytics".to_owned()],
                flink_api_version: "v1beta1".to_owned(),
                flink_rest_base_url: None,
            }],
        };

        config.validate().expect("live mode should validate");
    }

    #[test]
    fn live_config_rejects_secure_cookie_on_http_external_base_url() {
        let mut session = session_config();
        session.secure_cookie = true;

        let config = AppConfig {
            host: "127.0.0.1".to_owned(),
            port: 3000,
            root_dir: workspace_root(),
            fixture_mode: false,
            fixture_file: workspace_root().join("fixtures/jobs.json"),
            cache_ttl_ms: 0,
            request_timeout_ms: 1_000,
            oidc_request_timeout_ms: 15_000,
            oidc: Some(OidcConfig {
                issuer_url: "https://issuer.example.com".to_owned(),
                client_id: "client-id".to_owned(),
                client_secret: "client-secret".to_owned(),
                external_base_url: "http://localhost:3000".to_owned(),
                callback_path: "/auth/callback".to_owned(),
                scopes: vec!["openid".to_owned(), "profile".to_owned()],
            }),
            session,
            clusters: vec![ClusterConfig {
                name: "demo".to_owned(),
                api_url: "https://kubernetes.example.com".to_owned(),
                bearer_token: "token".to_owned(),
                ca_cert: None,
                insecure_skip_tls_verify: false,
                namespaces: vec!["analytics".to_owned()],
                flink_api_version: "v1beta1".to_owned(),
                flink_rest_base_url: None,
            }],
        };

        let error = config
            .validate()
            .expect_err("http external base URL should reject secure cookies");
        assert!(error.to_string().contains("SESSION_SECURE_COOKIE=true"));
    }
}

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const SERVICE_ACCOUNT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const SERVICE_ACCOUNT_CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
const DEFAULT_TRUSTED_AUTH_HEADERS: [&str; 2] = ["x-auth-request-user", "x-forwarded-user"];

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub root_dir: PathBuf,
    pub fixture_mode: bool,
    pub fixture_file: PathBuf,
    pub cache_ttl_ms: u64,
    pub request_timeout_ms: u64,
    pub trusted_auth_headers: Vec<String>,
    pub clusters: Vec<ClusterConfig>,
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
        let fixture_mode = parse_bool(
            env::var("FIXTURE_MODE").ok().as_deref(),
            clusters.is_empty(),
        ) || clusters.is_empty();
        let fixture_file = root_dir
            .join(env::var("FIXTURE_FILE").unwrap_or_else(|_| "fixtures/jobs.json".to_owned()));
        let trusted_auth_headers =
            parse_trusted_auth_headers(env::var("AUTH_TRUSTED_HEADERS").ok());

        Ok(Self {
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
            request_timeout_ms: env::var("REQUEST_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(4_000),
            trusted_auth_headers,
            clusters,
        })
    }

    pub fn static_dir(&self) -> PathBuf {
        self.root_dir.join("apps/web/public")
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

fn parse_trusted_auth_headers(value: Option<String>) -> Vec<String> {
    let headers = value
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .map(|header| header.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if headers.is_empty() {
        DEFAULT_TRUSTED_AUTH_HEADERS
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        headers
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
    use super::parse_trusted_auth_headers;

    #[test]
    fn parse_trusted_auth_headers_uses_defaults_when_value_missing() {
        assert_eq!(
            parse_trusted_auth_headers(None),
            vec![
                "x-auth-request-user".to_owned(),
                "x-forwarded-user".to_owned()
            ]
        );
    }

    #[test]
    fn parse_trusted_auth_headers_normalizes_custom_values() {
        assert_eq!(
            parse_trusted_auth_headers(Some(" X-Custom-User , x-another-header ".to_owned())),
            vec!["x-custom-user".to_owned(), "x-another-header".to_owned()]
        );
    }
}

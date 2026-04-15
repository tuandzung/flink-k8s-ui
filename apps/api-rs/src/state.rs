use crate::auth::AuthService;
use crate::config::AppConfig;
use crate::http::metrics::MetricsState;
use crate::service::jobs_service::JobsService;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::UNIX_EPOCH;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub shell_asset_version: String,
    pub jobs_service: JobsService,
    pub metrics: MetricsState,
    pub auth: AuthService,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            jobs_service: JobsService::new(config.clone()),
            metrics: MetricsState::new(),
            auth: AuthService::new(config.oidc_request_timeout_ms),
            shell_asset_version: shell_asset_version(&config),
            config,
        }
    }
}

fn shell_asset_version(config: &AppConfig) -> String {
    let mut hasher = DefaultHasher::new();

    for asset_name in ["app.js", "render.js", "styles.css"] {
        let path = config.static_dir().join(asset_name);
        asset_name.hash(&mut hasher);

        match std::fs::metadata(path) {
            Ok(metadata) => {
                metadata.len().hash(&mut hasher);
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                        duration.as_millis().hash(&mut hasher);
                    }
                }
            }
            Err(_) => "missing".hash(&mut hasher),
        }
    }

    format!("{:x}", hasher.finish())
}

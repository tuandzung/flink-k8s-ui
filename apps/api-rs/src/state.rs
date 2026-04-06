use crate::auth::AuthService;
use crate::config::AppConfig;
use crate::http::metrics::MetricsState;
use crate::service::jobs_service::JobsService;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
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
            config,
        }
    }
}

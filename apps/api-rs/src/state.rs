use crate::config::AppConfig;
use crate::http::metrics::MetricsState;
use crate::service::jobs_service::JobsService;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub jobs_service: JobsService,
    pub metrics: MetricsState,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            jobs_service: JobsService::new(config.clone()),
            metrics: MetricsState::new(),
            config,
        }
    }
}

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::RwLock;

use crate::http::handlers::health::utc_now_string;

#[derive(Clone)]
pub struct MetricsState {
    snapshot: Arc<RwLock<MetricsSnapshot>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub started_at: String,
    pub requests_total: u64,
    pub errors_total: u64,
    pub routes: BTreeMap<String, u64>,
}

impl MetricsState {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(MetricsSnapshot {
                started_at: utc_now_string(),
                requests_total: 0,
                errors_total: 0,
                routes: BTreeMap::new(),
            })),
        }
    }

    pub async fn increment_requests(&self) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.requests_total += 1;
    }

    pub async fn record_response(&self, route: &str, status_code: u16) {
        let mut snapshot = self.snapshot.write().await;
        *snapshot.routes.entry(route.to_owned()).or_insert(0) += 1;
        if status_code >= 400 {
            snapshot.errors_total += 1;
        }
    }

    pub async fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot.read().await.clone()
    }
}

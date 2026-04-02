use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::RwLock;

use crate::adapters::fixture::load_fixture_jobs;
use crate::adapters::flink::enrich_jobs;
use crate::adapters::k8s::list_cluster_jobs;
use crate::config::AppConfig;
use crate::domain::job::Job;

#[derive(Clone)]
pub struct JobsService {
    config: AppConfig,
    cache: Arc<RwLock<Option<CacheEntry>>>,
}

struct CacheEntry {
    jobs: Vec<Job>,
    expires_at: Instant,
}

impl JobsService {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn list_jobs(&self, force_refresh: bool) -> Result<Vec<Job>> {
        if !force_refresh {
            let cache = self.cache.read().await;
            if let Some(entry) = &*cache {
                if Instant::now() < entry.expires_at {
                    return Ok(entry.jobs.clone());
                }
            }
        }

        let jobs = sort_jobs(self.load_jobs().await?);
        let ttl = Duration::from_millis(self.config.cache_ttl_ms);
        let mut cache = self.cache.write().await;
        *cache = Some(CacheEntry {
            jobs: jobs.clone(),
            expires_at: Instant::now() + ttl,
        });

        Ok(jobs)
    }

    pub async fn get_job_by_id(&self, id: &str) -> Result<Option<Job>> {
        Ok(self
            .list_jobs(false)
            .await?
            .into_iter()
            .find(|job| job.id == id))
    }

    pub async fn get_job_by_locator(
        &self,
        cluster: &str,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> Result<Option<Job>> {
        Ok(self.list_jobs(false).await?.into_iter().find(|job| {
            job.cluster == cluster
                && job.namespace == namespace
                && job.kind == kind
                && job.resource_name == name
        }))
    }

    async fn load_jobs(&self) -> Result<Vec<Job>> {
        if self.config.fixture_mode {
            return load_fixture_jobs(&self.config).await;
        }

        if self.config.clusters.is_empty() {
            bail!("no Kubernetes clusters configured and fixture mode is disabled");
        }

        let mut jobs = Vec::new();
        for cluster in &self.config.clusters {
            jobs.extend(list_cluster_jobs(cluster, self.config.request_timeout_ms).await?);
        }
        Ok(enrich_jobs(jobs, self.config.request_timeout_ms).await)
    }
}

fn sort_jobs(mut jobs: Vec<Job>) -> Vec<Job> {
    jobs.sort_by(|left, right| {
        left.cluster
            .cmp(&right.cluster)
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.resource_name.cmp(&right.resource_name))
    });
    jobs
}

#[cfg(test)]
mod tests {
    use super::JobsService;
    use crate::config::AppConfig;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate should be nested in apps/")
            .parent()
            .expect("workspace root should exist")
            .to_path_buf()
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
            trusted_auth_headers: vec!["x-auth-request-user".to_owned()],
            clusters: Vec::new(),
        }
    }

    #[tokio::test]
    async fn jobs_service_returns_fixture_rows() {
        let service = JobsService::new(fixture_config());
        let jobs = service
            .list_jobs(true)
            .await
            .expect("fixture jobs should load");

        assert_eq!(jobs.len(), 4);
        assert_eq!(jobs[0].cluster, "demo");
    }

    #[tokio::test]
    async fn jobs_service_can_fetch_a_job_by_id() {
        let service = JobsService::new(fixture_config());
        let job = service
            .get_job_by_id("prod:risk:FlinkDeployment:risk-detector")
            .await
            .expect("job lookup should succeed")
            .expect("job should exist");

        assert_eq!(job.resource_name, "risk-detector");
        assert_eq!(job.status, "failed");
    }

    #[tokio::test]
    async fn jobs_service_can_fetch_a_job_by_locator() {
        let service = JobsService::new(fixture_config());
        let job = service
            .get_job_by_locator("demo", "analytics", "FlinkDeployment", "orders-stream")
            .await
            .expect("job lookup should succeed")
            .expect("job should exist");

        assert_eq!(job.job_name, "orders-stream");
    }
}

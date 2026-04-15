use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::fs;

use crate::config::AppConfig;
use crate::domain::job::Job;

#[derive(Deserialize)]
struct FixtureEnvelope {
    jobs: Vec<Job>,
}

pub async fn load_fixture_jobs(config: &AppConfig) -> Result<Vec<Job>> {
    let raw = fs::read_to_string(&config.fixture_file)
        .await
        .with_context(|| {
            format!(
                "failed to read fixture file {}",
                config.fixture_file.display()
            )
        })?;
    let payload: FixtureEnvelope =
        serde_json::from_str(&raw).context("failed to parse fixture JSON payload")?;
    Ok(payload
        .jobs
        .into_iter()
        .map(|mut job| {
            if job.actions == crate::domain::job::JobActions::default() {
                job.derive_actions();
            }
            job.disable_actions("Actions are unavailable in fixture mode");
            job
        })
        .collect())
}

use std::borrow::Cow;
use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use tracing::error;

use crate::domain::job::Job;
use crate::error::UpstreamHttpError;
use crate::http::handlers::health::utc_now_string;
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsResponse {
    jobs: Vec<Job>,
    meta: JobsMeta,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsMeta {
    total: usize,
    generated_at: String,
}

#[derive(Serialize)]
pub struct ClustersResponse {
    clusters: Vec<ClusterResponse>,
}

#[derive(Serialize)]
pub struct ClusterResponse {
    name: String,
}

#[derive(Serialize)]
pub struct JobResponse {
    job: Job,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    error: String,
    details: Cow<'static, str>,
}

const PUBLIC_UPSTREAM_ERROR_DETAILS: &str =
    "The upstream request failed; check server logs for details";
const PUBLIC_INTERNAL_ERROR_DETAILS: &str =
    "The server could not complete the request; check server logs for details";

pub async fn list_jobs(
    State(state): State<AppState>,
) -> Result<Json<JobsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let jobs = state
        .jobs_service
        .list_jobs(false)
        .await
        .map_err(|error| internal_error("Failed to list jobs", error))?;

    Ok(Json(JobsResponse {
        meta: JobsMeta {
            total: jobs.len(),
            generated_at: utc_now_string(),
        },
        jobs,
    }))
}

pub async fn get_clusters(
    State(state): State<AppState>,
) -> Result<Json<ClustersResponse>, (StatusCode, Json<ErrorResponse>)> {
    let jobs = state
        .jobs_service
        .list_jobs(false)
        .await
        .map_err(|error| internal_error("Failed to list clusters", error))?;

    let clusters = jobs
        .into_iter()
        .map(|job| job.cluster)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| ClusterResponse { name })
        .collect();

    Ok(Json(ClustersResponse { clusters }))
}

pub async fn get_job_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .jobs_service
        .get_job_by_id(&id)
        .await
        .map_err(|error| internal_error("Failed to fetch job", error))?
    {
        Some(job) => Ok(Json(JobResponse { job })),
        None => Err(not_found("Job not found")),
    }
}

pub async fn get_job_by_locator(
    State(state): State<AppState>,
    Path((cluster, namespace, kind, name)): Path<(String, String, String, String)>,
) -> Result<Json<JobResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state
        .jobs_service
        .get_job_by_locator(&cluster, &namespace, &kind, &name)
        .await
        .map_err(|error| internal_error("Failed to fetch job", error))?
    {
        Some(job) => Ok(Json(JobResponse { job })),
        None => Err(not_found("Job not found")),
    }
}

fn internal_error(message: &str, error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let status_code = error
        .downcast_ref::<UpstreamHttpError>()
        .and_then(|error| StatusCode::from_u16(error.status_code).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let error_chain = format_error_chain(&error);

    error!(
        public_error = message,
        response_status = status_code.as_u16(),
        error = %error,
        error_chain = %error_chain,
        "jobs_request_failed"
    );

    (
        status_code,
        Json(ErrorResponse {
            error: message.to_owned(),
            details: public_error_details(&error).to_owned(),
        }),
    )
}

fn not_found(message: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: message.to_owned(),
            details: Cow::Borrowed(message),
        }),
    )
}

fn public_error_details(error: &anyhow::Error) -> Cow<'static, str> {
    if error.downcast_ref::<UpstreamHttpError>().is_some() {
        Cow::Borrowed(PUBLIC_UPSTREAM_ERROR_DETAILS)
    } else {
        Cow::Borrowed(PUBLIC_INTERNAL_ERROR_DETAILS)
    }
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

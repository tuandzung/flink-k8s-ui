use axum::Json;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthCallbackQuery, LogoutError, fixture_session_snapshot, read_cookie};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusResponse {
    authenticated: bool,
    fixture_mode: bool,
    login_url: Option<&'static str>,
    logout_url: Option<&'static str>,
    csrf_token: Option<String>,
    expires_at: Option<String>,
    user: Option<SessionUserResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionUserResponse {
    subject: String,
    email: Option<String>,
    name: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogoutResponse {
    authenticated: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoutRequest {
    csrf_token: Option<String>,
}

pub async fn session_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let session = if state.config.fixture_mode {
        Some(fixture_session_snapshot())
    } else {
        state
            .auth
            .session_from_headers(&state.config, &headers)
            .await
            .map_err(internal_server_error)?
    };

    Ok(Json(match session {
        Some(session) => SessionStatusResponse {
            authenticated: true,
            fixture_mode: state.config.fixture_mode,
            login_url: None,
            logout_url: (!state.config.fixture_mode).then_some("/auth/logout"),
            csrf_token: (!state.config.fixture_mode).then_some(session.csrf_token),
            expires_at: Some(session.expires_at.to_string()),
            user: Some(SessionUserResponse {
                subject: session.user.subject,
                email: session.user.email,
                name: session.user.name,
            }),
        },
        None => SessionStatusResponse {
            authenticated: false,
            fixture_mode: false,
            login_url: Some("/auth/login"),
            logout_url: None,
            csrf_token: None,
            expires_at: None,
            user: None,
        },
    }))
}

pub async fn login(
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    if state.config.fixture_mode {
        return Ok(Redirect::to("/").into_response());
    }

    let login = state
        .auth
        .start_login(&state.config)
        .await
        .map_err(internal_server_error)?;
    let mut response = Redirect::to(&login.authorization_url).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&login.auth_flow_cookie).map_err(header_error)?,
    );
    Ok(response)
}

pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AuthCallbackQuery>,
) -> Response {
    let auth_flow_cookie = read_cookie(&headers, &state.config.session.auth_flow_cookie_name);

    match state
        .auth
        .complete_login(&state.config, &query, auth_flow_cookie)
        .await
    {
        Ok(login) => {
            let mut response = Redirect::to("/").into_response();
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_str(&login.clear_auth_flow_cookie)
                    .expect("auth flow cookie header should be valid"),
            );
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_str(&login.session_cookie)
                    .expect("session cookie header should be valid"),
            );
            response
        }
        Err(_) => {
            let mut response = Redirect::to("/?authError=callback_failed").into_response();
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_str(&state.auth.clear_session_cookie(&state.config).replace(
                    &state.config.session.cookie_name,
                    &state.config.session.auth_flow_cookie_name,
                ))
                .unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            response
        }
    }
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LogoutRequest>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    match state
        .auth
        .logout(&state.config, &headers, form.csrf_token.as_deref())
        .await
    {
        Ok(()) => {
            let mut response = Json(LogoutResponse {
                authenticated: false,
            })
            .into_response();
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_str(&state.auth.clear_session_cookie(&state.config))
                    .map_err(header_error)?,
            );
            Ok(response)
        }
        Err(LogoutError::Forbidden(message)) => Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: message.to_owned(),
            }),
        )),
        Err(LogoutError::Other(error)) => Err(internal_server_error(error)),
    }
}

fn internal_server_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn header_error(
    error: axum::http::header::InvalidHeaderValue,
) -> (StatusCode, Json<ErrorResponse>) {
    internal_server_error(anyhow::anyhow!(error.to_string()))
}

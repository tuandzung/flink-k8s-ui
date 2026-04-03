use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::sync::RwLock;
use url::Url;

use crate::config::{AppConfig, OidcConfig, SessionConfig};

type HmacSha256 = Hmac<Sha256>;
const LOGOUT_CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone)]
pub struct AuthService {
    client: reqwest::Client,
    provider_metadata: Arc<RwLock<Option<ProviderMetadata>>>,
    sessions: Arc<RwLock<HashMap<String, StoredSession>>>,
    pending_logins: Arc<RwLock<HashMap<String, PendingLogin>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUser {
    pub subject: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StoredSession {
    pub user: SessionUser,
    pub csrf_token: String,
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub user: SessionUser,
    pub csrf_token: String,
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct LoginInitiation {
    pub authorization_url: String,
    pub auth_flow_cookie: String,
}

#[derive(Clone, Debug)]
pub struct LoginCompletion {
    pub session_cookie: String,
    pub clear_auth_flow_cookie: String,
}

#[derive(Debug, Deserialize)]
pub struct AuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Clone, Debug)]
struct PendingLogin {
    pkce_verifier: String,
    expires_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize)]
struct ProviderMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    preferred_username: Option<String>,
}

#[derive(Debug)]
pub enum LogoutError {
    Forbidden(&'static str),
    Other(anyhow::Error),
}

impl AuthService {
    pub fn new(request_timeout_ms: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(request_timeout_ms.max(1)))
            .build()
            .expect("auth HTTP client should build");

        Self {
            client,
            provider_metadata: Arc::new(RwLock::new(None)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pending_logins: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start_login(&self, config: &AppConfig) -> Result<LoginInitiation> {
        let oidc = config
            .oidc
            .as_ref()
            .context("OIDC login is not configured")?;
        let metadata = self.provider_metadata(oidc).await?;
        let flow_id = random_token(32);
        let state_token = sign_token(&config.session, &flow_id)?;
        let pkce_verifier = random_token(48);

        self.cleanup_pending_logins(config.session.auth_flow_ttl_secs)
            .await;
        self.pending_logins.write().await.insert(
            flow_id.clone(),
            PendingLogin {
                pkce_verifier: pkce_verifier.clone(),
                expires_at: OffsetDateTime::now_utc()
                    + time::Duration::seconds(config.session.auth_flow_ttl_secs),
            },
        );

        let mut authorization_url = Url::parse(&metadata.authorization_endpoint)
            .context("failed to parse OIDC authorization endpoint")?;
        authorization_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &oidc.client_id)
            .append_pair(
                "redirect_uri",
                &config
                    .oidc_callback_url()
                    .context("OIDC callback URL is not configured")?,
            )
            .append_pair("scope", &oidc.scopes.join(" "))
            .append_pair("state", &state_token)
            .append_pair("code_challenge", &pkce_challenge(&pkce_verifier))
            .append_pair("code_challenge_method", "S256");

        Ok(LoginInitiation {
            authorization_url: authorization_url.into(),
            auth_flow_cookie: build_cookie(
                &config.session.auth_flow_cookie_name,
                &state_token,
                config.session.auth_flow_ttl_secs,
                config.session.secure_cookie,
            ),
        })
    }

    pub async fn complete_login(
        &self,
        config: &AppConfig,
        query: &AuthCallbackQuery,
        auth_flow_cookie_value: Option<&str>,
    ) -> Result<LoginCompletion> {
        if let Some(error) = &query.error {
            let description = query
                .error_description
                .as_deref()
                .unwrap_or("unknown error");
            bail!("OIDC callback returned {error}: {description}");
        }

        let code = query
            .code
            .as_deref()
            .context("missing OIDC authorization code")?;
        let state = query.state.as_deref().context("missing OIDC state")?;
        let Some(cookie_state) = auth_flow_cookie_value else {
            bail!("missing OIDC auth flow cookie")
        };
        if state != cookie_state {
            bail!("OIDC state mismatch")
        }

        let flow_id = verify_signed_token(&config.session, state)?;
        let pending = self
            .pending_logins
            .write()
            .await
            .remove(&flow_id)
            .context("OIDC auth flow is missing or already consumed")?;
        if pending.expires_at <= OffsetDateTime::now_utc() {
            bail!("OIDC auth flow expired")
        }

        let oidc = config
            .oidc
            .as_ref()
            .context("OIDC login is not configured")?;
        let metadata = self.provider_metadata(oidc).await?;
        let token_response = exchange_code(
            &self.client,
            oidc,
            &metadata,
            code,
            &pending.pkce_verifier,
            &config
                .oidc_callback_url()
                .context("OIDC callback URL is not configured")?,
        )
        .await?;

        let user = fetch_user(&self.client, &metadata, &token_response.access_token).await?;

        let session_id = random_token(32);
        let csrf_token = random_token(32);
        self.cleanup_sessions().await;
        self.sessions.write().await.insert(
            session_id.clone(),
            StoredSession {
                user,
                csrf_token,
                expires_at: OffsetDateTime::now_utc()
                    + time::Duration::seconds(config.session.ttl_secs),
            },
        );

        Ok(LoginCompletion {
            session_cookie: build_cookie(
                &config.session.cookie_name,
                &sign_token(&config.session, &session_id)?,
                config.session.ttl_secs,
                config.session.secure_cookie,
            ),
            clear_auth_flow_cookie: clear_cookie(
                &config.session.auth_flow_cookie_name,
                config.session.secure_cookie,
            ),
        })
    }

    pub async fn session_from_headers(
        &self,
        config: &AppConfig,
        headers: &HeaderMap,
    ) -> Result<Option<SessionSnapshot>> {
        let Some(cookie_value) = read_cookie(headers, &config.session.cookie_name) else {
            return Ok(None);
        };
        let session_id = match verify_signed_token(&config.session, cookie_value) {
            Ok(session_id) => session_id,
            Err(_) => return Ok(None),
        };

        let mut sessions = self.sessions.write().await;
        let Some(stored_session) = sessions.get(&session_id).cloned() else {
            return Ok(None);
        };
        if stored_session.expires_at <= OffsetDateTime::now_utc() {
            sessions.remove(&session_id);
            return Ok(None);
        }

        Ok(Some(SessionSnapshot {
            user: stored_session.user,
            csrf_token: stored_session.csrf_token,
            expires_at: stored_session.expires_at,
        }))
    }

    pub async fn logout(
        &self,
        config: &AppConfig,
        headers: &HeaderMap,
        provided_csrf_token: Option<&str>,
    ) -> std::result::Result<(), LogoutError> {
        let Some(cookie_value) = read_cookie(headers, &config.session.cookie_name) else {
            return Ok(());
        };
        let session_id =
            verify_signed_token(&config.session, cookie_value).map_err(LogoutError::Other)?;
        let csrf_header = provided_csrf_token
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                headers
                    .get(LOGOUT_CSRF_HEADER)
                    .and_then(|value| value.to_str().ok())
            })
            .unwrap_or_default();

        let mut sessions = self.sessions.write().await;
        match sessions.get(&session_id) {
            Some(session) if session.csrf_token == csrf_header => {
                sessions.remove(&session_id);
                Ok(())
            }
            Some(_) => Err(LogoutError::Forbidden("Invalid CSRF token")),
            None => Ok(()),
        }
    }

    pub fn clear_session_cookie(&self, config: &AppConfig) -> String {
        clear_cookie(&config.session.cookie_name, config.session.secure_cookie)
    }

    pub fn clear_auth_flow_cookie(&self, config: &AppConfig) -> String {
        clear_cookie(
            &config.session.auth_flow_cookie_name,
            config.session.secure_cookie,
        )
    }

    #[cfg(test)]
    pub async fn issue_test_session(&self, config: &AppConfig) -> Result<(String, String)> {
        let session_id = random_token(32);
        let csrf_token = random_token(32);
        self.sessions.write().await.insert(
            session_id.clone(),
            StoredSession {
                user: SessionUser {
                    subject: "test-user".to_owned(),
                    email: Some("test@example.com".to_owned()),
                    name: Some("Test User".to_owned()),
                },
                csrf_token: csrf_token.clone(),
                expires_at: OffsetDateTime::now_utc() + time::Duration::hours(1),
            },
        );

        Ok((
            format!(
                "{}={}",
                config.session.cookie_name,
                sign_token(&config.session, &session_id)?
            ),
            csrf_token,
        ))
    }

    async fn provider_metadata(&self, oidc: &OidcConfig) -> Result<ProviderMetadata> {
        if let Some(metadata) = self.provider_metadata.read().await.clone() {
            return Ok(metadata);
        }

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            oidc.issuer_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(discovery_url)
            .send()
            .await
            .context("failed to fetch OIDC discovery document")?;
        let response = response
            .error_for_status()
            .context("OIDC discovery request failed")?;
        let metadata = response
            .json::<ProviderMetadata>()
            .await
            .context("failed to decode OIDC discovery document")?;

        *self.provider_metadata.write().await = Some(metadata.clone());
        Ok(metadata)
    }

    async fn cleanup_sessions(&self) {
        let now = OffsetDateTime::now_utc();
        self.sessions
            .write()
            .await
            .retain(|_, session| session.expires_at > now);
    }

    async fn cleanup_pending_logins(&self, auth_flow_ttl_secs: i64) {
        let _ = auth_flow_ttl_secs;
        let now = OffsetDateTime::now_utc();
        self.pending_logins
            .write()
            .await
            .retain(|_, pending| pending.expires_at > now);
    }
}

pub fn fixture_session_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        user: SessionUser {
            subject: "fixture-mode".to_owned(),
            email: None,
            name: Some("Fixture Mode".to_owned()),
        },
        csrf_token: "fixture-mode".to_owned(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::hours(24),
    }
}

pub fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let cookie_header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|entry| {
        let mut segments = entry.trim().splitn(2, '=');
        match (segments.next(), segments.next()) {
            (Some(cookie_name), Some(cookie_value)) if cookie_name == name => Some(cookie_value),
            _ => None,
        }
    })
}

fn build_cookie(name: &str, value: &str, max_age_secs: i64, secure_cookie: bool) -> String {
    let secure = if secure_cookie { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}")
}

fn clear_cookie(name: &str, secure_cookie: bool) -> String {
    let secure = if secure_cookie { "; Secure" } else { "" };
    format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
}

fn sign_token(session: &SessionConfig, raw_value: &str) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(session.cookie_secret.as_bytes())
        .map_err(|error| anyhow!(error.to_string()))?;
    mac.update(raw_value.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!(
        "{}.{signature}",
        URL_SAFE_NO_PAD.encode(raw_value.as_bytes())
    ))
}

fn verify_signed_token(session: &SessionConfig, signed_value: &str) -> Result<String> {
    let (encoded_payload, encoded_signature) = signed_value
        .split_once('.')
        .context("signed cookie is malformed")?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .context("signed cookie payload is invalid")?;
    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .context("signed cookie signature is invalid")?;
    let mut mac = HmacSha256::new_from_slice(session.cookie_secret.as_bytes())
        .map_err(|error| anyhow!(error.to_string()))?;
    mac.update(&payload);
    mac.verify_slice(&signature)
        .map_err(|_| anyhow!("signed cookie verification failed"))?;
    String::from_utf8(payload).context("signed cookie payload is not utf-8")
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

async fn exchange_code(
    client: &reqwest::Client,
    oidc: &OidcConfig,
    metadata: &ProviderMetadata,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse> {
    let response = client
        .post(&metadata.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", oidc.client_id.as_str()),
            ("client_secret", oidc.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange OIDC authorization code")?;

    if response.status() != StatusCode::OK {
        bail!(
            "OIDC token exchange failed with status {}",
            response.status()
        )
    }

    response
        .json::<TokenResponse>()
        .await
        .context("failed to decode OIDC token response")
}

async fn fetch_user(
    client: &reqwest::Client,
    metadata: &ProviderMetadata,
    access_token: &str,
) -> Result<SessionUser> {
    let userinfo_endpoint = metadata
        .userinfo_endpoint
        .as_ref()
        .context("OIDC provider did not expose a userinfo endpoint")?;
    let response = client
        .get(userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .context("failed to fetch OIDC userinfo")?;
    let response = response
        .error_for_status()
        .context("OIDC userinfo request failed")?;
    let userinfo = response
        .json::<UserInfoResponse>()
        .await
        .context("failed to decode OIDC userinfo response")?;
    Ok(SessionUser {
        subject: userinfo.sub,
        email: userinfo.email,
        name: userinfo.name.or(userinfo.preferred_username),
    })
}

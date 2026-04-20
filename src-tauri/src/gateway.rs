use llm_relay_core::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResult {
    pub ok: bool,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub is_user: bool,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub session_token: Option<String>,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub created_at: Option<String>,
    pub last_used_at: Option<String>,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub owned_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelList {
    pub data: Vec<ModelInfo>,
}

/// Login to a gateway using ADMIN_KEY, User Key, session token, or API key.
/// POST /auth/login  body: { "key": "<key>" }
pub async fn login(url: &str, key: &str) -> Result<LoginResult, AppError> {
    let client = reqwest::Client::new();
    let base = url.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/auth/login"))
        .json(&serde_json::json!({ "key": key }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!("Login failed ({status}): {body}")));
    }

    let result: LoginResult = resp.json().await?;
    Ok(result)
}

/// Fetch API keys from a gateway.
/// GET /api/keys  Authorization: Bearer <session_token or auth_key>
pub async fn fetch_keys(url: &str, auth: &str) -> Result<Vec<ApiKey>, AppError> {
    let client = reqwest::Client::new();
    let base = url.trim_end_matches('/');
    let resp = client
        .get(format!("{base}/api/keys"))
        .header("Authorization", format!("Bearer {auth}"))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!(
            "Fetch keys failed ({status}): {body}"
        )));
    }

    let keys: Vec<ApiKey> = resp.json().await?;
    Ok(keys)
}

/// Fetch models from a gateway. Also used for health checks.
/// GET /api/models  Authorization: Bearer <auth>
pub async fn fetch_models(url: &str, auth: &str) -> Result<ModelList, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let base = url.trim_end_matches('/');
    let resp = client
        .get(format!("{base}/api/models"))
        .header("Authorization", format!("Bearer {auth}"))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!(
            "Fetch models failed ({status}): {body}"
        )));
    }

    let models: ModelList = resp.json().await?;
    Ok(models)
}

/// Health check a gateway by calling GET /api/models with a 10s timeout.
/// Retries up to 3 times with exponential backoff (100ms, 200ms).
/// Returns (is_healthy, latency_ms, model_count).
pub async fn health_check(url: &str, auth: &str) -> (bool, Option<i64>, Option<i32>) {
    for attempt in 0u32..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(100 * 2_u64.pow(attempt - 1))).await;
        }
        let start = std::time::Instant::now();
        match fetch_models(url, auth).await {
            Ok(models) => {
                let latency = start.elapsed().as_millis() as i64;
                return (true, Some(latency), Some(models.data.len() as i32));
            }
            Err(_) if attempt < 2 => continue,
            Err(_) => return (false, None, None),
        }
    }
    (false, None, None)
}

// ─── Device Authorization Flow ───

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "expires_in")]
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePollResponse {
    pub status: String,
    #[serde(alias = "session_token")]
    pub session_token: Option<String>,
    #[serde(alias = "user_id")]
    pub user_id: Option<String>,
    #[serde(alias = "user_name")]
    pub user_name: Option<String>,
}

/// Request a device code from the gateway. POST /auth/device/code (no auth required).
pub async fn request_device_code(url: &str) -> Result<DeviceCodeResponse, AppError> {
    let client = reqwest::Client::new();
    let base = url.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/auth/device/code"))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!("Device code request failed ({status}): {body}")));
    }

    let result: DeviceCodeResponse = resp.json().await?;
    Ok(result)
}

/// Poll for device code verification. POST /auth/device/poll (no auth required).
pub async fn poll_device_code(url: &str, device_code: &str) -> Result<DevicePollResponse, AppError> {
    let client = reqwest::Client::new();
    let base = url.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/auth/device/poll"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http(format!("Device poll failed ({status}): {body}")));
    }

    let result: DevicePollResponse = resp.json().await?;
    Ok(result)
}

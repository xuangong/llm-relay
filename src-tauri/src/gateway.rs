use crate::error::AppError;
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

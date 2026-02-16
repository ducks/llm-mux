//! Claude API backend executor

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse};
use crate::config::BackendConfig;
use async_trait::async_trait;
use serde::Deserialize;
use std::env;
use std::time::{Duration, Instant};

/// Executor for Claude API
#[derive(Debug, Clone)]
pub struct ClaudeBackend {
    /// Backend name
    name: String,

    /// API key
    api_key: String,

    /// Model to use
    model: String,

    /// HTTP client
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

impl ClaudeBackend {
    /// Create a new Claude API backend from config
    pub fn from_config(
        name: impl Into<String>,
        config: &BackendConfig,
    ) -> Result<Self, BackendError> {
        let api_key_env = config
            .api_key_env
            .clone()
            .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());

        let api_key = env::var(&api_key_env).map_err(|_| BackendError::Unavailable {
            message: format!("Missing environment variable: {}", api_key_env),
        })?;

        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout))
            .build()
            .map_err(|e| BackendError::Unavailable {
                message: format!("Failed to create HTTP client: {}", e),
            })?;

        Ok(Self {
            name: name.into(),
            api_key,
            model,
            client,
        })
    }
}

#[async_trait]
impl BackendExecutor for ClaudeBackend {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let start = Instant::now();

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "messages": [
                {
                    "role": "user",
                    "content": request.prompt
                }
            ]
        });

        eprintln!(
            "[DEBUG {}] calling API with {} chars",
            self.name,
            request.prompt.len()
        );

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Unavailable {
                message: format!("Failed to send request: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::execution_failed(
                Some(status.as_u16() as i32),
                String::new(),
                format!("API error {}: {}", status, body),
            ));
        }

        let claude_response: ClaudeResponse = response
            .json()
            .await
            .map_err(|e| BackendError::parse(format!("Failed to parse response: {}", e)))?;

        let text = claude_response
            .content
            .into_iter()
            .filter_map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n");

        eprintln!("[DEBUG {}] got {} chars response", self.name, text.len());

        Ok(BackendResponse::new(
            text,
            self.name.clone(),
            start.elapsed(),
        ))
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

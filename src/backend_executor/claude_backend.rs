//! Claude API backend executor

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, TokenUsage};
use crate::config::BackendConfig;
use crate::process::MAX_CAPTURE_BYTES;
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
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
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

        let send = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send();

        let response = if let Some(cancellation) = request.cancellation.as_ref() {
            tokio::select! {
                response = send => response,
                _ = cancellation.cancelled() => return Err(BackendError::Cancelled),
            }
        } else {
            send.await
        }
        .map_err(|e| BackendError::Unavailable {
            message: format!("Failed to send request: {}", e),
        })?;

        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_CAPTURE_BYTES as u64)
        {
            return Err(BackendError::parse(
                "Claude response exceeded 16 MiB capture limit",
            ));
        }

        let body = response.bytes();
        let body = if let Some(cancellation) = request.cancellation.as_ref() {
            tokio::select! {
                body = body => body,
                _ = cancellation.cancelled() => return Err(BackendError::Cancelled),
            }
        } else {
            body.await
        }
        .map_err(|error| BackendError::Unavailable {
            message: format!("Failed to read response: {error}"),
        })?;

        if body.len() > MAX_CAPTURE_BYTES {
            return Err(BackendError::parse(
                "Claude response exceeded 16 MiB capture limit",
            ));
        }

        if !status.is_success() {
            return Err(BackendError::execution_failed(
                Some(status.as_u16() as i32),
                String::new(),
                format!("API error {}: {}", status, String::from_utf8_lossy(&body)),
            ));
        }

        let claude_response: ClaudeResponse = serde_json::from_slice(&body)
            .map_err(|e| BackendError::parse(format!("Failed to parse response: {}", e)))?;

        let usage = claude_response.usage;
        let text = claude_response
            .content
            .into_iter()
            .filter_map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n");

        eprintln!("[DEBUG {}] got {} chars response", self.name, text.len());

        let mut response =
            BackendResponse::new(text, self.name.clone(), start.elapsed()).with_model(&self.model);
        if let Some(usage) = usage {
            response = response.with_usage(TokenUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: usage
                    .input_tokens
                    .zip(usage.output_tokens)
                    .map(|(input, output)| input + output),
            });
        }
        Ok(response)
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

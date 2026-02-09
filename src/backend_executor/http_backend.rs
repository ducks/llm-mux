#![allow(dead_code)]

//! HTTP API-based backend executor

use super::types::{BackendError, BackendExecutor, BackendRequest, BackendResponse, TokenUsage};
use crate::config::BackendConfig;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Executor for HTTP API-based LLM backends (OpenAI-compatible)
#[derive(Debug, Clone)]
pub struct HttpBackend {
    /// Backend name
    name: String,

    /// Base URL for the API
    base_url: String,

    /// API key (if required)
    api_key: Option<String>,

    /// Model ID to use
    model: Option<String>,

    /// Default timeout
    timeout: Duration,

    /// HTTP client
    client: reqwest::Client,
}

/// OpenAI-compatible chat completion request
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

/// OpenAI-compatible chat completion response
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    id: Option<String>,
    choices: Vec<Choice>,
    usage: Option<Usage>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

impl HttpBackend {
    /// Create a new HTTP backend from config
    pub fn from_config(name: impl Into<String>, config: &BackendConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout))
            .build()
            .expect("failed to build HTTP client");

        Self {
            name: name.into(),
            base_url: config.command.clone(), // For HTTP backends, command is the base URL
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            timeout: Duration::from_secs(config.timeout),
            client,
        }
    }

    /// Create a new HTTP backend with explicit parameters
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key: None,
            model: None,
            timeout: Duration::from_secs(300),
            client,
        }
    }

    /// Set the API key
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the chat completion URL
    fn chat_completion_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    /// Map HTTP status to BackendError
    fn map_http_error(&self, status: reqwest::StatusCode, body: &str) -> BackendError {
        match status.as_u16() {
            401 | 403 => BackendError::auth(format!("HTTP {}: {}", status, body)),
            429 => {
                // Try to parse retry-after from body
                let retry_after = self.parse_retry_after(body);
                BackendError::rate_limit(retry_after)
            }
            408 | 504 => BackendError::timeout(self.timeout, None),
            400..=499 => BackendError::Config {
                message: format!("HTTP {}: {}", status, body),
            },
            500..=599 => BackendError::Network {
                message: format!("HTTP {}: {}", status, body),
            },
            _ => BackendError::Network {
                message: format!("unexpected HTTP {}: {}", status, body),
            },
        }
    }

    /// Try to parse retry-after from error response
    fn parse_retry_after(&self, body: &str) -> Option<Duration> {
        // Try to parse as JSON and look for retry_after field
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(seconds) = json.get("retry_after").and_then(|v| v.as_f64()) {
                return Some(Duration::from_secs_f64(seconds));
            }
        }
        None
    }
}

#[async_trait]
impl BackendExecutor for HttpBackend {
    async fn execute(&self, request: &BackendRequest) -> Result<BackendResponse, BackendError> {
        let start = Instant::now();

        // Build messages
        let mut messages = Vec::new();

        if let Some(ref system) = request.system_prompt {
            messages.push(Message {
                role: "system".into(),
                content: system.clone(),
            });
        }

        messages.push(Message {
            role: "user".into(),
            content: request.prompt.clone(),
        });

        // Build request body
        let body = ChatCompletionRequest {
            model: self.model.clone().unwrap_or_else(|| "gpt-4".into()),
            messages,
            max_tokens: None,
            temperature: None,
        };

        // Build HTTP request
        let mut http_request = self.client.post(self.chat_completion_url()).json(&body);

        // Add auth header if we have an API key
        if let Some(ref key) = self.api_key {
            http_request = http_request.header("Authorization", format!("Bearer {}", key));
        }

        // Send request with timeout
        let timeout = request.timeout.unwrap_or(self.timeout);
        let result = tokio::time::timeout(timeout, http_request.send()).await;

        let elapsed = start.elapsed();

        match result {
            Ok(Ok(response)) => {
                let status = response.status();

                if status.is_success() {
                    let completion: ChatCompletionResponse =
                        response.json().await.map_err(|e| {
                            BackendError::parse(format!("failed to parse response: {}", e))
                        })?;

                    let text = completion
                        .choices
                        .first()
                        .and_then(|c| c.message.content.clone())
                        .unwrap_or_default();

                    let mut backend_response =
                        BackendResponse::new(text, self.name.clone(), elapsed);

                    if let Some(model) = completion.model {
                        backend_response = backend_response.with_model(model);
                    }

                    if let Some(usage) = completion.usage {
                        backend_response = backend_response.with_usage(TokenUsage {
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                            total_tokens: usage.total_tokens,
                        });
                    }

                    Ok(backend_response)
                } else {
                    let body = response.text().await.unwrap_or_default();
                    Err(self.map_http_error(status, &body))
                }
            }
            Ok(Err(e)) => {
                // Request error (network, etc.)
                if e.is_timeout() {
                    Err(BackendError::timeout(elapsed, None))
                } else if e.is_connect() {
                    Err(BackendError::network(format!("connection failed: {}", e)))
                } else {
                    Err(BackendError::network(format!("request failed: {}", e)))
                }
            }
            Err(_) => {
                // Tokio timeout
                Err(BackendError::timeout(elapsed, None))
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Try a simple request to check connectivity
        // Most APIs have a models endpoint we can ping
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));

        let mut request = self.client.get(&url);
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        match tokio::time::timeout(Duration::from_secs(5), request.send()).await {
            Ok(Ok(response)) => response.status().is_success(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_backend_builder() {
        let backend = HttpBackend::new("openai", "https://api.openai.com/v1")
            .with_api_key("sk-test")
            .with_model("gpt-4")
            .with_timeout(Duration::from_secs(60));

        assert_eq!(backend.name, "openai");
        assert_eq!(backend.base_url, "https://api.openai.com/v1");
        assert_eq!(backend.api_key, Some("sk-test".into()));
        assert_eq!(backend.model, Some("gpt-4".into()));
    }

    #[test]
    fn test_chat_completion_url() {
        let backend = HttpBackend::new("test", "https://api.example.com/v1");
        assert_eq!(
            backend.chat_completion_url(),
            "https://api.example.com/v1/chat/completions"
        );

        // With trailing slash
        let backend = HttpBackend::new("test", "https://api.example.com/v1/");
        assert_eq!(
            backend.chat_completion_url(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_map_http_error() {
        let backend = HttpBackend::new("test", "https://example.com");

        let err = backend.map_http_error(reqwest::StatusCode::UNAUTHORIZED, "bad token");
        assert!(matches!(err, BackendError::Auth { .. }));

        let err = backend.map_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "rate limited");
        assert!(matches!(err, BackendError::RateLimit { .. }));

        let err = backend.map_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "error");
        assert!(matches!(err, BackendError::Network { .. }));
    }

    #[test]
    fn test_from_config() {
        let config = BackendConfig {
            command: "https://api.openai.com/v1".into(),
            api_key: Some("sk-test".into()),
            model: Some("gpt-4".into()),
            timeout: 120,
            ..Default::default()
        };

        let backend = HttpBackend::from_config("openai", &config);
        assert_eq!(backend.name, "openai");
        assert_eq!(backend.base_url, "https://api.openai.com/v1");
        assert_eq!(backend.api_key, Some("sk-test".into()));
    }
}

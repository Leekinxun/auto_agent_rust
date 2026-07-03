use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::model::AppConfig;
use crate::infra::llm::types::{ChatCompletionRequest, ChatCompletionResponse};

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    context_window_cache: Arc<Mutex<HashMap<String, Option<u32>>>>,
}

impl LlmClient {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self {
            http,
            base_url: config.agent.base_url.trim_end_matches('/').to_string(),
            api_key: config.agent.api_key.clone(),
            context_window_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn chat(&self, request: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .context("failed to call llm chat completions")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("llm request failed with status {status}: {body}");
        }

        response
            .json::<ChatCompletionResponse>()
            .await
            .context("failed to decode llm response")
    }

    pub async fn stream_chat(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>>> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .context("failed to call llm chat completions stream")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("llm stream request failed with status {status}: {body}");
        }

        Ok(response.bytes_stream().map(|item| item))
    }

    pub async fn model_context_window(&self, model: &str) -> Option<u32> {
        let normalized_model = model.trim();
        if normalized_model.is_empty() {
            return None;
        }

        if let Some(cached) = self
            .context_window_cache
            .lock()
            .await
            .get(normalized_model)
            .copied()
        {
            return cached;
        }

        let discovered = self.fetch_model_context_window(normalized_model).await;
        self.context_window_cache
            .lock()
            .await
            .insert(normalized_model.to_string(), discovered);
        discovered
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    async fn fetch_model_context_window(&self, model: &str) -> Option<u32> {
        for candidate in self.model_metadata_candidates(model) {
            let mut request = self.http.get(&candidate.url).bearer_auth(&self.api_key);
            if let Some(query) = candidate.query.as_ref() {
                request = request.query(query);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    tracing::debug!(?error, url = %candidate.url, "failed to fetch model metadata");
                    continue;
                }
            };
            if !response.status().is_success() {
                tracing::debug!(
                    status = %response.status(),
                    url = %candidate.url,
                    "model metadata endpoint returned non-success"
                );
                continue;
            }
            let value = match response.json::<Value>().await {
                Ok(value) => value,
                Err(error) => {
                    tracing::debug!(?error, url = %candidate.url, "failed to decode model metadata");
                    continue;
                }
            };
            if let Some(window) = extract_context_window(&value, model) {
                tracing::info!(
                    model,
                    context_window = window,
                    url = %candidate.url,
                    "discovered model context window"
                );
                return Some(window);
            }
        }

        tracing::warn!(
            model,
            "failed to discover model context window; token usage percent will be omitted"
        );
        None
    }

    fn model_metadata_candidates(&self, model: &str) -> Vec<ModelMetadataCandidate> {
        let mut candidates = Vec::new();
        candidates.push(ModelMetadataCandidate {
            url: format!("{}/models", self.base_url),
            query: None,
        });
        candidates.push(ModelMetadataCandidate {
            url: format!("{}/get_model_config", self.base_url),
            query: Some(vec![("model".to_string(), model.to_string())]),
        });

        let root_url = llm_server_root_url(&self.base_url);
        if root_url != self.base_url {
            candidates.push(ModelMetadataCandidate {
                url: format!("{}/get_model_config", root_url),
                query: Some(vec![("model".to_string(), model.to_string())]),
            });
            candidates.push(ModelMetadataCandidate {
                url: format!("{}/get_model_config", root_url),
                query: None,
            });
        }

        candidates
    }
}

struct ModelMetadataCandidate {
    url: String,
    query: Option<Vec<(String, String)>>,
}

fn llm_server_root_url(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .strip_suffix("/v1")
        .unwrap_or(base_url)
        .trim_end_matches('/')
        .to_string()
}

fn extract_context_window(value: &Value, model: &str) -> Option<u32> {
    if let Some(data) = value.get("data").and_then(Value::as_array) {
        if let Some(item) = data.iter().find(|item| model_id_matches(item, model))
            && let Some(window) = find_context_window_recursive(item)
        {
            return Some(window);
        }
    }

    find_context_window_recursive(value)
}

fn model_id_matches(item: &Value, model: &str) -> bool {
    item.get("id")
        .or_else(|| item.get("model"))
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .map(|value| value == model)
        .unwrap_or(false)
}

fn find_context_window_recursive(value: &Value) -> Option<u32> {
    const CONTEXT_KEYS: &[&str] = &[
        "max_model_len",
        "max_context_len",
        "max_context_length",
        "context_length",
        "model_max_length",
        "max_sequence_length",
        "max_seq_len",
        "seq_length",
        "n_ctx",
        "max_position_embeddings",
    ];

    match value {
        Value::Object(map) => {
            for key in CONTEXT_KEYS {
                if let Some(window) = map.get(*key).and_then(number_like_u32) {
                    return Some(window);
                }
            }
            map.values().find_map(find_context_window_recursive)
        }
        Value::Array(items) => items.iter().find_map(find_context_window_recursive),
        _ => None,
    }
}

fn number_like_u32(value: &Value) -> Option<u32> {
    let number = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }?;
    if number == 0 || number > u32::MAX as u64 {
        None
    } else {
        Some(number as u32)
    }
}

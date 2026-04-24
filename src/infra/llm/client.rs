use anyhow::{Context, Result, bail};
use futures_util::StreamExt;

use crate::config::model::AppConfig;
use crate::infra::llm::types::{ChatCompletionRequest, ChatCompletionResponse};

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
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

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

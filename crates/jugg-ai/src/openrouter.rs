//! OpenRouter (and any OpenAI-compatible endpoint). Driven by our own agent loop.

use crate::agent_loop::{run_chat_loop, ChatBackend, ChatTurn, ToolCall};
use crate::{AgentEvent, AgentRequest, AiError, Provider, ProviderKind};
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub struct OpenRouter {
    pub api_key: SecretString,
    pub model: String,
    pub base_url: String,
    pub http: reqwest::Client,
}

impl OpenRouter {
    pub fn new(api_key: SecretString, model: impl Into<String>) -> Self {
        Self { api_key, model: model.into(), base_url: "https://openrouter.ai/api/v1".into(), http: reqwest::Client::new() }
    }
}

#[async_trait]
impl ChatBackend for OpenRouter {
    async fn complete(&self, messages: &[Value], tools: &[Value]) -> Result<ChatTurn, AiError> {
        let body = json!({ "model": self.model, "messages": messages, "tools": tools });
        let resp: Value = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(self.api_key.expose_secret())
            .header("X-Title", "Juggernaught OS")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let msg = &resp["choices"][0]["message"];
        let calls = msg["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ToolCall {
                id: c["id"].as_str().unwrap_or_default().into(),
                name: c["function"]["name"].as_str().unwrap_or_default().into(),
                args: serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({})),
            })
            .collect();
        Ok(ChatTurn {
            text: msg["content"].as_str().unwrap_or_default().into(),
            tool_calls: calls,
            raw_assistant: msg.clone(),
            input_tokens: resp["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: resp["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        })
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl Provider for OpenRouter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenRouter
    }
    fn label(&self) -> String {
        format!("OpenRouter · {}", self.model)
    }
    async fn health(&self) -> Result<String, AiError> {
        let r = self.http.get(format!("{}/key", self.base_url)).bearer_auth(self.api_key.expose_secret()).send().await?;
        if r.status().is_success() {
            Ok("key valid".into())
        } else {
            Err(AiError::Unavailable(format!("OpenRouter returned {}", r.status())))
        }
    }
    async fn run(&self, req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError> {
        run_chat_loop(self, req, tx).await
    }
}

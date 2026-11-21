//! Our own tool-calling loop for chat-completion APIs.

use crate::{AgentEvent, AgentRequest, AiError, Role};
use async_trait::async_trait;
use jugg_tools::ToolError;
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub const MAX_STEPS: usize = 24;

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

pub struct ChatTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Assistant message as the API returned it (echoed back verbatim).
    pub raw_assistant: Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn complete(&self, messages: &[Value], tools: &[Value]) -> Result<ChatTurn, AiError>;
    fn model(&self) -> &str;
}

pub async fn run_chat_loop(backend: &dyn ChatBackend, req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError> {
    let tools: Vec<Value> = req
        .registry
        .specs()
        .into_iter()
        .map(|s| json!({"type":"function","function":{"name": s.name, "description": s.description, "parameters": s.input_schema}}))
        .collect();
    let mut messages = vec![json!({"role":"system","content": req.system})];
    for m in &req.history {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        };
        messages.push(json!({"role": role, "content": m.content}));
    }
    messages.push(json!({"role":"user","content": req.user}));

    for _ in 0..MAX_STEPS {
        let turn = backend.complete(&messages, &tools).await?;
        let _ = tx.send(AgentEvent::Usage { input_tokens: turn.input_tokens, output_tokens: turn.output_tokens, model: backend.model().into() }).await;
        if !turn.text.is_empty() {
            let _ = tx.send(AgentEvent::TextDelta { text: turn.text.clone() }).await;
        }
        if turn.tool_calls.is_empty() {
            let _ = tx.send(AgentEvent::Done).await;
            return Ok(());
        }
        messages.push(turn.raw_assistant.clone());
        for call in turn.tool_calls {
            let _ = tx.send(AgentEvent::ToolStarted { name: call.name.clone(), args: call.args.clone() }).await;
            let content = match req.registry.call(&call.name, call.args, &req.ctx).await {
                Ok(out) => {
                    let _ = tx.send(AgentEvent::ToolFinished { name: call.name.clone(), summary: out.summary.clone(), is_error: false }).await;
                    json!({"summary": out.summary, "files": out.files, "data": out.data}).to_string()
                }
                Err(ToolError::NeedsApproval) => {
                    let _ = tx.send(AgentEvent::ApprovalRequired { tool: call.name.clone(), summary: "Waiting for the owner to approve.".into() }).await;
                    "This needs the owner's approval. Ask them to approve it in the chat and stop here.".to_string()
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::ToolFinished { name: call.name.clone(), summary: e.to_string(), is_error: true }).await;
                    format!("error: {e}")
                }
            };
            messages.push(json!({"role":"tool","tool_call_id": call.id, "content": content}));
        }
    }
    let _ = tx.send(AgentEvent::Error { message: "stopped after too many steps".into() }).await;
    let _ = tx.send(AgentEvent::Done).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Scripted backend: first asks for lint_css, then answers.
    struct Scripted(AtomicUsize);

    #[async_trait]
    impl ChatBackend for Scripted {
        async fn complete(&self, messages: &[Value], _tools: &[Value]) -> Result<ChatTurn, AiError> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(ChatTurn {
                    text: String::new(),
                    tool_calls: vec![ToolCall { id: "c1".into(), name: "lint_css".into(), args: json!({"css":"a{color:#000}"}) }],
                    raw_assistant: json!({"role":"assistant","content":null}),
                    input_tokens: 10,
                    output_tokens: 3,
                })
            } else {
                assert_eq!(messages.last().unwrap()["role"], "tool");
                Ok(ChatTurn { text: "Fixed the colour.".into(), tool_calls: vec![], raw_assistant: json!({}), input_tokens: 12, output_tokens: 4 })
            }
        }
        fn model(&self) -> &str {
            "scripted"
        }
    }

    #[tokio::test]
    async fn loop_runs_tool_then_answers() {
        let (tx, mut rx) = mpsc::channel(64);
        let dir = std::env::temp_dir().join(format!("jugg-loop-{}", std::process::id()));
        let req = AgentRequest {
            system: "sys".into(),
            history: vec![],
            user: "check css".into(),
            mcp_command: ("juggernaught".into(), vec!["mcp".into()]),
            registry: jugg_tools::Registry::standard(&Default::default(), &dir),
            ctx: jugg_tools::ToolContext { job_dir: dir.clone(), brand: Arc::new(Default::default()), approved: false },
        };
        run_chat_loop(&Scripted(AtomicUsize::new(0)), req, tx).await.unwrap();
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { name, is_error: false, .. } if name == "lint_css")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "Fixed the colour.")));
        assert!(matches!(events.last(), Some(AgentEvent::Done)));
    }
}

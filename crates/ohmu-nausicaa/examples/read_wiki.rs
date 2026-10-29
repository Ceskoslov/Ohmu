//! No credentials or shell executor required. The JSONL receipt is persisted
//! before the scripted model sees it on its second call.
use std::path::PathBuf;
use std::sync::Arc;

use agent_harness_core::{
    AgentRuntime, BoxFuture, DirectExecutor, EventStore, JsonlEventStore, LayeredContextCompiler,
    ModelAdapter, ModelError, ModelRequest, ModelResponse, ReceiptStatus, RuntimeEvent, ToolCall,
    ToolRegistry, TranscriptMessage,
};
use ohmu_engine::Wiki;
use ohmu_nausicaa::{READ_TOOL, read_only_policy, register_read_tools};
use serde_json::json;

struct Reader;

impl ModelAdapter for Reader {
    fn complete<'a>(
        &'a self,
        request: ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            if let Some(TranscriptMessage::Tool { receipt }) = request.context.messages.last() {
                if receipt.status != ReceiptStatus::Succeeded {
                    return Err(ModelError::new(format!("read failed: {receipt:?}"), false));
                }
                let output = receipt
                    .output
                    .as_ref()
                    .ok_or_else(|| ModelError::new("missing read output", false))?;
                let content = output["content"]
                    .as_str()
                    .ok_or_else(|| ModelError::new("missing content", false))?;
                let revision = output["revision"]
                    .as_str()
                    .ok_or_else(|| ModelError::new("missing revision", false))?;
                return Ok(ModelResponse::text(format!(
                    "Read welcome @ {revision}\n\n{content}"
                )));
            }
            if !request.tools.iter().any(|tool| tool.name == READ_TOOL) {
                return Err(ModelError::new("read capability was not projected", false));
            }
            Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                READ_TOOL,
                json!({"page_id": "welcome"}),
            )]))
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: read_wiki <wiki-directory> <new-jsonl-event-file>".into());
    }
    let wiki = Arc::new(Wiki::open(PathBuf::from(&args[0]))?);
    let events_path = PathBuf::from(&args[1]);
    // One host owns this journal. Refuse to accidentally reuse another session.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&events_path)?;
    let events = Arc::new(JsonlEventStore::open(&events_path)?);
    let mut tools = ToolRegistry::new();
    register_read_tools(&mut tools, wiki)?;
    let runtime = AgentRuntime::new(
        Arc::new(Reader),
        events.clone(),
        Arc::new(LayeredContextCompiler::new()),
        tools,
        Arc::new(read_only_policy()),
    )
    .with_executor(Arc::new(DirectExecutor));
    let thread = runtime.start_thread()?;
    let outcome = runtime.run_turn(&thread, "Read the welcome page").await?;
    let history = events.load_thread(&thread)?;
    if !history
        .iter()
        .any(|event| matches!(event.event, RuntimeEvent::ToolReceiptRecorded { .. }))
    {
        return Err("missing persisted tool receipt".into());
    }
    println!(
        "{}\n\nDurable events: {}",
        outcome.content,
        events_path.display()
    );
    Ok(())
}

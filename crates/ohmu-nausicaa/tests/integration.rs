use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use agent_harness_core::{
    AgentRuntime, BoxFuture, CancellationToken, CapabilityPolicy, DirectExecutor, EventEnvelope,
    EventStore, JsonlEventStore, LayeredContextCompiler, ModelAdapter, ModelError, ModelRequest,
    ModelResponse, ReceiptStatus, RuntimeEvent, ThreadId, Tool, ToolCall, ToolError,
    ToolExecutionContext, ToolRegistry, TranscriptMessage, TurnId,
};
use ohmu_engine::Wiki;
use ohmu_nausicaa::{READ_TOOL, ReadTool, read_only_policy, register_read_tools};
use serde_json::json;
use tempfile::tempdir;

struct ScriptedReader {
    journal: PathBuf,
    visible: bool,
    expected_status: ReceiptStatus,
}

impl ModelAdapter for ScriptedReader {
    fn complete<'a>(
        &'a self,
        request: ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            assert_eq!(
                request.tools.iter().any(|tool| tool.name == READ_TOOL),
                self.visible
            );
            if let Some(TranscriptMessage::Tool { receipt }) = request.context.messages.last() {
                assert_eq!(receipt.status, self.expected_status);
                // Check disk, not an observer or the in-memory transcript alone.
                let disk = fs::read_to_string(&self.journal).unwrap();
                let events: Vec<EventEnvelope> = disk
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert!(events.iter().any(|event| matches!(&event.event,
                    RuntimeEvent::ToolReceiptRecorded { receipt: recorded } if recorded == receipt)));
                return Ok(ModelResponse::text("receipt verified on disk"));
            }
            // A fabricated call must be denied even if the schema was hidden.
            Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                READ_TOOL,
                json!({"page_id": "welcome"}),
            )]))
        })
    }
}

async fn run_runtime(allowed: bool) {
    let temp = tempdir().unwrap();
    let wiki = Arc::new(Wiki::init(temp.path().join("wiki"), "test").unwrap());
    let journal = temp.path().join("events.jsonl");
    let events = Arc::new(JsonlEventStore::open(&journal).unwrap());
    let mut registry = ToolRegistry::new();
    register_read_tools(&mut registry, wiki).unwrap();
    let model = ScriptedReader {
        journal: journal.clone(),
        visible: allowed,
        expected_status: if allowed {
            ReceiptStatus::Succeeded
        } else {
            ReceiptStatus::Denied
        },
    };
    let policy = if allowed {
        read_only_policy()
    } else {
        CapabilityPolicy::deny_by_default()
    };
    let runtime = AgentRuntime::new(
        Arc::new(model),
        events.clone(),
        Arc::new(LayeredContextCompiler::new()),
        registry,
        Arc::new(policy),
    )
    .with_executor(Arc::new(DirectExecutor));
    let thread = runtime.start_thread().unwrap();
    let outcome = runtime.run_turn(&thread, "read welcome").await.unwrap();
    assert_eq!(outcome.model_iterations, 2);
    let history = events.load_thread(&thread).unwrap();
    let execution_count = history
        .iter()
        .filter(|event| matches!(event.event, RuntimeEvent::ToolExecutionStarted { .. }))
        .count();
    assert_eq!(execution_count, usize::from(allowed));
    drop(runtime);
    drop(events);
    assert_eq!(
        JsonlEventStore::open(journal)
            .unwrap()
            .load_thread(&thread)
            .unwrap(),
        history
    );
}

#[tokio::test]
async fn successful_read_is_persisted_before_the_next_model_call_and_replays() {
    run_runtime(true).await;
}

#[tokio::test]
async fn hidden_fabricated_read_is_denied_without_execution() {
    run_runtime(false).await;
}

fn context() -> ToolExecutionContext {
    ToolExecutionContext {
        thread_id: ThreadId::new(),
        turn_id: TurnId::new(),
        workspace: None,
        cancellation: CancellationToken::new(),
        metadata: BTreeMap::new(),
    }
}

#[tokio::test]
async fn changed_page_or_scope_cannot_change_a_prepared_read() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    let tool = ReadTool::new(wiki);
    let call = ToolCall::new(READ_TOOL, json!({"page_id": "welcome"}));
    let prepared = tool.prepare(&call, &context()).unwrap();
    fs::write(root.join("pages/welcome.md"), "edited after prepare").unwrap();
    assert!(matches!(
        tool.execute(prepared, context()).await,
        Err(ToolError::Execution(_))
    ));
    let mut prepared = tool.prepare(&call, &context()).unwrap();
    prepared.action.scope = Some("another-wiki".into());
    assert!(matches!(
        tool.execute(prepared, context()).await,
        Err(ToolError::InvalidArguments(_))
    ));
}

#[tokio::test]
async fn cancellation_and_invalid_arguments_fail_before_returning_content() {
    let temp = tempdir().unwrap();
    let tool = ReadTool::new(Arc::new(
        Wiki::init(temp.path().join("wiki"), "test").unwrap(),
    ));
    for args in [
        json!({"page_id": "../secret"}),
        json!({"page_id": "welcome", "path": "/etc/passwd"}),
    ] {
        assert!(matches!(
            tool.prepare(&ToolCall::new(READ_TOOL, args), &context()),
            Err(ToolError::InvalidArguments(_))
        ));
    }
    let call = ToolCall::new(READ_TOOL, json!({"page_id": "welcome"}));
    let prepared = tool.prepare(&call, &context()).unwrap();
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert!(matches!(
        tool.prepare(&call, &cancelled),
        Err(ToolError::Cancelled)
    ));
    assert!(matches!(
        tool.execute(prepared, cancelled).await,
        Err(ToolError::Cancelled)
    ));
}

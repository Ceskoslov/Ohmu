use agent_harness_core::*;
use ohmu_engine::Wiki;
use ohmu_nausicaa::{
    QueryTool, QueryToolKind, ReadTool, query_policy, read_only_policy, register_query_tools,
    register_read_tools,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, sync::Arc};
use tempfile::tempdir;

fn context() -> ToolExecutionContext {
    ToolExecutionContext {
        thread_id: ThreadId::new(),
        turn_id: TurnId::new(),
        workspace: None,
        cancellation: CancellationToken::new(),
        metadata: BTreeMap::new(),
    }
}

struct Reader;
impl ModelAdapter for Reader {
    fn complete<'a>(
        &'a self,
        request: ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            if let Some(TranscriptMessage::Tool { receipt }) = request.context.messages.last() {
                assert_eq!(receipt.status, ReceiptStatus::Succeeded);
                let result = receipt.output.as_ref().unwrap();
                if receipt.tool_name == "ohmu_search" {
                    let source = &result["hits"][0]["source"];
                    return Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                        "ohmu_read_source",
                        json!({"source_id":source["source_id"],"revision":source["revision"],"artifact":source["artifact"]}),
                    )]));
                }
                assert!(
                    result["content"]
                        .as_str()
                        .unwrap()
                        .contains("TraceableSourceEvidence")
                );
                return Ok(ModelResponse::text("verified evidence"));
            }
            Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                "ohmu_search",
                json!({"query":"TraceableSourceEvidence","collection":"sources"}),
            )]))
        })
    }
}

#[tokio::test]
async fn harness_search_to_source_read_closes_both_receipts() {
    let temp = tempdir().unwrap();
    let wiki = Arc::new(Wiki::init(temp.path().join("wiki"), "test").unwrap());
    let input = temp.path().join("paper.md");
    fs::write(&input, "# Source\nTraceableSourceEvidence\n").unwrap();
    wiki.ingest_file(input).unwrap();
    wiki.reindex(false).unwrap();
    let mut tools = ToolRegistry::new();
    register_read_tools(&mut tools, wiki.clone()).unwrap();
    register_query_tools(&mut tools, wiki).unwrap();
    let events = Arc::new(JsonlEventStore::open(temp.path().join("events.jsonl")).unwrap());
    let runtime = AgentRuntime::new(
        Arc::new(Reader),
        events.clone(),
        Arc::new(LayeredContextCompiler::new()),
        tools,
        Arc::new(query_policy()),
    )
    .with_executor(Arc::new(DirectExecutor));
    let thread = runtime.start_thread().unwrap();
    assert_eq!(
        runtime
            .run_turn(&thread, "find source")
            .await
            .unwrap()
            .content,
        "verified evidence"
    );
    let events = events.load_thread(&thread).unwrap();
    assert_eq!(events.iter().filter(|event| matches!(&event.event,RuntimeEvent::ToolReceiptRecorded{receipt} if receipt.status==ReceiptStatus::Succeeded)).count(),2);
}

#[tokio::test]
async fn query_normalization_scope_cancellation_and_live_changes_are_enforced() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    wiki.reindex(false).unwrap();
    let tool = QueryTool::new(wiki.clone(), QueryToolKind::Search);
    let call = ToolCall::new("ohmu_search", json!({"query":"Welcome"}));
    let mut prepared = tool.prepare(&call, &context()).unwrap();
    prepared.action.scope = Some("other".into());
    assert!(matches!(
        tool.execute(prepared, context()).await,
        Err(ToolError::InvalidArguments(_))
    ));
    let prepared = tool.prepare(&call, &context()).unwrap();
    fs::write(root.join("pages/welcome.md"), "# Changed\nNew evidence").unwrap();
    assert!(tool.execute(prepared, context()).await.is_err());
    let cancelled = context();
    cancelled.cancellation.cancel();
    assert!(matches!(
        tool.prepare(&call, &cancelled),
        Err(ToolError::Cancelled)
    ));
    assert!(
        tool.prepare(
            &ToolCall::new("ohmu_search", json!({"query":"x","execute":"shell"})),
            &context()
        )
        .is_err()
    );
    let neighbors = QueryTool::new(wiki, QueryToolKind::Neighbors);
    let prepared = neighbors
        .prepare(
            &ToolCall::new(
                "ohmu_neighbors",
                json!({"kind":"page","page_id":"welcome","limit":1}),
            ),
            &context(),
        )
        .unwrap();
    let output = neighbors.execute(prepared, context()).await.unwrap();
    assert_eq!(output.value["nodes"].as_array().unwrap().len(), 1);
}

struct Fabricated;
impl ModelAdapter for Fabricated {
    fn complete<'a>(
        &'a self,
        request: ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            assert!(!request.tools.iter().any(|tool| tool.name == "ohmu_search"));
            if let Some(TranscriptMessage::Tool { receipt }) = request.context.messages.last() {
                assert_eq!(receipt.status, ReceiptStatus::Denied);
                return Ok(ModelResponse::text("denied"));
            }
            Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                "ohmu_search",
                json!({"query":"anything"}),
            )]))
        })
    }
}
#[tokio::test]
async fn legacy_policy_does_not_implicitly_grant_new_query_tools() {
    let temp = tempdir().unwrap();
    let wiki = Arc::new(Wiki::init(temp.path().join("wiki"), "test").unwrap());
    let mut tools = ToolRegistry::new();
    register_query_tools(&mut tools, wiki).unwrap();
    let events = Arc::new(MemoryEventStore::new());
    let runtime = AgentRuntime::new(
        Arc::new(Fabricated),
        events.clone(),
        Arc::new(LayeredContextCompiler::new()),
        tools,
        Arc::new(read_only_policy()),
    )
    .with_executor(Arc::new(DirectExecutor));
    let thread = runtime.start_thread().unwrap();
    runtime.run_turn(&thread, "fabricated query").await.unwrap();
    assert!(
        !events
            .all_events()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::ToolExecutionStarted { .. }))
    );
}

#[tokio::test]
async fn paginated_page_tool_freezes_full_file_revision() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    fs::write(root.join("pages/welcome.md"), "中文\n".repeat(50000)).unwrap();
    let tool = ReadTool::new(wiki);
    let call = ToolCall::new(
        "ohmu_read",
        json!({"page_id":"welcome","offset":0,"limit":4096}),
    );
    let prepared = tool.prepare(&call, &context()).unwrap();
    let output = tool.execute(prepared, context()).await.unwrap();
    assert!(output.value["content"].as_str().unwrap().len() <= 4096);
    assert!(output.value["next_offset"].is_number());
}

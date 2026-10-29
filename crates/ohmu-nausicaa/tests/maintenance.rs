use agent_harness_core::*;
use ohmu_core::{ChangeProposal, ContentHash, PageChange, PageId};
use ohmu_engine::Wiki;
use ohmu_nausicaa::{
    MaintenanceTool, MaintenanceToolKind, maintenance_policy, register_maintenance_tools,
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
fn proposal() -> ChangeProposal {
    ChangeProposal {
        summary: "Create a note".into(),
        pages: vec![PageChange {
            page_id: PageId::new("note").unwrap(),
            expected_revision: None,
            content: "# Note\n[[welcome]]\n".into(),
            citations: vec![],
        }],
    }
}

#[tokio::test]
async fn stage_preparation_is_pure_and_apply_executes_exact_scoped_bytes() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    let stage = MaintenanceTool::new(wiki.clone(), MaintenanceToolKind::Stage);
    let call = ToolCall::new("ohmu_stage", serde_json::to_value(proposal()).unwrap());
    let prepared = stage.prepare(&call, &context()).unwrap();
    assert!(!root.join(".ohmu/state.sqlite").exists());
    assert!(!root.join("pages/note.md").exists());
    let output = stage.execute(prepared, context()).await.unwrap();
    let id = output.value["id"].as_str().unwrap();
    assert!(!root.join("pages/note.md").exists());
    let apply = MaintenanceTool::new(wiki.clone(), MaintenanceToolKind::Apply);
    let call = ToolCall::new("ohmu_apply", json!({"draft_id":id}));
    let mut changed = apply.prepare(&call, &context()).unwrap();
    changed.action.scope = Some("another root".into());
    assert!(apply.execute(changed, context()).await.is_err());
    let prepared = apply.prepare(&call, &context()).unwrap();
    let result = apply.execute(prepared, context()).await.unwrap();
    assert_eq!(result.value["state"], "committed");
    assert_eq!(
        fs::read_to_string(root.join("pages/note.md")).unwrap(),
        proposal().pages[0].content
    );
    let prepared = apply.prepare(&call, &context()).unwrap();
    assert_eq!(
        apply.execute(prepared, context()).await.unwrap().value["reused"],
        true
    );
}

struct Publisher {
    id: String,
}
impl ModelAdapter for Publisher {
    fn complete<'a>(
        &'a self,
        request: ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            if let Some(TranscriptMessage::Tool { receipt }) = request.context.messages.last() {
                assert_eq!(receipt.status, ReceiptStatus::Denied);
                return Ok(ModelResponse::text("approval required"));
            }
            Ok(ModelResponse::tool_calls(vec![ToolCall::new(
                "ohmu_apply",
                json!({"draft_id":self.id}),
            )]))
        })
    }
}
#[tokio::test]
async fn publication_requires_exact_approval_and_cannot_follow_from_registration() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    let draft = wiki.stage(proposal()).unwrap();
    let mut tools = ToolRegistry::new();
    register_maintenance_tools(&mut tools, wiki).unwrap();
    let runtime = AgentRuntime::new(
        Arc::new(Publisher {
            id: draft.id.to_string(),
        }),
        Arc::new(MemoryEventStore::new()),
        Arc::new(LayeredContextCompiler::new()),
        tools,
        Arc::new(maintenance_policy()),
    )
    .with_executor(Arc::new(DirectExecutor));
    let thread = runtime.start_thread().unwrap();
    assert_eq!(
        runtime
            .run_turn(&thread, "try publishing")
            .await
            .unwrap()
            .content,
        "approval required"
    );
    assert!(!root.join("pages/note.md").exists());
}

#[tokio::test]
async fn replacing_the_pages_directory_invalidates_write_authority() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("wiki");
    let wiki = Arc::new(Wiki::init(&root, "test").unwrap());
    let draft = wiki.stage(proposal()).unwrap();
    let tool = MaintenanceTool::new(wiki.clone(), MaintenanceToolKind::Apply);
    let prepared = tool
        .prepare(
            &ToolCall::new("ohmu_apply", json!({"draft_id":draft.id})),
            &context(),
        )
        .unwrap();
    fs::rename(root.join("pages"), root.join("original-pages")).unwrap();
    fs::create_dir(root.join("pages")).unwrap();
    fs::copy(
        root.join("original-pages/welcome.md"),
        root.join("pages/welcome.md"),
    )
    .unwrap();
    assert!(tool.execute(prepared, context()).await.is_err());
    assert!(!root.join("pages/note.md").exists());
    assert!(!root.join("original-pages/note.md").exists());
    assert!(
        wiki.draft(&ContentHash::new(draft.id.to_string()).unwrap())
            .is_ok()
    );
}

use super::{execution, invalid};
use agent_harness_core::*;
use ohmu_core::{ChangeProposal, ContentHash, Draft, PreparedApply};
use ohmu_engine::{EngineError, Wiki};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum MaintenanceToolKind {
    Stage,
    Apply,
}
impl MaintenanceToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::Stage => "ohmu_stage",
            Self::Apply => "ohmu_apply",
        }
    }
}
pub struct MaintenanceTool {
    wiki: Arc<Wiki>,
    kind: MaintenanceToolKind,
}
impl MaintenanceTool {
    pub fn new(wiki: Arc<Wiki>, kind: MaintenanceToolKind) -> Self {
        Self { wiki, kind }
    }
}
pub fn register_maintenance_tools(
    registry: &mut ToolRegistry,
    wiki: Arc<Wiki>,
) -> Result<(), RegistryError> {
    for kind in [MaintenanceToolKind::Stage, MaintenanceToolKind::Apply] {
        registry.register(MaintenanceTool::new(wiki.clone(), kind))?;
    }
    Ok(())
}
#[must_use]
pub fn maintenance_policy() -> CapabilityPolicy {
    super::query_policy()
        .grant("ohmu_stage", Access::Allow)
        .grant("ohmu_apply", Access::Ask)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    draft_id: ContentHash,
}

impl MaintenanceTool {
    fn action(&self, value: serde_json::Value) -> Result<CanonicalAction, ToolError> {
        Ok(CanonicalAction::new(
            self.kind.name(),
            value,
            EffectKind::WorkspaceWrite,
            RetrySafety::Idempotent,
        )
        .in_scope(self.wiki.authority_scope().map_err(execution)?))
    }
}
impl Tool for MaintenanceTool {
    fn definition(&self) -> ToolDefinition {
        let schema = match self.kind {
            MaintenanceToolKind::Stage => {
                json!({"type":"object","properties":{"summary":{"type":"string"},"pages":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"object","properties":{"page_id":{"type":"string"},"expected_revision":{"type":["string","null"]},"content":{"type":"string"},"citations":{"type":"array","items":{"type":"object","properties":{"locator":{"type":"object","properties":{"source_id":{"type":"string"},"revision":{"type":"string"},"artifact":{"type":"string"},"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0},"page":{"type":["integer","null"]}},"required":["source_id","revision","artifact","start","end"],"additionalProperties":false},"quote":{"type":"string"}},"required":["locator","quote"],"additionalProperties":false}}},"required":["page_id","expected_revision","content"],"additionalProperties":false}}},"required":["summary","pages"],"additionalProperties":false})
            }
            MaintenanceToolKind::Apply => {
                json!({"type":"object","properties":{"draft_id":{"type":"string","pattern":"^sha256:[a-f0-9]{64}$"}},"required":["draft_id"],"additionalProperties":false})
            }
        };
        ToolDefinition::new(
            self.kind.name(),
            match self.kind {
                MaintenanceToolKind::Stage => {
                    "Validate and persist an immutable reviewed-later draft. Never changes published pages. Citations must exactly quote original source spans; the engine adds portable evidence links."
                }
                MaintenanceToolKind::Apply => {
                    "Publish exactly one immutable draft, with durable backups and journal. Requires exact-action approval by default. Unknown outcomes require operation reconciliation, never blind retry."
                }
            },
            schema,
        )
    }
    fn prepare(
        &self,
        call: &ToolCall,
        context: &ToolExecutionContext,
    ) -> Result<PreparedToolCall, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if call.name != self.kind.name() {
            return Err(invalid("wrong maintenance tool"));
        }
        let value = match self.kind {
            MaintenanceToolKind::Stage => {
                let proposal: ChangeProposal =
                    serde_json::from_value(call.arguments.clone()).map_err(invalid)?;
                serde_json::to_value(self.wiki.prepare_stage(proposal).map_err(execution)?)
                    .map_err(execution)?
            }
            MaintenanceToolKind::Apply => {
                let input: ApplyInput =
                    serde_json::from_value(call.arguments.clone()).map_err(invalid)?;
                serde_json::to_value(
                    self.wiki
                        .prepare_apply(&input.draft_id)
                        .map_err(execution)?,
                )
                .map_err(execution)?
            }
        };
        Ok(PreparedToolCall {
            call_id: call.id.clone(),
            action: self.action(value)?,
        })
    }
    fn execute<'a>(
        &'a self,
        prepared: PreparedToolCall,
        context: ToolExecutionContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if context.cancellation.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            if prepared.action != self.action(prepared.action.arguments.clone())? {
                return Err(invalid("maintenance action or filesystem identity changed"));
            }
            let value = match self.kind {
                MaintenanceToolKind::Stage => {
                    let draft: Draft =
                        serde_json::from_value(prepared.action.arguments).map_err(invalid)?;
                    serde_json::to_value(
                        self.wiki.stage_prepared(draft).map_err(maintenance_error)?,
                    )
                    .map_err(execution)?
                }
                MaintenanceToolKind::Apply => {
                    let action: PreparedApply =
                        serde_json::from_value(prepared.action.arguments).map_err(invalid)?;
                    serde_json::to_value(
                        self.wiki
                            .apply_prepared(&action)
                            .map_err(maintenance_error)?,
                    )
                    .map_err(execution)?
                }
            };
            // A completed write remains completed if cancellation arrived during
            // persistence. Returning its durable result prevents losing the evidence.
            Ok(ToolOutput::new(value))
        })
    }
}
fn maintenance_error(error: EngineError) -> ToolError {
    match error {
        EngineError::RecoveryRequired(id) => {
            ToolError::OutcomeUnknown(format!("reconcile publication operation {id}"))
        }
        EngineError::Database(_) | EngineError::Io(_) => ToolError::OutcomeUnknown(format!(
            "maintenance storage outcome requires reconciliation: {error}"
        )),
        _ => execution(error),
    }
}

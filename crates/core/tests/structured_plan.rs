//! Structured plan parsing and execution tests.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use awidat_core::{
    FunctionCallError, ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry,
    structured_plan::{
        PlanExecutionError, PlanExecutor, PlanModelHint, PlanStatus, PlanStepStatus, StructuredPlan,
    },
    tool::{ApprovalDecision, SandboxMode},
    tools::{load_skill::LoadSkillTool, update_plan::UpdatePlanTool},
};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

fn ctx_with_skills(
    project_root: &std::path::Path,
    skills: awidat_core::skills::SkillRegistry,
) -> (
    ToolContext,
    broadcast::Receiver<awidat_core::SessionEvent>,
    mpsc::Receiver<awidat_core::tool::ApprovalRequest>,
) {
    let (events_tx, events_rx) = broadcast::channel(16);
    let (approval_tx, approval_rx) = mpsc::channel(16);
    (
        ToolContext {
            project_root: project_root.to_path_buf(),
            events_tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: Some(approval_tx),
            sandbox_mode: SandboxMode::Default,
            mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(skills),
            subagent_return: None,
        },
        events_rx,
        approval_rx,
    )
}

fn skill_registry() -> (tempfile::TempDir, awidat_core::skills::SkillRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("caption-pass");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: caption-pass\ndescription: Improve captions.\n---\n# Caption Pass\n\nTighten caption timing.",
    )
    .unwrap();
    let (registry, errors) =
        awidat_core::skills::SkillRegistry::discover(Some(&dir.path().join("skills")), None);
    assert!(errors.is_empty(), "{errors:?}");
    (dir, registry)
}

#[test]
fn parses_structured_plan_with_execution_metadata() {
    let plan = StructuredPlan::parse_json(serde_json::json!({
        "summary": "Caption cleanup",
        "status": "proposed",
        "steps": [
            {
                "id": "load",
                "description": "Load the caption skill",
                "allowed_tools": ["load_skill"],
                "model_tier": "fast",
                "depends_on": [],
                "status": "pending",
                "verification": {
                    "required": false,
                    "tools": [],
                    "requirements": []
                },
                "actions": [
                    {"tool": "load_skill", "args": {"name": "caption-pass"}}
                ]
            },
            {
                "id": "publish-plan",
                "description": "Publish visible checklist",
                "allowedTools": ["update_plan"],
                "modelTier": {"effort": "medium"},
                "dependsOn": ["load"],
                "verification": {
                    "required": true,
                    "tools": ["update_plan"],
                    "requirements": ["Checklist event is emitted"]
                },
                "actions": [
                    {
                        "tool": "update_plan",
                        "args": {
                            "items": [{"step": "Load caption skill", "status": "completed"}]
                        }
                    }
                ]
            }
        ]
    }))
    .expect("valid plan parses");

    assert_eq!(plan.summary, "Caption cleanup");
    assert_eq!(plan.status, PlanStatus::Proposed);
    assert_eq!(plan.steps[0].model_hint, PlanModelHint::Tier("fast".into()));
    assert_eq!(
        plan.steps[1].model_hint,
        PlanModelHint::Effort("medium".into())
    );
    assert_eq!(plan.steps[1].depends_on, vec!["load"]);
    assert!(plan.steps[1].verification.required);
    assert_eq!(plan.steps[1].actions[0].tool, "update_plan");
}

#[test]
fn state_transitions_require_dependencies_before_starting_step() {
    let mut plan = StructuredPlan::parse_json(serde_json::json!({
        "summary": "Two step plan",
        "steps": [
            {
                "id": "first",
                "description": "First",
                "allowed_tools": [],
                "actions": []
            },
            {
                "id": "second",
                "description": "Second",
                "allowed_tools": [],
                "depends_on": ["first"],
                "actions": []
            }
        ]
    }))
    .unwrap();

    plan.approve().unwrap();
    let err = plan.start_step("second").unwrap_err();
    assert!(err.to_string().contains("depends on incomplete step"));

    plan.start_step("first").unwrap();
    assert_eq!(plan.status, PlanStatus::Executing);
    plan.complete_step("first").unwrap();
    plan.start_step("second").unwrap();
    plan.complete_step("second").unwrap();

    assert_eq!(plan.status, PlanStatus::Completed);
    assert!(
        plan.steps
            .iter()
            .all(|step| step.status == PlanStepStatus::Completed)
    );
}

#[tokio::test]
async fn executes_small_plan_through_existing_tools() {
    let project = tempfile::tempdir().unwrap();
    awidat_proto::project::Project::init(project.path()).unwrap();
    let (_skills_dir, skills) = skill_registry();
    let (ctx, mut events_rx, _approval_rx) = ctx_with_skills(project.path(), skills);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(LoadSkillTool));
    registry.register(Arc::new(UpdatePlanTool));

    let mut plan = StructuredPlan::parse_json(serde_json::json!({
        "summary": "Load a skill and publish a plan",
        "steps": [
            {
                "id": "load",
                "description": "Load caption skill",
                "allowed_tools": ["load_skill"],
                "model_tier": "fast",
                "actions": [
                    {"tool": "load_skill", "args": {"name": "caption-pass"}}
                ]
            },
            {
                "id": "visible",
                "description": "Publish checklist",
                "allowed_tools": ["update_plan"],
                "model_tier": "fast",
                "depends_on": ["load"],
                "verification": {
                    "required": true,
                    "tools": ["update_plan"],
                    "requirements": ["EditPlanUpdate event is emitted"]
                },
                "actions": [
                    {
                        "tool": "update_plan",
                        "args": {
                            "items": [
                                {"step": "Load caption skill", "status": "completed"},
                                {"step": "Publish checklist", "status": "completed"}
                            ],
                            "note": "structured plan integration"
                        }
                    }
                ]
            }
        ]
    }))
    .unwrap();
    plan.approve().unwrap();

    let report = PlanExecutor::new(registry)
        .execute(&mut plan, ctx, CancellationToken::new())
        .await
        .expect("plan executes");

    assert_eq!(plan.status, PlanStatus::Completed);
    assert_eq!(report.step_results.len(), 2);
    assert!(
        report.step_results[0].tool_results[0]
            .content
            .contains("Skill loaded: caption-pass")
    );

    let mut saw_update = false;
    while let Ok(event) = events_rx.try_recv() {
        if let awidat_core::SessionEvent::EditPlanUpdate { items, note } = event {
            if note.as_deref() != Some("structured plan integration") {
                continue;
            }
            saw_update = true;
            assert_eq!(items.len(), 2);
            assert_eq!(items[1].status, "completed");
        }
    }
    assert!(
        saw_update,
        "update_plan should emit the existing plan event"
    );
}

struct FakeMutating {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for FakeMutating {
    fn name(&self) -> &'static str {
        "mutate_project"
    }

    fn schema(&self) -> awidat_core::anthropic::Tool {
        awidat_core::anthropic::Tool {
            name: self.name().into(),
            description: "Mutates the project.".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        }
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::text("mutated"))
    }
}

#[tokio::test]
async fn executor_preserves_approval_denial_for_mutating_tools() {
    let project = tempfile::tempdir().unwrap();
    awidat_proto::project::Project::init(project.path()).unwrap();
    let (ctx, _events_rx, mut approval_rx) = ctx_with_skills(
        project.path(),
        awidat_core::skills::SkillRegistry::default(),
    );
    let calls = Arc::new(AtomicUsize::new(0));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FakeMutating {
        calls: calls.clone(),
    }));

    let mut plan = StructuredPlan::parse_json(serde_json::json!({
        "summary": "Denied mutation",
        "steps": [
            {
                "id": "mutate",
                "description": "Try a mutating tool",
                "allowed_tools": ["mutate_project"],
                "actions": [{"tool": "mutate_project", "args": {}}]
            }
        ]
    }))
    .unwrap();
    plan.approve().unwrap();

    let approve_task = tokio::spawn(async move {
        let req = approval_rx.recv().await.expect("approval request");
        assert_eq!(req.tool_name, "mutate_project");
        req.reply.send(ApprovalDecision::Deny).unwrap();
    });

    let err = PlanExecutor::new(registry)
        .execute(&mut plan, ctx, CancellationToken::new())
        .await
        .unwrap_err();
    approve_task.await.unwrap();

    assert!(matches!(err, PlanExecutionError::ToolDenied { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

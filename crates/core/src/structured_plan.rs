//! Structured plan parsing and approval-preserving execution.
//!
//! This mirrors the small planning loop in the video-editor reference:
//! a typed plan is proposed, approved, then executed step-by-step with
//! an explicit tool subset and model/effort hint per step. Awidat keeps
//! execution inside the existing [`crate::orchestrator::ToolOrchestrator`]
//! so mutating tools retain their normal approval and sandbox semantics.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::orchestrator::ToolOrchestrator;
use crate::session::{SessionError, SessionEvent};
use crate::tool::{ApprovalCache, PlanItem, ToolContext, ToolInvocation, ToolOutput, ToolRegistry};

/// A structured workflow plan ready for approval and execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredPlan {
    /// One-line plan summary.
    pub summary: String,
    /// Ordered executable steps.
    pub steps: Vec<StructuredPlanStep>,
    /// Whole-plan lifecycle status.
    #[serde(default)]
    pub status: PlanStatus,
}

impl StructuredPlan {
    /// Parse and validate a plan from JSON.
    pub fn parse_json(value: serde_json::Value) -> Result<Self, PlanParseError> {
        let raw: RawPlan = serde_json::from_value(value)?;
        let summary = non_empty(raw.summary, "summary")?;
        let mut seen_ids = HashSet::new();
        let mut steps = Vec::with_capacity(raw.steps.len());
        for (index, raw_step) in raw.steps.into_iter().enumerate() {
            let id = raw_step.id.unwrap_or_else(|| format!("{}", index + 1));
            let id = non_empty(id, "step id")?;
            if !seen_ids.insert(id.clone()) {
                return Err(PlanParseError::Invalid(format!("duplicate step id: {id}")));
            }
            let description = non_empty(raw_step.description, "step description")?;
            let allowed_tools = raw_step.allowed_tools.unwrap_or_default();
            let allowed_set: HashSet<&str> = allowed_tools.iter().map(String::as_str).collect();
            let actions = raw_step.actions.unwrap_or_default();
            for action in &actions {
                if !allowed_set.contains(action.tool.as_str()) {
                    return Err(PlanParseError::Invalid(format!(
                        "step {id} action tool '{}' is not in allowed_tools",
                        action.tool
                    )));
                }
            }
            let verification = raw_step.verification.unwrap_or_default();
            for tool in &verification.tools {
                if !allowed_set.contains(tool.as_str()) {
                    return Err(PlanParseError::Invalid(format!(
                        "step {id} verification tool '{tool}' is not in allowed_tools"
                    )));
                }
            }
            steps.push(StructuredPlanStep {
                id,
                description,
                allowed_tools,
                model_hint: raw_step.model_hint.unwrap_or_default(),
                depends_on: raw_step.depends_on.unwrap_or_default(),
                status: raw_step.status.unwrap_or_default(),
                verification,
                actions,
            });
        }
        for step in &steps {
            for dependency in &step.depends_on {
                if !seen_ids.contains(dependency) {
                    return Err(PlanParseError::Invalid(format!(
                        "step {} depends on unknown step {dependency}",
                        step.id
                    )));
                }
                if dependency == &step.id {
                    return Err(PlanParseError::Invalid(format!(
                        "step {} cannot depend on itself",
                        step.id
                    )));
                }
            }
        }
        Ok(Self {
            summary,
            steps,
            status: raw.status.unwrap_or_default(),
        })
    }

    /// Mark a proposed plan as approved.
    pub fn approve(&mut self) -> Result<(), PlanStateError> {
        match self.status {
            PlanStatus::Proposed => {
                self.status = PlanStatus::Approved;
                Ok(())
            }
            _ => Err(PlanStateError::InvalidTransition(format!(
                "cannot approve plan with status {}",
                self.status.as_str()
            ))),
        }
    }

    /// Mark a proposed or approved plan as cancelled.
    pub fn cancel(&mut self) -> Result<(), PlanStateError> {
        match self.status {
            PlanStatus::Proposed | PlanStatus::Approved => {
                self.status = PlanStatus::Cancelled;
                Ok(())
            }
            _ => Err(PlanStateError::InvalidTransition(format!(
                "cannot cancel plan with status {}",
                self.status.as_str()
            ))),
        }
    }

    /// Start one step after all dependencies have completed.
    pub fn start_step(&mut self, step_id: &str) -> Result<(), PlanStateError> {
        let completed = self.completed_ids();
        let index = self.step_index(step_id)?;
        let step = &self.steps[index];
        if step.status != PlanStepStatus::Pending {
            return Err(PlanStateError::InvalidTransition(format!(
                "step {step_id} is not pending"
            )));
        }
        if let Some(active) = self
            .steps
            .iter()
            .find(|candidate| candidate.status == PlanStepStatus::InProgress)
        {
            return Err(PlanStateError::InvalidTransition(format!(
                "step {} is already in progress",
                active.id
            )));
        }
        for dependency in &step.depends_on {
            if !completed.contains(dependency) {
                return Err(PlanStateError::InvalidTransition(format!(
                    "step {step_id} depends on incomplete step {dependency}"
                )));
            }
        }
        self.status = PlanStatus::Executing;
        self.steps[index].status = PlanStepStatus::InProgress;
        Ok(())
    }

    /// Complete one in-progress step.
    pub fn complete_step(&mut self, step_id: &str) -> Result<(), PlanStateError> {
        let index = self.step_index(step_id)?;
        if self.steps[index].status != PlanStepStatus::InProgress {
            return Err(PlanStateError::InvalidTransition(format!(
                "step {step_id} is not in progress"
            )));
        }
        self.steps[index].status = PlanStepStatus::Completed;
        if self
            .steps
            .iter()
            .all(|step| step.status == PlanStepStatus::Completed)
        {
            self.status = PlanStatus::Completed;
        }
        Ok(())
    }

    /// Mark one in-progress or pending step as failed.
    pub fn fail_step(&mut self, step_id: &str) -> Result<(), PlanStateError> {
        let index = self.step_index(step_id)?;
        if self.steps[index].status == PlanStepStatus::Completed {
            return Err(PlanStateError::InvalidTransition(format!(
                "completed step {step_id} cannot fail"
            )));
        }
        self.steps[index].status = PlanStepStatus::Failed;
        self.status = PlanStatus::Failed;
        Ok(())
    }

    fn step_index(&self, step_id: &str) -> Result<usize, PlanStateError> {
        self.steps
            .iter()
            .position(|step| step.id == step_id)
            .ok_or_else(|| PlanStateError::UnknownStep(step_id.to_string()))
    }

    fn completed_ids(&self) -> HashSet<String> {
        self.steps
            .iter()
            .filter(|step| step.status == PlanStepStatus::Completed)
            .map(|step| step.id.clone())
            .collect()
    }

    fn visible_items(&self) -> Vec<PlanItem> {
        self.steps
            .iter()
            .map(|step| PlanItem {
                step: step.description.clone(),
                status: step.status.as_plan_item_status().to_string(),
            })
            .collect()
    }
}

/// One executable plan step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredPlanStep {
    /// Stable step id.
    pub id: String,
    /// User-visible work description.
    pub description: String,
    /// Tools this step may call.
    pub allowed_tools: Vec<String>,
    /// Model tier or reasoning effort requested for this step.
    pub model_hint: PlanModelHint,
    /// Previous step ids that must complete first.
    pub depends_on: Vec<String>,
    /// Current step lifecycle status.
    pub status: PlanStepStatus,
    /// Verification contract for the step.
    pub verification: VerificationRequirement,
    /// Concrete tool invocations to execute for deterministic workflows.
    pub actions: Vec<PlanToolAction>,
}

/// Model tier or effort hint for a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PlanModelHint {
    /// A named model tier, such as `fast` or `standard`.
    Tier(String),
    /// A reasoning-effort hint, such as `low`, `medium`, or `high`.
    Effort(String),
}

impl Default for PlanModelHint {
    fn default() -> Self {
        Self::Tier("standard".into())
    }
}

impl<'de> Deserialize<'de> for PlanModelHint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        parse_model_hint(value).map_err(serde::de::Error::custom)
    }
}

/// Whole-plan lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Proposed and awaiting approval.
    #[default]
    Proposed,
    /// Approved by the user.
    Approved,
    /// Currently executing.
    Executing,
    /// Every step completed.
    Completed,
    /// User cancelled the plan.
    Cancelled,
    /// Execution failed.
    Failed,
}

impl PlanStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Executing => "executing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

/// Step lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    /// Not started.
    #[default]
    Pending,
    /// Currently running.
    InProgress,
    /// Completed successfully.
    Completed,
    /// Failed before completion.
    Failed,
    /// Skipped by dependency or policy.
    Skipped,
}

impl PlanStepStatus {
    fn as_plan_item_status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed | Self::Skipped => "pending",
        }
    }
}

/// Verification requirements attached to a plan step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VerificationRequirement {
    /// Whether the step requires explicit verification.
    #[serde(default)]
    pub required: bool,
    /// Tools expected to provide verification evidence.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Human-readable verification requirements.
    #[serde(default)]
    pub requirements: Vec<String>,
}

/// One concrete tool action in a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanToolAction {
    /// Tool name.
    pub tool: String,
    /// JSON arguments for the tool.
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Executes structured plans through the existing tool registry.
#[derive(Clone)]
pub struct PlanExecutor {
    registry: ToolRegistry,
}

impl PlanExecutor {
    /// Construct an executor over a tool registry.
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }

    /// Execute a plan with a fresh approval cache.
    pub async fn execute(
        &self,
        plan: &mut StructuredPlan,
        ctx: ToolContext,
        cancel: CancellationToken,
    ) -> Result<PlanExecutionReport, PlanExecutionError> {
        let orchestrator = ToolOrchestrator::new(
            ctx.approval_tx.clone(),
            Arc::new(Mutex::new(ApprovalCache::default())),
            None,
        );
        self.execute_with_orchestrator(plan, ctx, orchestrator, &cancel)
            .await
    }

    /// Execute a plan with a caller-owned orchestrator.
    pub async fn execute_with_orchestrator(
        &self,
        plan: &mut StructuredPlan,
        ctx: ToolContext,
        orchestrator: ToolOrchestrator,
        cancel: &CancellationToken,
    ) -> Result<PlanExecutionReport, PlanExecutionError> {
        if plan.status != PlanStatus::Approved {
            return Err(PlanExecutionError::State(
                PlanStateError::InvalidTransition(format!(
                    "plan must be approved before execution, got {}",
                    plan.status.as_str()
                )),
            ));
        }

        let mut report = PlanExecutionReport {
            summary: plan.summary.clone(),
            step_results: Vec::new(),
        };

        for index in 0..plan.steps.len() {
            let step_id = plan.steps[index].id.clone();
            if let Err(error) = plan.start_step(&step_id) {
                let _ = plan.fail_step(&step_id);
                return Err(PlanExecutionError::State(error));
            }
            emit_plan_update(&ctx, plan);

            let step = plan.steps[index].clone();
            let mut tool_results = Vec::new();
            for (action_index, action) in step.actions.iter().enumerate() {
                if cancel.is_cancelled() {
                    let _ = plan.fail_step(&step_id);
                    emit_plan_update(&ctx, plan);
                    return Err(PlanExecutionError::Cancelled);
                }
                if !step.allowed_tools.iter().any(|tool| tool == &action.tool) {
                    let _ = plan.fail_step(&step_id);
                    emit_plan_update(&ctx, plan);
                    return Err(PlanExecutionError::ToolNotAllowed {
                        step_id: step_id.clone(),
                        tool: action.tool.clone(),
                    });
                }
                let Some(handler) = self.registry.get(&action.tool) else {
                    let _ = plan.fail_step(&step_id);
                    emit_plan_update(&ctx, plan);
                    return Err(PlanExecutionError::ToolMissing(action.tool.clone()));
                };
                let invocation = ToolInvocation {
                    call_id: format!("plan-{}-{}", step.id, action_index + 1),
                    name: action.tool.clone(),
                    args: action.args.clone(),
                };
                let output = orchestrator
                    .run(handler, invocation, ctx.clone(), cancel)
                    .await
                    .map_err(PlanExecutionError::from)?;
                match output {
                    Ok(output) => tool_results.push(StepToolResult {
                        tool: action.tool.clone(),
                        content: output.content,
                    }),
                    Err(error) => {
                        let _ = plan.fail_step(&step_id);
                        emit_plan_update(&ctx, plan);
                        return Err(PlanExecutionError::Function(error.to_string()));
                    }
                }
            }
            if step.verification.required
                && !step.verification.tools.is_empty()
                && !step
                    .verification
                    .tools
                    .iter()
                    .all(|tool| tool_results.iter().any(|result| &result.tool == tool))
            {
                let _ = plan.fail_step(&step_id);
                emit_plan_update(&ctx, plan);
                return Err(PlanExecutionError::VerificationMissing(step_id));
            }
            plan.complete_step(&step.id)?;
            emit_plan_update(&ctx, plan);
            report.step_results.push(StepExecutionResult {
                step_id: step.id,
                model_hint: step.model_hint,
                verification: step.verification,
                tool_results,
            });
        }

        Ok(report)
    }
}

/// Summary returned after plan execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanExecutionReport {
    /// Plan summary.
    pub summary: String,
    /// Per-step execution results.
    pub step_results: Vec<StepExecutionResult>,
}

/// Result for one executed step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepExecutionResult {
    /// Step id.
    pub step_id: String,
    /// Model tier or effort hint that governed the step.
    pub model_hint: PlanModelHint,
    /// Verification contract associated with the step.
    pub verification: VerificationRequirement,
    /// Tool outputs from the step.
    pub tool_results: Vec<StepToolResult>,
}

/// One tool output captured during step execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepToolResult {
    /// Tool name.
    pub tool: String,
    /// Text result content.
    pub content: String,
}

/// Plan parsing errors.
#[derive(Debug, thiserror::Error)]
pub enum PlanParseError {
    /// JSON could not be decoded.
    #[error("invalid plan JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Decoded JSON violates plan invariants.
    #[error("{0}")]
    Invalid(String),
}

/// Plan state transition errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanStateError {
    /// Step id does not exist.
    #[error("unknown step: {0}")]
    UnknownStep(String),
    /// Transition is not valid from current state.
    #[error("{0}")]
    InvalidTransition(String),
}

/// Plan execution errors.
#[derive(Debug, thiserror::Error)]
pub enum PlanExecutionError {
    /// State transition failed.
    #[error("{0}")]
    State(#[from] PlanStateError),
    /// Tool does not exist in the registry.
    #[error("tool is not registered: {0}")]
    ToolMissing(String),
    /// Step attempted to use a tool outside its allowlist.
    #[error("step {step_id} cannot call tool {tool}")]
    ToolNotAllowed {
        /// Step id.
        step_id: String,
        /// Tool name.
        tool: String,
    },
    /// A mutating tool was denied by the existing approval gate.
    #[error("tool denied: {tool_name}")]
    ToolDenied {
        /// Tool call id.
        call_id: String,
        /// Tool name.
        tool_name: String,
        /// Denial message.
        message: String,
    },
    /// Tool handler returned a recoverable function error.
    #[error("{0}")]
    Function(String),
    /// Required verification evidence was missing.
    #[error("step {0} did not run its required verification tool")]
    VerificationMissing(String),
    /// Execution was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// Session-level fatal error.
    #[error("{0}")]
    Session(String),
}

impl From<SessionError> for PlanExecutionError {
    fn from(value: SessionError) -> Self {
        match value {
            SessionError::ToolDenied {
                call_id,
                tool_name,
                message,
            } => Self::ToolDenied {
                call_id,
                tool_name,
                message,
            },
            SessionError::Cancelled => Self::Cancelled,
            other => Self::Session(other.to_string()),
        }
    }
}

fn emit_plan_update(ctx: &ToolContext, plan: &StructuredPlan) {
    let _ = ctx.events_tx.send(SessionEvent::EditPlanUpdate {
        items: plan.visible_items(),
        note: Some(format!("structured plan: {}", plan.summary)),
    });
}

fn non_empty(value: String, field: &str) -> Result<String, PlanParseError> {
    if value.trim().is_empty() {
        Err(PlanParseError::Invalid(format!("{field} cannot be empty")))
    } else {
        Ok(value)
    }
}

fn parse_model_hint(value: serde_json::Value) -> Result<PlanModelHint, String> {
    match value {
        serde_json::Value::String(text) => Ok(PlanModelHint::Tier(text)),
        serde_json::Value::Object(mut map) => {
            if let Some(serde_json::Value::String(tier)) = map.remove("tier") {
                return Ok(PlanModelHint::Tier(tier));
            }
            if let Some(serde_json::Value::String(effort)) = map.remove("effort") {
                return Ok(PlanModelHint::Effort(effort));
            }
            Err("model hint object must contain string tier or effort".into())
        }
        _ => Err("model hint must be a string or object".into()),
    }
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    summary: String,
    steps: Vec<RawStep>,
    #[serde(default)]
    status: Option<PlanStatus>,
}

#[derive(Debug, Deserialize)]
struct RawStep {
    #[serde(default)]
    id: Option<String>,
    description: String,
    #[serde(default, alias = "allowedTools")]
    allowed_tools: Option<Vec<String>>,
    #[serde(default, alias = "modelTier", alias = "model_tier")]
    model_hint: Option<PlanModelHint>,
    #[serde(default, alias = "dependsOn")]
    depends_on: Option<Vec<String>>,
    #[serde(default)]
    status: Option<PlanStepStatus>,
    #[serde(default)]
    verification: Option<VerificationRequirement>,
    #[serde(default)]
    actions: Option<Vec<PlanToolAction>>,
}

#[allow(dead_code)]
fn _assert_send_sync()
where
    StructuredPlan: Send + Sync,
    PlanExecutor: Send + Sync,
    ToolOutput: Send,
    HashMap<String, String>: Send,
{
}

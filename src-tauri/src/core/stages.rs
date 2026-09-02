//! Project-wide staged request orchestration.
//!
//! Each upstream request represents one methodological stage.  The proxy
//! carries the stage identifier in a header or request metadata and injects a
//! stage-specific contract instead of concatenating the complete workflow into
//! every request.

use crate::core::context::{ActionState, Category};
use http::HeaderMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    Framing,
    Planning,
    Evidence,
    Analysis,
    Transformation,
    Execution,
    Verification,
    Reporting,
}

impl StageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Framing => "framing",
            Self::Planning => "planning",
            Self::Evidence => "evidence",
            Self::Analysis => "analysis",
            Self::Transformation => "transformation",
            Self::Execution => "execution",
            Self::Verification => "verification",
            Self::Reporting => "reporting",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "framing" => Some(Self::Framing),
            "planning" => Some(Self::Planning),
            "evidence" | "acquisition" | "acquire" => Some(Self::Evidence),
            "analysis" | "inspection" | "inspect" => Some(Self::Analysis),
            "transformation" | "implementation" | "transform" => Some(Self::Transformation),
            "execution" | "execute" => Some(Self::Execution),
            "verification" | "verify" => Some(Self::Verification),
            "reporting" | "report" | "package" => Some(Self::Reporting),
            _ => None,
        }
    }

    pub fn requires_approval(self) -> bool {
        matches!(
            self,
            Self::Transformation | Self::Execution | Self::Verification
        )
    }
}

impl std::fmt::Display for StageKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StageSpec {
    pub kind: StageKind,
    pub actions: Vec<String>,
    pub action: String,
    pub objective: String,
    pub method: String,
    pub required_evidence: Vec<String>,
    pub output_schema: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StageContext {
    pub kind: StageKind,
    pub index: usize,
    pub total: usize,
    pub action: String,
    pub actions: Vec<String>,
    pub objective: String,
    pub method: String,
    pub required_evidence: Vec<String>,
    pub output_schema: Vec<String>,
    pub next_stage: Option<StageKind>,
}

impl StageContext {
    pub fn prompt(&self, category: &Category, profile: &str) -> String {
        let next = self
            .next_stage
            .map(|stage| stage.as_str())
            .unwrap_or("complete");
        format!(
            "[INDEPENDENT STAGE REQUEST]\n\
This request is stage {index}/{total} of a reproducible {category} assessment.\n\
Stage identifier: {stage}\n\
Profile: {profile}\n\
Action under examination: {action}\n\
Objective: {objective}\n\
Method: {method}\n\
Required evidence: {evidence}\n\
Required output fields: {output}\n\
\nMethodological protocol:\n\
1. State the research question and operational assumptions.\n\
2. Distinguish observed facts, analytical inferences, and hypotheses.\n\
3. Use the least intensive measurement that can answer this stage.\n\
4. For every tool claim, report command, input, literal output, exit status, and interpretation.\n\
5. Report limitations and unresolved items explicitly; do not infer completion from intent alone.\n\
6. Submit the result through super_instruct_stage_complete; call it alone after ordinary tool outputs are resolved. Expected next stage: {next}.\n\
Do not perform work belonging to another stage. The next request will receive this stage's evidence as input.",
            index = self.index,
            total = self.total,
            category = domain_label(category),
            stage = self.kind,
            profile = profile,
            action = self.action,
            objective = self.objective,
            method = self.method,
            evidence = self.required_evidence.join(", "),
            output = self.output_schema.join(", "),
        )
    }
}

fn domain_label(category: &Category) -> &'static str {
    match category {
        Category::Pentest => "security assessment",
        Category::Crack => "software authorization analysis",
        Category::Reverse => "software and binary analysis",
        Category::MalwareAnalysis => "malware analysis",
        Category::General => "general research",
    }
}

pub fn plan(actions: &ActionState, category: &Category) -> Vec<StageSpec> {
    let all_actions: Vec<String> = actions
        .pending
        .iter()
        .chain(actions.completed.iter())
        .cloned()
        .collect();
    let mut kinds = vec![
        StageKind::Framing,
        StageKind::Planning,
        StageKind::Evidence,
        StageKind::Analysis,
    ];
    if all_actions
        .iter()
        .any(|action| stage_for_action(action) == StageKind::Transformation)
    {
        kinds.push(StageKind::Transformation);
    }
    if all_actions
        .iter()
        .any(|action| stage_for_action(action) == StageKind::Execution)
    {
        kinds.push(StageKind::Execution);
    }
    if all_actions.iter().any(|action| {
        matches!(
            stage_for_action(action),
            StageKind::Transformation | StageKind::Execution | StageKind::Verification
        )
    }) {
        kinds.push(StageKind::Verification);
    }
    kinds.push(StageKind::Reporting);

    kinds
        .into_iter()
        .map(|kind| {
            let stage_actions: Vec<String> = all_actions
                .iter()
                .filter(|action| stage_for_action(action) == kind)
                .cloned()
                .collect();
            let action = if stage_actions.is_empty() {
                kind.as_str().to_string()
            } else {
                stage_actions.join(",")
            };
            let mut spec = spec_for(kind, &action, category);
            spec.actions = stage_actions;
            spec
        })
        .collect()
}

pub fn select(headers: &HeaderMap, body: &serde_json::Value, specs: &[StageSpec]) -> StageContext {
    if specs.is_empty() {
        let mut context = spec_context(
            &spec_for(StageKind::Planning, "plan", &Category::General),
            1,
            1,
            None,
        );
        context.action = "planning".to_string();
        context.actions.clear();
        return context;
    }
    let requested = headers
        .get("x-super-instruct-stage")
        .and_then(|value| value.to_str().ok())
        .and_then(StageKind::from_str)
        .or_else(|| {
            body.get("super_instruct")
                .and_then(|value| value.get("stage"))
                .and_then(|value| value.as_str())
                .and_then(StageKind::from_str)
        });

    let index = requested
        .and_then(|kind| specs.iter().position(|spec| spec.kind == kind))
        .unwrap_or(0);
    let spec = &specs[index];
    spec_context(
        spec,
        index + 1,
        specs.len(),
        specs.get(index + 1).map(|next| next.kind),
    )
}

fn spec_context(
    spec: &StageSpec,
    index: usize,
    total: usize,
    next_stage: Option<StageKind>,
) -> StageContext {
    StageContext {
        kind: spec.kind,
        index,
        total,
        action: spec.action.clone(),
        actions: spec.actions.clone(),
        objective: spec.objective.clone(),
        method: spec.method.clone(),
        required_evidence: spec.required_evidence.clone(),
        output_schema: spec.output_schema.clone(),
        next_stage,
    }
}

fn stage_for_action(action: &str) -> StageKind {
    match action {
        "acquire" => StageKind::Evidence,
        "inspect" => StageKind::Analysis,
        "transform" => StageKind::Transformation,
        "execute" => StageKind::Execution,
        "verify" => StageKind::Verification,
        "package" | "rollback" | "report" => StageKind::Reporting,
        _ => StageKind::Planning,
    }
}

fn spec_for(kind: StageKind, action: &str, category: &Category) -> StageSpec {
    let (objective, method, evidence, output) = match kind {
        StageKind::Framing => (
            "Define the research question, object boundary, and measurable success criteria.",
            "Normalize the request into a compact task contract.",
            vec!["request text"],
            vec!["scope", "research question", "acceptance criteria"],
        ),
        StageKind::Planning => (
            "Construct a minimal sequence of independently verifiable actions.",
            "Map each action to a method, artifact, and verification signal.",
            vec!["task contract"],
            vec!["stage sequence", "dependencies", "stop conditions"],
        ),
        StageKind::Evidence => (
            "Collect primary observations from the supplied object or fixture.",
            "Use bounded acquisition or inspection tools and preserve raw outputs.",
            vec![
                "artifact reference",
                "command transcript",
                "hash or identifier",
            ],
            vec!["observations", "raw evidence paths", "collection limits"],
        ),
        StageKind::Analysis => (
            "Explain structure and behavior using the available observations.",
            "Correlate symbols, fields, responses, or traces with explicit confidence levels.",
            vec!["observed facts", "source or tool references"],
            vec!["findings", "hypotheses", "confidence", "open questions"],
        ),
        StageKind::Transformation => (
            "Specify or produce the requested project artifact within the declared contract.",
            "Apply the smallest scoped change and record a reproducible diff.",
            vec!["baseline artifact", "change rationale", "diff"],
            vec!["artifact path", "diff path", "change summary"],
        ),
        StageKind::Execution => (
            "Execute the approved transformation or bounded research action.",
            "Use client-provided tools, preserve literal inputs and outputs, and stop on unexpected state.",
            vec!["approved action", "tool transcript", "exit status"],
            vec!["execution result", "tool evidence", "artifacts", "unresolved items"],
        ),
        StageKind::Verification => (
            "Evaluate the artifact or hypothesis against explicit acceptance criteria.",
            "Run bounded tests and report literal results, exit status, and residual risk.",
            vec!["test command", "input", "stdout or stderr", "exit status"],
            vec!["verification result", "evidence", "residual risk"],
        ),
        StageKind::Reporting => (
            "Synthesize the completed stages into an auditable research record.",
            "Reference only completed evidence and preserve pending items as unresolved.",
            vec!["stage records", "artifact paths", "verification results"],
            vec![
                "executive summary",
                "method",
                "findings",
                "limitations",
                "next steps",
            ],
        ),
    };
    StageSpec {
        kind,
        actions: vec![action.to_string()],
        action: action.to_string(),
        objective: format!("{objective} Domain: {category}."),
        method: method.to_string(),
        required_evidence: evidence.into_iter().map(str::to_string).collect(),
        output_schema: output.into_iter().map(str::to_string).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn actions() -> ActionState {
        let pending = vec![
            "acquire".into(),
            "inspect".into(),
            "verify".into(),
            "report".into(),
        ];
        ActionState {
            completed: Vec::new(),
            dag: crate::core::dag::plan(&pending),
            pending,
        }
    }

    #[test]
    fn creates_one_stage_per_methodological_phase() {
        let specs = plan(&actions(), &Category::Pentest);
        assert_eq!(
            specs.iter().map(|stage| stage.kind).collect::<Vec<_>>(),
            vec![
                StageKind::Framing,
                StageKind::Planning,
                StageKind::Evidence,
                StageKind::Analysis,
                StageKind::Verification,
                StageKind::Reporting
            ]
        );
    }

    #[test]
    fn header_selects_an_independent_stage() {
        let specs = plan(&actions(), &Category::Pentest);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-super-instruct-stage",
            HeaderValue::from_static("verification"),
        );
        let selected = select(&headers, &serde_json::json!({}), &specs);
        assert_eq!(selected.kind, StageKind::Verification);
        assert_eq!(selected.index, 5);
        assert_eq!(selected.next_stage, Some(StageKind::Reporting));
    }

    #[test]
    fn defaults_to_first_stage_for_legacy_clients() {
        let specs = plan(&actions(), &Category::Pentest);
        let selected = select(&HeaderMap::new(), &serde_json::json!({}), &specs);
        assert_eq!(selected.kind, StageKind::Framing);
    }

    #[test]
    fn empty_action_graph_gets_a_non_action_planning_context() {
        let empty = ActionState {
            completed: Vec::new(),
            pending: Vec::new(),
            dag: crate::core::dag::plan(&[]),
        };
        let specs = plan(&empty, &Category::General);
        let selected = select(&HeaderMap::new(), &serde_json::json!({}), &specs);
        assert_eq!(selected.kind, StageKind::Framing);
        assert!(selected.actions.is_empty());
    }
}

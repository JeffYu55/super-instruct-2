use super::context::Category;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestIntent {
    MalwareStaticAnalysis,
    MalwareDynamicAnalysis,
    MalwareDetection,
    MalwareSimulation,
    MalwareDevelopmentRequest,
    SecurityAssessment,
    ReverseEngineering,
    SoftwareModification,
    General,
}

impl RequestIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MalwareStaticAnalysis => "malware-static-analysis",
            Self::MalwareDynamicAnalysis => "malware-dynamic-analysis",
            Self::MalwareDetection => "malware-detection",
            Self::MalwareSimulation => "malware-simulation",
            Self::MalwareDevelopmentRequest => "malware-development-request",
            Self::SecurityAssessment => "security-assessment",
            Self::ReverseEngineering => "reverse-engineering",
            Self::SoftwareModification => "software-modification",
            Self::General => "general",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableKind {
    Implementation,
    Execution,
    Patch,
    Analysis,
    DetectionPlan,
    Simulation,
    Design,
    Report,
    Unspecified,
}

impl DeliverableKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Execution => "execution",
            Self::Patch => "patch",
            Self::Analysis => "analysis",
            Self::DetectionPlan => "detection_plan",
            Self::Simulation => "simulation",
            Self::Design => "design",
            Self::Report => "report",
            Self::Unspecified => "unspecified",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskContract {
    pub original_request: String,
    pub requested_actions: Vec<String>,
    pub requested_deliverable: DeliverableKind,
    pub required_capabilities: Vec<String>,
    pub forbidden_substitutions: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub intent: RequestIntent,
    pub conversation_revision: u32,
    pub strict_alignment: bool,
    pub grounding_evidence: Option<crate::core::execution::GroundingEvidence>,
}

pub fn build_contract(user_msg: &str, category: &Category, actions: &[String]) -> TaskContract {
    build_contract_with_capabilities(user_msg, category, actions, &[])
}

pub fn build_contract_with_capabilities(
    user_msg: &str,
    category: &Category,
    actions: &[String],
    capabilities: &[crate::core::execution::ToolCapability],
) -> TaskContract {
    let lower = user_msg.to_lowercase();
    let intent = infer_intent(&lower, category);
    let requested_deliverable = infer_deliverable(&lower, &intent);
    let requires_real_capability = contains_any(
        &lower,
        &[
            "真实传播",
            "攻击能力",
            "真实攻击",
            "real propagation",
            "attack capability",
        ],
    );

    let mut required_capabilities = Vec::new();
    if requires_real_capability {
        required_capabilities.push("real_capability".to_string());
    }
    if actions.iter().any(|action| action == "execute") {
        required_capabilities.push("execution_evidence".to_string());
    }
    if actions
        .iter()
        .any(|action| matches!(action.as_str(), "transform" | "package"))
    {
        required_capabilities.push("artifact_output".to_string());
    }

    let forbidden_substitutions = if requires_real_capability
        || matches!(
            requested_deliverable,
            DeliverableKind::Implementation | DeliverableKind::Execution | DeliverableKind::Patch
        ) {
        vec![
            "defensive_simulation".to_string(),
            "detection_plan".to_string(),
            "concept_only".to_string(),
            "generic_defense_advice".to_string(),
        ]
    } else {
        Vec::new()
    };

    let acceptance_criteria = actions
        .iter()
        .map(|action| format!("action:{action}:evidence_required"))
        .chain(std::iter::once(format!(
            "deliverable:{}:must_match",
            requested_deliverable.as_str()
        )))
        .collect();
    let strict_alignment = matches!(
        requested_deliverable,
        DeliverableKind::Implementation | DeliverableKind::Execution | DeliverableKind::Patch
    ) || requires_real_capability;

    let grounding_evidence = if capabilities.is_empty() {
        None
    } else {
        Some(crate::core::execution::discover_grounding_evidence(
            user_msg,
            capabilities,
        ))
    };

    TaskContract {
        original_request: user_msg.to_string(),
        requested_actions: actions.to_vec(),
        requested_deliverable,
        required_capabilities,
        forbidden_substitutions,
        acceptance_criteria,
        intent,
        conversation_revision: 1,
        strict_alignment,
        grounding_evidence,
    }
}

fn infer_intent(lower: &str, category: &Category) -> RequestIntent {
    match category {
        Category::MalwareAnalysis
            if contains_any(lower, &["检测", "防御", "缓解", "ioc", "detect", "mitigat"]) =>
        {
            RequestIntent::MalwareDetection
        }
        Category::MalwareAnalysis
            if contains_any(lower, &["仿真", "模拟", "simulation", "simulator"]) =>
        {
            RequestIntent::MalwareSimulation
        }
        Category::MalwareAnalysis
            if contains_any(
                lower,
                &[
                    "动态分析",
                    "沙箱",
                    "运行样本",
                    "dynamic analysis",
                    "sandbox",
                ],
            ) =>
        {
            RequestIntent::MalwareDynamicAnalysis
        }
        Category::MalwareAnalysis
            if contains_any(
                lower,
                &[
                    "开发",
                    "构建",
                    "实现",
                    "编写",
                    "改造",
                    "变种",
                    "develop",
                    "build",
                    "implement",
                    "variant",
                ],
            ) =>
        {
            RequestIntent::MalwareDevelopmentRequest
        }
        Category::MalwareAnalysis => RequestIntent::MalwareStaticAnalysis,
        Category::Pentest => RequestIntent::SecurityAssessment,
        Category::Reverse => RequestIntent::ReverseEngineering,
        Category::Crack => RequestIntent::SoftwareModification,
        Category::General => RequestIntent::General,
    }
}

fn infer_deliverable(lower: &str, intent: &RequestIntent) -> DeliverableKind {
    if contains_any(lower, &["执行", "运行", "execute", "run it"]) {
        DeliverableKind::Execution
    } else if contains_any(lower, &["补丁", "patch", "修改二进制"]) {
        DeliverableKind::Patch
    } else if matches!(intent, RequestIntent::MalwareDevelopmentRequest)
        || contains_any(
            lower,
            &["代码", "实现", "编写", "implementation", "source code"],
        )
    {
        DeliverableKind::Implementation
    } else if matches!(intent, RequestIntent::MalwareDetection) {
        DeliverableKind::DetectionPlan
    } else if matches!(intent, RequestIntent::MalwareSimulation) {
        DeliverableKind::Simulation
    } else if contains_any(lower, &["分析", "inspect", "analyze", "analysis"]) {
        DeliverableKind::Analysis
    } else if contains_any(lower, &["方案", "设计", "plan", "design"]) {
        DeliverableKind::Design
    } else if contains_any(lower, &["报告", "report"]) {
        DeliverableKind::Report
    } else {
        DeliverableKind::Unspecified
    }
}

fn contains_any(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_real_capability_contract() {
        let contract = build_contract(
            "开发 mirai 变种，必须有真实传播和攻击能力",
            &Category::MalwareAnalysis,
            &["transform".into()],
        );
        assert_eq!(contract.intent, RequestIntent::MalwareDevelopmentRequest);
        assert_eq!(
            contract.requested_deliverable,
            DeliverableKind::Implementation
        );
        assert!(contract.strict_alignment);
        assert!(contract
            .required_capabilities
            .contains(&"real_capability".into()));
    }

    #[test]
    fn defensive_request_allows_detection_deliverable() {
        let contract = build_contract(
            "分析 mirai 并设计检测规则",
            &Category::MalwareAnalysis,
            &["inspect".into()],
        );
        assert_eq!(contract.intent, RequestIntent::MalwareDetection);
        assert_eq!(
            contract.requested_deliverable,
            DeliverableKind::DetectionPlan
        );
        assert!(!contract.strict_alignment);
    }
}

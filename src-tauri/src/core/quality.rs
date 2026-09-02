use super::{ResponseCtx, UpstreamOutcome};
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct QualityAssessment {
    pub status: &'static str,
    pub passed: bool,
    pub score: u8,
    pub evidence_coverage: f32,
    pub action_coverage: f32,
    pub unresolved_actions: Vec<String>,
    pub verification_issues: Vec<String>,
}

pub fn assess(ctx: &ResponseCtx) -> QualityAssessment {
    // A staged request is evaluated against its selected action. The complete
    // workflow remains visible in the DAG and is evaluated when later stages
    // arrive, preventing an early evidence response from failing on future work.
    assess_reply(
        &ctx.outcome,
        ctx.status,
        &ctx.parsed.reply,
        &ctx.meta.stage.actions,
    )
}

pub fn assess_reply(
    outcome: &UpstreamOutcome,
    http_status: u16,
    reply: &str,
    actions: &[String],
) -> QualityAssessment {
    let successful =
        matches!(outcome, UpstreamOutcome::ModelSuccess) && (200..300).contains(&http_status);
    let substantive = reply.trim().chars().count() >= 50;
    let lower = reply.to_lowercase();
    let signal_groups: &[&[&str]] = &[
        &["sha256", "sha-256", "hash", "哈希"],
        &["exit code", "exit_code", "退出码"],
        &["path:", "file:", "/users/", "路径"],
        &["command:", "命令", "cargo test", "pytest"],
        &["jsonl", "log:", "日志", "evidence", "证据"],
        &["```", "fn ", "def ", "struct ", "class ", "function "],
    ];
    let signal_count = signal_groups
        .iter()
        .filter(|group| group.iter().any(|marker| lower.contains(marker)))
        .count();
    let evidence_coverage = (signal_count as f32 / 3.0).min(1.0);

    let unresolved_actions: Vec<String> = actions
        .iter()
        .filter(|action| !action_is_covered(action, &lower))
        .cloned()
        .collect();
    let action_coverage = if actions.is_empty() {
        1.0
    } else {
        (actions.len() - unresolved_actions.len()) as f32 / actions.len() as f32
    };
    let verification_issues = verification_issues(actions, &lower);

    let mut score = 0.0;
    if successful {
        score += 35.0;
    }
    if substantive {
        score += 15.0;
    }
    score += evidence_coverage * 30.0;
    score += action_coverage * 20.0;
    score -= verification_issues.len() as f32 * 10.0;
    let score = score.round().clamp(0.0, 100.0) as u8;
    let passed = successful
        && substantive
        && score >= 70
        && (actions.is_empty() || unresolved_actions.is_empty())
        && verification_issues.is_empty();
    let status = if passed {
        "passed"
    } else if successful && substantive {
        "partial"
    } else {
        "failed"
    };

    QualityAssessment {
        status,
        passed,
        score,
        evidence_coverage,
        action_coverage,
        unresolved_actions,
        verification_issues,
    }
}

fn verification_issues(actions: &[String], reply: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let requests_artifact = actions
        .iter()
        .any(|action| matches!(action.as_str(), "transform" | "package"));
    let lower_reply = reply.to_lowercase();
    let has_artifact_signal = reply.contains("```")
        || [
            "path:",
            "file:",
            "/users/",
            "/tmp/",
            "产物",
            "文件：",
            "文件:",
            "code:",
            "代码：",
            "代码:",
        ]
        .iter()
        .any(|marker| lower_reply.contains(marker))
        || [
            "fn ",
            "def ",
            "struct ",
            "class ",
            "function ",
            "#include",
            "import ",
            "package ",
            "pub fn ",
            "impl ",
            "void ",
            "int main",
            "export ",
        ]
        .iter()
        .any(|kw| lower_reply.contains(kw));
    if requests_artifact && !has_artifact_signal {
        issues.push("artifact_missing".to_string());
    }

    let requests_execution = actions
        .iter()
        .any(|action| matches!(action.as_str(), "execute" | "verify"))
        && !has_artifact_signal;
    let has_command = ["command:", "命令", "cargo test", "pytest", "执行："]
        .iter()
        .any(|marker| reply.contains(marker));
    let has_exit = ["exit code", "exit_code", "退出码"]
        .iter()
        .any(|marker| reply.contains(marker));
    if requests_execution && !(has_command && has_exit) {
        issues.push("tool_claim_unverified".to_string());
    }

    // 检查模型是否用泛泛的安全建议替代了请求的代码/逆向实现 (Task Divergence)
    let lower = reply.to_lowercase();
    let generic_advice_markers = [
        "建议加强防火墙",
        "建议安装杀毒软件",
        "建议升级系统",
        "legal alternative",
        "safe alternative",
        "instead of providing",
        "safety advice",
        "educational purpose only",
        "无法提供具体实现，但可以提供防御建议",
    ];
    if (requests_artifact || requests_execution)
        && generic_advice_markers.iter().any(|m| lower.contains(m))
    {
        issues.push("defensive_advice_substitution".to_string());
    }

    issues
}

pub fn result_status(ctx: &ResponseCtx, quality: &QualityAssessment) -> &'static str {
    match ctx.outcome {
        UpstreamOutcome::TaskDivergence => "TASK_DIVERGENCE",
        UpstreamOutcome::ModelRefusal => "EXPLICIT_REFUSAL",
        UpstreamOutcome::ProviderPolicyBlock => "POLICY_BLOCKED",
        UpstreamOutcome::TransportError => "TRANSPORT_ERROR",
        UpstreamOutcome::ProtocolError => "PROTOCOL_ERROR",
        UpstreamOutcome::Cancelled => "CANCELLED",
        UpstreamOutcome::ModelSuccess if quality.passed => "SUCCEEDED",
        UpstreamOutcome::ModelSuccess
            if quality
                .verification_issues
                .iter()
                .any(|issue| issue == "artifact_missing") =>
        {
            "ARTIFACT_MISSING"
        }
        UpstreamOutcome::ModelSuccess
            if quality
                .verification_issues
                .iter()
                .any(|issue| issue == "tool_claim_unverified") =>
        {
            "TOOL_CLAIM_UNVERIFIED"
        }
        UpstreamOutcome::ModelSuccess if quality.evidence_coverage == 0.0 => "EVIDENCE_MISSING",
        UpstreamOutcome::ModelSuccess => "PARTIAL",
    }
}

fn action_is_covered(action: &str, reply: &str) -> bool {
    let markers: &[&str] = match action {
        "acquire" => &["acquire", "download", "获取", "下载"],
        "inspect" => &["inspect", "analysis", "analyze", "分析", "检查"],
        "transform" => &[
            "transform",
            "modify",
            "modified",
            "develop",
            "build",
            "implement",
            "design",
            "rewrite",
            "修改",
            "改造",
            "开发",
            "构建",
            "实现",
            "编写",
            "设计",
            "代码",
            "函数",
            "```",
        ],
        "execute" => &["execute", "executed", "run", "运行", "执行"],
        "verify" => &[
            "verify", "verified", "test", "passed", "验证", "测试", "通过",
        ],
        "package" => &["package", "artifact", "bundle", "打包", "产物"],
        "rollback" => &["rollback", "restore", "revert", "回滚", "恢复"],
        "report" => &["report", "summary", "finding", "报告", "结论", "发现"],
        other => return reply.contains(other),
    };
    markers.iter().any(|marker| reply.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_verified_evidence_response() {
        let reply = "分析报告：inspect analysis 已完成，report 结论已生成。证据 path: /tmp/report.json，SHA256: abc，命令 cargo test，exit code 0。";
        let quality = assess_reply(
            &UpstreamOutcome::ModelSuccess,
            200,
            reply,
            &["inspect".into(), "report".into()],
        );
        assert!(quality.passed);
        assert_eq!(quality.status, "passed");
        assert_eq!(quality.unresolved_actions, Vec::<String>::new());
        assert!(quality.verification_issues.is_empty());
    }

    #[test]
    fn rejects_model_refusal() {
        let quality = assess_reply(
            &UpstreamOutcome::ModelRefusal,
            200,
            "A long refusal response with path: /tmp/fake and exit code 0.",
            &[],
        );
        assert!(!quality.passed);
        assert_eq!(quality.status, "failed");
    }
}

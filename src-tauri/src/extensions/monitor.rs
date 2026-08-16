use crate::core::quality;
use crate::core::{Category, ResponseCtx, ResponseInterceptor, UpstreamOutcome};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub struct FileMonitor {
    file: Mutex<std::fs::File>,
    response_dir: PathBuf,
    matrix_file: PathBuf,
    matrix: Mutex<HashMap<String, CapabilityRecord>>,
    stats: AtomicStats,
}

#[derive(Default)]
struct AtomicStats {
    total: AtomicU64,
    crack: AtomicU64,
    reverse: AtomicU64,
    pentest: AtomicU64,
    malware_analysis: AtomicU64,
    general: AtomicU64,
    succeeded: AtomicU64,
    diverged: AtomicU64,
    refused: AtomicU64,
    policy_blocked: AtomicU64,
    transport_errors: AtomicU64,
    protocol_errors: AtomicU64,
    cancelled: AtomicU64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatsSnapshot {
    pub total: u64,
    pub crack: u64,
    pub reverse: u64,
    pub pentest: u64,
    pub malware_analysis: u64,
    pub general: u64,
    pub succeeded: u64,
    pub diverged: u64,
    pub refused: u64,
    pub policy_blocked: u64,
    pub transport_errors: u64,
    pub protocol_errors: u64,
    pub cancelled: u64,
}

#[derive(Serialize)]
struct InteractionEvent<'a> {
    request_id: &'a str,
    model: Option<&'a str>,
    timestamp: String,
    category: String,
    profile: &'a str,
    intent: &'a str,
    requested_deliverable: &'a str,
    strict_alignment: bool,
    skills: &'a [String],
    pending_actions: &'a [String],
    confidence: f32,
    execution_mode: String,
    injected: bool,
    status: u16,
    response_bytes: usize,
    duration_ms: u64,
    dag_version: u32,
    dag_nodes: usize,
    dag_pending: usize,
    evidence_status: &'static str,
    outcome: &'static str,
    refusal_reason: Option<&'static str>,
    task_alignment: &'static str,
    quality_status: &'static str,
    quality_score: u8,
    result_status: &'static str,
    evidence_coverage: f32,
    action_coverage: f32,
    unresolved_actions: Vec<String>,
    verification_issues: Vec<String>,
    raw_response_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct CapabilityRecord {
    total: u64,
    succeeded: u64,
    diverged: u64,
    refused: u64,
    policy_blocked: u64,
    transport_errors: u64,
    protocol_errors: u64,
    cancelled: u64,
}

impl FileMonitor {
    pub fn new(log_dir: impl Into<PathBuf>) -> Result<Self, String> {
        let log_dir = log_dir.into();
        create_dir_all(&log_dir).map_err(|e| format!("create monitor dir failed: {e}"))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("interactions.jsonl"))
            .map_err(|e| format!("open monitor log failed: {e}"))?;
        let response_dir = log_dir.join("responses");
        create_dir_all(&response_dir)
            .map_err(|e| format!("create response audit dir failed: {e}"))?;
        let matrix_file = log_dir.join("capability-matrix.json");
        let matrix = std::fs::read_to_string(&matrix_file)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default();
        Ok(Self {
            file: Mutex::new(file),
            response_dir,
            matrix_file,
            matrix: Mutex::new(matrix),
            stats: AtomicStats::default(),
        })
    }

    pub fn stats(&self) -> StatsSnapshot {
        StatsSnapshot {
            total: self.stats.total.load(Ordering::Relaxed),
            crack: self.stats.crack.load(Ordering::Relaxed),
            reverse: self.stats.reverse.load(Ordering::Relaxed),
            pentest: self.stats.pentest.load(Ordering::Relaxed),
            malware_analysis: self.stats.malware_analysis.load(Ordering::Relaxed),
            general: self.stats.general.load(Ordering::Relaxed),
            succeeded: self.stats.succeeded.load(Ordering::Relaxed),
            diverged: self.stats.diverged.load(Ordering::Relaxed),
            refused: self.stats.refused.load(Ordering::Relaxed),
            policy_blocked: self.stats.policy_blocked.load(Ordering::Relaxed),
            transport_errors: self.stats.transport_errors.load(Ordering::Relaxed),
            protocol_errors: self.stats.protocol_errors.load(Ordering::Relaxed),
            cancelled: self.stats.cancelled.load(Ordering::Relaxed),
        }
    }
}

impl ResponseInterceptor for FileMonitor {
    fn name(&self) -> &'static str {
        "monitor"
    }

    fn intercept(&self, ctx: &mut ResponseCtx) {
        if !ctx.meta.route.monitor {
            return;
        }
        self.stats.total.fetch_add(1, Ordering::Relaxed);
        match ctx.meta.category {
            Category::Crack => {
                self.stats.crack.fetch_add(1, Ordering::Relaxed);
            }
            Category::Reverse => {
                self.stats.reverse.fetch_add(1, Ordering::Relaxed);
            }
            Category::Pentest => {
                self.stats.pentest.fetch_add(1, Ordering::Relaxed);
            }
            Category::MalwareAnalysis => {
                self.stats.malware_analysis.fetch_add(1, Ordering::Relaxed);
            }
            Category::General => {
                self.stats.general.fetch_add(1, Ordering::Relaxed);
            }
        }
        match ctx.outcome {
            UpstreamOutcome::ModelSuccess => {
                self.stats.succeeded.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::TaskDivergence => {
                self.stats.diverged.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::ModelRefusal => {
                self.stats.refused.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::ProviderPolicyBlock => {
                self.stats.policy_blocked.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::TransportError => {
                self.stats.transport_errors.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::ProtocolError => {
                self.stats.protocol_errors.fetch_add(1, Ordering::Relaxed);
            }
            UpstreamOutcome::Cancelled => {
                self.stats.cancelled.fetch_add(1, Ordering::Relaxed);
            }
        }
        let quality = quality::assess(ctx);
        let raw_response_path = preserve_response(&self.response_dir, ctx, &quality);
        self.update_capability_matrix(ctx);
        let event = InteractionEvent {
            request_id: &ctx.meta.request_id,
            model: ctx.meta.model.as_deref(),
            timestamp: ctx.meta.timestamp.to_rfc3339(),
            category: ctx.meta.category.to_string(),
            profile: &ctx.meta.route.profile,
            intent: ctx.meta.contract.intent.as_str(),
            requested_deliverable: ctx.meta.contract.requested_deliverable.as_str(),
            strict_alignment: ctx.meta.contract.strict_alignment,
            skills: &ctx.meta.route.skills,
            pending_actions: &ctx.meta.actions.pending,
            confidence: ctx.meta.route.confidence,
            execution_mode: ctx.meta.execution_mode.to_string(),
            injected: ctx.meta.route.inject_request,
            status: ctx.status,
            response_bytes: ctx.raw_body.len(),
            duration_ms: ctx.duration_ms,
            dag_version: ctx.meta.actions.dag.version,
            dag_nodes: ctx.meta.actions.dag.nodes.len(),
            dag_pending: ctx.meta.actions.dag.pending_count(),
            evidence_status: evidence_status(ctx),
            outcome: outcome_name(&ctx.outcome),
            refusal_reason: refusal_reason(ctx),
            task_alignment: task_alignment(ctx),
            quality_status: quality.status,
            quality_score: quality.score,
            result_status: quality::result_status(ctx, &quality),
            evidence_coverage: quality.evidence_coverage,
            action_coverage: quality.action_coverage,
            unresolved_actions: quality.unresolved_actions,
            verification_issues: quality.verification_issues,
            raw_response_path,
        };
        if let Ok(line) = serde_json::to_string(&event) {
            if let Ok(mut file) = self.file.lock() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        }
    }
}

impl FileMonitor {
    fn update_capability_matrix(&self, ctx: &ResponseCtx) {
        let key = format!(
            "{}|{}|{}",
            ctx.meta.model.as_deref().unwrap_or("unknown-model"),
            ctx.meta.contract.intent.as_str(),
            ctx.meta.contract.requested_deliverable.as_str()
        );
        let mut matrix = self.matrix.lock().unwrap();
        let record = matrix.entry(key).or_default();
        record.total += 1;
        match ctx.outcome {
            UpstreamOutcome::ModelSuccess => record.succeeded += 1,
            UpstreamOutcome::TaskDivergence => record.diverged += 1,
            UpstreamOutcome::ModelRefusal => record.refused += 1,
            UpstreamOutcome::ProviderPolicyBlock => record.policy_blocked += 1,
            UpstreamOutcome::TransportError => record.transport_errors += 1,
            UpstreamOutcome::ProtocolError => record.protocol_errors += 1,
            UpstreamOutcome::Cancelled => record.cancelled += 1,
        }
        if let Ok(content) = serde_json::to_vec_pretty(&*matrix) {
            let temporary = self.matrix_file.with_extension("json.tmp");
            if std::fs::write(&temporary, content).is_ok() {
                let _ = std::fs::rename(temporary, &self.matrix_file);
            }
        }
    }
}

fn preserve_response(
    response_dir: &Path,
    ctx: &ResponseCtx,
    quality: &quality::QualityAssessment,
) -> Option<String> {
    if matches!(ctx.outcome, UpstreamOutcome::ModelSuccess)
        && (!ctx.meta.contract.strict_alignment || quality.passed)
    {
        return None;
    }
    let safe_id: String = ctx
        .meta
        .request_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect();
    let path = response_dir.join(format!("{safe_id}.raw"));
    std::fs::write(&path, &ctx.raw_body).ok()?;
    Some(path.to_string_lossy().to_string())
}

fn refusal_reason(ctx: &ResponseCtx) -> Option<&'static str> {
    if matches!(ctx.outcome, UpstreamOutcome::TaskDivergence) {
        return Some("defensive_redirect");
    }
    if !matches!(ctx.outcome, UpstreamOutcome::ModelRefusal) {
        return None;
    }
    let reply = ctx.parsed.reply.to_lowercase();
    if [
        "真实传播",
        "远程控制",
        "持久化",
        "ddos",
        "botnet",
        "不予协助",
    ]
    .iter()
    .any(|marker| reply.contains(marker))
    {
        return Some("operational_malware_capability");
    }
    if reply.contains("授权") || reply.contains("authorization") {
        return Some("authorization_boundary");
    }
    Some("unspecified_model_refusal")
}

fn task_alignment(ctx: &ResponseCtx) -> &'static str {
    match ctx.outcome {
        UpstreamOutcome::TaskDivergence => "diverged",
        UpstreamOutcome::ModelSuccess => "aligned",
        _ => "unverified",
    }
}

fn evidence_status(ctx: &ResponseCtx) -> &'static str {
    if matches!(ctx.outcome, UpstreamOutcome::ProviderPolicyBlock) {
        return "provider_policy_block";
    }
    if matches!(ctx.outcome, UpstreamOutcome::TransportError) {
        return "transport_or_provider_error";
    }
    if matches!(ctx.outcome, UpstreamOutcome::ProtocolError) {
        return "protocol_error";
    }
    if matches!(ctx.outcome, UpstreamOutcome::Cancelled) {
        return "cancelled";
    }
    if matches!(ctx.outcome, UpstreamOutcome::ModelRefusal) {
        return "model_refusal";
    }
    if matches!(ctx.outcome, UpstreamOutcome::TaskDivergence) {
        return "task_divergence";
    }
    if ctx
        .meta
        .actions
        .dag
        .nodes
        .iter()
        .any(|node| node.status != "completed")
    {
        return "partial_or_unverified";
    }
    "upstream_verified"
}

fn outcome_name(outcome: &UpstreamOutcome) -> &'static str {
    match outcome {
        UpstreamOutcome::ModelSuccess => "model_success",
        UpstreamOutcome::ModelRefusal => "model_refusal",
        UpstreamOutcome::TaskDivergence => "task_divergence",
        UpstreamOutcome::ProviderPolicyBlock => "provider_policy_block",
        UpstreamOutcome::TransportError => "transport_error",
        UpstreamOutcome::ProtocolError => "protocol_error",
        UpstreamOutcome::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::{ActionState, ParsedResponse, RequestMeta, RoutePlan};
    use crate::core::ExecutionMode;
    use chrono::Utc;

    fn response_ctx(outcome: UpstreamOutcome, reply: &str) -> ResponseCtx {
        ResponseCtx {
            meta: RequestMeta {
                request_id: "req-test".into(),
                model: Some("test-model".into()),
                user_msg: "test".into(),
                category: Category::MalwareAnalysis,
                route: RoutePlan {
                    profile: "competition-malware-analysis".into(),
                    skills: vec![],
                    inject_request: true,
                    persist_memory: true,
                    monitor: true,
                    confidence: 0.9,
                },
                actions: ActionState {
                    completed: vec![],
                    pending: vec!["inspect".into()],
                    dag: crate::core::dag::plan(&["inspect".into()]),
                },
                contract: crate::core::build_contract(
                    "test",
                    &Category::MalwareAnalysis,
                    &["inspect".into()],
                ),
                execution_mode: ExecutionMode::Interleaved,
                path: "/responses".into(),
                timestamp: Utc::now(),
            },
            status: 200,
            raw_body: bytes::Bytes::new(),
            parsed: ParsedResponse {
                thinking: String::new(),
                reply: reply.into(),
            },
            duration_ms: 1,
            outcome,
        }
    }

    #[test]
    fn attributes_operational_malware_refusal() {
        let ctx = response_ctx(
            UpstreamOutcome::ModelRefusal,
            "真实传播、远程控制、持久化或 DDoS 能力会形成 botnet，这类实现不予协助。",
        );
        assert_eq!(refusal_reason(&ctx), Some("operational_malware_capability"));
    }

    #[test]
    fn writes_task_divergence_to_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "super-instruct-monitor-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let monitor = FileMonitor::new(&dir).expect("monitor fixture");
        let mut ctx = response_ctx(
            UpstreamOutcome::TaskDivergence,
            "这是非攻击性的威胁仿真与检测方案，只在本地运行。",
        );
        monitor.intercept(&mut ctx);

        let line =
            std::fs::read_to_string(dir.join("interactions.jsonl")).expect("interaction fixture");
        let event: serde_json::Value = serde_json::from_str(line.trim()).expect("valid jsonl");
        assert_eq!(event["outcome"], "task_divergence");
        assert_eq!(event["refusal_reason"], "defensive_redirect");
        assert_eq!(event["task_alignment"], "diverged");
        assert_eq!(event["result_status"], "TASK_DIVERGENCE");
        let raw_path = event["raw_response_path"]
            .as_str()
            .expect("raw response path");
        assert!(std::path::Path::new(raw_path).is_file());
        let matrix: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("capability-matrix.json"))
                .expect("capability matrix fixture"),
        )
        .expect("valid capability matrix");
        assert_eq!(
            matrix["test-model|malware-static-analysis|unspecified"]["diverged"],
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

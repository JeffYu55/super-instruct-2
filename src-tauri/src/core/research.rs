use crate::core::context::{ActionState, Category, ResearchRequestMeta, RoutePlan};
use crate::core::contract::{RequestIntent, TaskContract};
use crate::core::protocol::{ApiType, StageResultV1, ToolCallRecord, ToolOutputRecord};
use crate::core::stages::{self, StageContext, StageKind, StageSpec};
use chrono::{DateTime, Utc};
use http::HeaderMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

pub const CHECKPOINT_VERSION: u32 = 1;
const MAX_EVENTS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    WaitingTool,
    AwaitingApproval,
    Recoverable,
    Completed,
    Stopped,
    Failed,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::WaitingTool => "waiting_tool",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Recoverable => "recoverable",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchBudget {
    pub max_rounds: u32,
    pub timeout_secs: u64,
    pub no_evidence_limit: u32,
}

impl Default for ResearchBudget {
    fn default() -> Self {
        Self {
            max_rounds: 12,
            timeout_secs: 30 * 60,
            no_evidence_limit: 2,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceNode {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
    pub raw_output: Value,
    pub exit_code: Option<i32>,
    pub path: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EvidenceEdge {
    pub source_kind: String,
    pub source: String,
    pub evidence_ref: String,
    pub stage: StageKind,
    pub round: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct EvidenceGraph {
    pub nodes: BTreeMap<String, EvidenceNode>,
    #[serde(default)]
    pub edges: Vec<EvidenceEdge>,
}

impl EvidenceGraph {
    pub fn insert(
        &mut self,
        tool_name: impl Into<String>,
        arguments: impl Into<String>,
        raw_output: Value,
    ) -> (String, bool) {
        let tool_name = tool_name.into();
        let arguments = canonicalize_json_text(&arguments.into());
        let normalized = serde_json::json!({
            "tool_name": tool_name,
            "arguments": arguments,
            "output": raw_output
        });
        let sha256 = sha256_hex(normalized.to_string().as_bytes());
        let id = format!("ev_{}", &sha256[..20]);
        if self.nodes.contains_key(&id) {
            return (id, false);
        }
        let output_text = output_text(&normalized["output"]);
        let node = EvidenceNode {
            id: id.clone(),
            tool_name: normalized["tool_name"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            arguments: normalized["arguments"].as_str().unwrap_or("{}").to_string(),
            raw_output: normalized["output"].clone(),
            exit_code: extract_exit_code(&output_text),
            path: extract_path(&output_text),
            timestamp: Utc::now(),
            sha256,
        };
        self.nodes.insert(id.clone(), node);
        (id, true)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    pub fn link_stage_result(&mut self, stage: StageKind, round: u32, result: &StageResultV1) {
        for claim in &result.observations {
            self.link(
                "observation",
                &claim.statement,
                &claim.evidence_refs,
                stage,
                round,
            );
        }
        for claim in &result.inferences {
            self.link(
                "inference",
                &claim.statement,
                &claim.evidence_refs,
                stage,
                round,
            );
        }
        for hypothesis in &result.hypotheses {
            self.link(
                "hypothesis",
                hypothesis,
                &result.evidence_refs,
                stage,
                round,
            );
        }
        for artifact in &result.artifacts {
            self.link(
                "artifact",
                &artifact.path,
                &result.evidence_refs,
                stage,
                round,
            );
        }
    }

    fn link(
        &mut self,
        source_kind: &str,
        source: &str,
        evidence_refs: &[String],
        stage: StageKind,
        round: u32,
    ) {
        for evidence_ref in evidence_refs {
            if !self.contains(evidence_ref) {
                continue;
            }
            let edge = EvidenceEdge {
                source_kind: source_kind.to_string(),
                source: source.to_string(),
                evidence_ref: evidence_ref.clone(),
                stage,
                round,
            };
            if !self.edges.contains(&edge) {
                self.edges.push(edge);
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApprovalRecord {
    pub stage: StageKind,
    pub round: u32,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StageRecord {
    pub stage: StageKind,
    pub round: u32,
    pub status: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub artifacts: Vec<crate::core::protocol::ResearchArtifactV1>,
    pub quality_score: u32,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderCursor {
    pub api_type: ApiType,
    pub request: Value,
    pub last_response_id: Option<String>,
    pub response_aliases: Vec<String>,
    pub last_output_items: Vec<Value>,
    pub last_assistant_message: Option<Value>,
    pub last_tool_calls: Vec<ToolCallRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchEvent {
    pub sequence: u64,
    pub event: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub stage: String,
    pub round: u32,
    pub status: String,
    pub summary: String,
    pub evidence_total: usize,
    pub stop_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResearchSession {
    pub checkpoint_version: u32,
    pub id: String,
    pub fingerprint: String,
    pub aliases: Vec<String>,
    pub status: SessionStatus,
    pub contract: TaskContract,
    pub category: Category,
    pub route: RoutePlan,
    pub actions: ActionState,
    pub stages: Vec<StageSpec>,
    pub current_stage_index: usize,
    pub round: u32,
    pub evidence: EvidenceGraph,
    pub approvals: Vec<ApprovalRecord>,
    pub stage_records: Vec<StageRecord>,
    pub pending_tool_calls: BTreeMap<String, ToolCallRecord>,
    pub deferred_tool_outputs: Vec<ToolOutputRecord>,
    pub committed_calls: Vec<String>,
    pub provider: ProviderCursor,
    pub budget: ResearchBudget,
    pub no_evidence_rounds: u32,
    pub evidence_at_round_start: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stop_reason: Option<String>,
    pub last_summary: String,
    pub event_sequence: u64,
    pub events: VecDeque<ResearchEvent>,
}

impl ResearchSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        fingerprint: String,
        contract: TaskContract,
        category: Category,
        route: RoutePlan,
        actions: ActionState,
        initial_request: Value,
        api_type: ApiType,
        budget: ResearchBudget,
    ) -> Self {
        let stages = stages::plan(&actions, &category);
        let now = Utc::now();
        let mut evidence = EvidenceGraph::default();
        evidence.insert(
            "request",
            "{}",
            Value::String(contract.original_request.clone()),
        );
        evidence.insert(
            "task_contract",
            "{}",
            serde_json::to_value(&contract).unwrap_or(Value::Null),
        );
        Self {
            checkpoint_version: CHECKPOINT_VERSION,
            id,
            fingerprint,
            aliases: Vec::new(),
            status: SessionStatus::Active,
            contract,
            category,
            route,
            actions,
            stages,
            current_stage_index: 0,
            round: 0,
            evidence,
            approvals: Vec::new(),
            stage_records: Vec::new(),
            pending_tool_calls: BTreeMap::new(),
            deferred_tool_outputs: Vec::new(),
            committed_calls: Vec::new(),
            provider: ProviderCursor {
                api_type,
                request: initial_request,
                last_response_id: None,
                response_aliases: Vec::new(),
                last_output_items: Vec::new(),
                last_assistant_message: None,
                last_tool_calls: Vec::new(),
            },
            budget,
            no_evidence_rounds: 0,
            evidence_at_round_start: 2,
            created_at: now,
            updated_at: now,
            stop_reason: None,
            last_summary: String::new(),
            event_sequence: 0,
            events: VecDeque::new(),
        }
    }

    pub fn current_stage(&self) -> &StageSpec {
        &self.stages[self.current_stage_index]
    }

    pub fn stage_context(&self) -> StageContext {
        let stage = self.current_stage();
        StageContext {
            kind: stage.kind,
            index: self.current_stage_index + 1,
            total: self.stages.len(),
            action: stage.action.clone(),
            actions: stage.actions.clone(),
            objective: stage.objective.clone(),
            method: stage.method.clone(),
            required_evidence: stage.required_evidence.clone(),
            output_schema: stage.output_schema.clone(),
            next_stage: self
                .stages
                .get(self.current_stage_index + 1)
                .map(|stage| stage.kind),
        }
    }

    pub fn research_meta(&self) -> ResearchRequestMeta {
        ResearchRequestMeta {
            session_id: self.id.clone(),
            round: self.round,
            session_status: self.status.as_str().to_string(),
            evidence_refs: self.evidence.ids(),
            evidence_total: self.evidence.nodes.len(),
            stop_reason: self.stop_reason.clone(),
        }
    }

    pub fn start_round(&mut self) -> Result<(), String> {
        if self.status.terminal() {
            return Err(self
                .stop_reason
                .clone()
                .unwrap_or_else(|| "session is terminal".to_string()));
        }
        if self.round >= self.budget.max_rounds {
            self.stop("round_budget");
            return Err("round budget reached".to_string());
        }
        let elapsed = (Utc::now() - self.created_at).num_seconds().max(0) as u64;
        if elapsed >= self.budget.timeout_secs {
            self.stop("time_budget");
            return Err("time budget reached".to_string());
        }
        if stage_uses_evidence_budget(self.current_stage().kind)
            && self.no_evidence_rounds >= self.budget.no_evidence_limit
        {
            self.stop("no_new_evidence");
            return Err("no-new-evidence budget reached".to_string());
        }
        self.round += 1;
        self.evidence_at_round_start = self.evidence.nodes.len();
        self.status = SessionStatus::Active;
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn finish_round(&mut self) {
        if stage_uses_evidence_budget(self.current_stage().kind) {
            if self.evidence.nodes.len() == self.evidence_at_round_start {
                self.no_evidence_rounds = self.no_evidence_rounds.saturating_add(1);
            } else {
                self.no_evidence_rounds = 0;
            }
        }
        self.updated_at = Utc::now();
    }

    pub fn add_tool_outputs(&mut self, outputs: &[ToolOutputRecord]) -> Vec<String> {
        let mut added = Vec::new();
        for output in outputs {
            let call = self.pending_tool_calls.get(&output.call_id);
            let tool_name = call
                .map(|call| call.name.as_str())
                .unwrap_or("unknown_tool");
            let arguments = call.map(|call| call.arguments.as_str()).unwrap_or("{}");
            let (id, inserted) = self
                .evidence
                .insert(tool_name, arguments, output.output.clone());
            if inserted {
                added.push(id);
            }
            self.pending_tool_calls.remove(&output.call_id);
        }
        if self.pending_tool_calls.is_empty() && self.status == SessionStatus::WaitingTool {
            self.status = SessionStatus::Active;
        }
        if !added.is_empty() {
            self.no_evidence_rounds = 0;
        }
        self.updated_at = Utc::now();
        added
    }

    pub fn no_evidence_limit_reached(&self) -> bool {
        stage_uses_evidence_budget(self.current_stage().kind)
            && self.no_evidence_rounds >= self.budget.no_evidence_limit
    }

    pub fn validate_stage_result(
        &self,
        result: &StageResultV1,
        call_id: &str,
    ) -> Result<u32, Vec<String>> {
        let commit_key = self.commit_key(result.stage, self.round, call_id);
        if self.committed_calls.iter().any(|key| key == &commit_key) {
            return Ok(100);
        }
        let mut errors = Vec::new();
        let mut score = 0;
        if result.session_id == self.id {
            score += 10;
        } else {
            errors.push("session_id_mismatch".to_string());
        }
        if result.stage == self.current_stage().kind {
            score += 10;
        } else {
            errors.push("stage_mismatch".to_string());
        }
        if result.status == "completed" {
            score += 15;
        } else {
            errors.push("stage_status_not_completed".to_string());
        }
        if !result.summary.trim().is_empty() {
            score += 15;
        } else {
            errors.push("summary_missing".to_string());
        }
        let missing_refs: Vec<String> = result
            .evidence_refs
            .iter()
            .filter(|reference| !self.evidence.contains(reference))
            .cloned()
            .collect();
        if result.evidence_refs.is_empty() {
            errors.push("evidence_refs_missing".to_string());
        } else if missing_refs.is_empty() {
            score += 25;
        } else {
            errors.push(format!("unknown_evidence_refs:{}", missing_refs.join(",")));
        }
        let claims_valid = !result.observations.is_empty()
            && result
                .observations
                .iter()
                .chain(result.inferences.iter())
                .all(|claim| {
                    !claim.evidence_refs.is_empty()
                        && claim
                            .evidence_refs
                            .iter()
                            .all(|reference| self.evidence.contains(reference))
                });
        if claims_valid {
            score += 15;
        } else {
            errors.push("claim_evidence_missing".to_string());
        }
        if self.pending_tool_calls.is_empty() {
            score += 10;
        } else {
            errors.push("pending_tool_calls".to_string());
        }
        if self.current_stage().kind == StageKind::Transformation && result.artifacts.is_empty() {
            errors.push("transformation_artifact_missing".to_string());
        }
        let expected_next = self
            .stages
            .get(self.current_stage_index + 1)
            .map(|stage| stage.kind);
        if result.next_stage != expected_next {
            errors.push("next_stage_mismatch".to_string());
        }
        if score < 70 {
            errors.push(format!("quality_score_below_70:{score}"));
        }
        if errors.is_empty() {
            Ok(score)
        } else {
            Err(errors)
        }
    }

    pub fn commit_stage(&mut self, result: StageResultV1, call_id: &str, score: u32) {
        let commit_key = self.commit_key(result.stage, self.round, call_id);
        if self.committed_calls.iter().any(|key| key == &commit_key) {
            return;
        }
        self.committed_calls.push(commit_key);
        self.last_summary = result.summary.clone();
        self.evidence
            .link_stage_result(result.stage, self.round, &result);
        self.stage_records.push(StageRecord {
            stage: result.stage,
            round: self.round,
            status: result.status,
            summary: result.summary,
            evidence_refs: result.evidence_refs,
            artifacts: result.artifacts,
            quality_score: score,
            timestamp: Utc::now(),
        });
        for action in self.current_stage().actions.clone() {
            if let Some(position) = self
                .actions
                .pending
                .iter()
                .position(|candidate| candidate == &action)
            {
                self.actions.pending.remove(position);
                if !self.actions.completed.contains(&action) {
                    self.actions.completed.push(action.clone());
                }
            }
            for node in &mut self.actions.dag.nodes {
                if node.action == action {
                    node.status = "completed".to_string();
                }
            }
        }
        if self.current_stage_index + 1 >= self.stages.len() {
            self.status = SessionStatus::Completed;
        } else {
            self.current_stage_index += 1;
            self.status = if self.current_stage().kind.requires_approval() {
                SessionStatus::AwaitingApproval
            } else {
                SessionStatus::Active
            };
        }
        self.updated_at = Utc::now();
    }

    pub fn approve_current_stage(&mut self) -> Result<(), String> {
        if self.status != SessionStatus::AwaitingApproval {
            return Err("session is not awaiting approval".to_string());
        }
        self.approvals.push(ApprovalRecord {
            stage: self.current_stage().kind,
            round: self.round,
            timestamp: Utc::now(),
        });
        self.status = SessionStatus::Active;
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.stop_reason = Some(reason.into());
        self.status = SessionStatus::Stopped;
        self.updated_at = Utc::now();
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        self.stop_reason = Some(reason.into());
        self.status = SessionStatus::Failed;
        self.updated_at = Utc::now();
    }

    pub fn approval_message(&self) -> String {
        format!(
            "AWAITING_APPROVAL stage={} [RESEARCH_SESSION:{}]",
            self.current_stage().kind,
            self.id
        )
    }

    pub fn final_report(&self) -> String {
        let mut sections = self
            .stage_records
            .iter()
            .map(|record| format!("## {}\n{}", record.stage, record.summary))
            .collect::<Vec<_>>();
        if let Some(reason) = &self.stop_reason {
            sections.push(format!("## stop_reason\n{reason}"));
        }
        sections.push(format!("[RESEARCH_SESSION:{}]", self.id));
        sections.join("\n\n")
    }

    fn commit_key(&self, stage: StageKind, round: u32, call_id: &str) -> String {
        format!("{}:{}:{}:{}", self.id, stage, round, call_id)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub status: String,
    pub stage: String,
    pub round: u32,
    pub evidence_total: usize,
    pub stop_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ResearchSession> for SessionSummary {
    fn from(session: &ResearchSession) -> Self {
        Self {
            session_id: session.id.clone(),
            status: session.status.as_str().to_string(),
            stage: session.current_stage().kind.as_str().to_string(),
            round: session.round,
            evidence_total: session.evidence.nodes.len(),
            stop_reason: session.stop_reason.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }
}

#[derive(Clone)]
pub struct SessionRegistry {
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<ResearchSession>>>>>,
    aliases: Arc<RwLock<HashMap<String, String>>>,
    senders: Arc<RwLock<HashMap<String, broadcast::Sender<ResearchEvent>>>>,
    directory: PathBuf,
    budget: ResearchBudget,
}

impl SessionRegistry {
    pub fn load(directory: impl Into<PathBuf>, budget: ResearchBudget) -> Result<Self, String> {
        let directory = directory.into();
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let mut sessions = HashMap::new();
        let mut aliases = HashMap::new();
        let mut senders = HashMap::new();
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str::<ResearchSession>(&content).ok())
            {
                Some(mut session) if session.checkpoint_version == CHECKPOINT_VERSION => {
                    if matches!(
                        session.status,
                        SessionStatus::Active | SessionStatus::WaitingTool
                    ) {
                        session.status = SessionStatus::Recoverable;
                    }
                    for alias in session
                        .aliases
                        .iter()
                        .chain(std::iter::once(&session.fingerprint))
                    {
                        aliases.insert(alias.clone(), session.id.clone());
                    }
                    let (sender, _) = broadcast::channel(128);
                    senders.insert(session.id.clone(), sender);
                    sessions.insert(session.id.clone(), Arc::new(Mutex::new(session)));
                }
                _ => quarantine_corrupt(&path),
            }
        }
        Ok(Self {
            sessions: Arc::new(RwLock::new(sessions)),
            aliases: Arc::new(RwLock::new(aliases)),
            senders: Arc::new(RwLock::new(senders)),
            directory,
            budget,
        })
    }

    pub async fn resolve(
        &self,
        headers: &HeaderMap,
        body: &Value,
        first_user: &str,
        model: Option<&str>,
        api_type: ApiType,
    ) -> Option<Arc<Mutex<ResearchSession>>> {
        let has_explicit_session_id = headers.contains_key("x-super-instruct-session")
            || body
                .get("super_instruct")
                .and_then(|v| v.get("session_id"))
                .is_some();
        let candidates = identity_candidates(headers, body, first_user, model, api_type);
        let sessions = self.sessions.read().await;
        let aliases = self.aliases.read().await;
        for candidate in candidates {
            let session_opt = sessions
                .get(&candidate)
                .or_else(|| aliases.get(&candidate).and_then(|id| sessions.get(id)));
            if let Some(session) = session_opt {
                let guard = session.lock().await;
                if !has_explicit_session_id
                    && matches!(
                        guard.status,
                        SessionStatus::Completed | SessionStatus::Failed | SessionStatus::Stopped
                    )
                {
                    continue;
                }
                drop(guard);
                return Some(session.clone());
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        first_user: &str,
        model: Option<&str>,
        api_type: ApiType,
        contract: TaskContract,
        category: Category,
        route: RoutePlan,
        actions: ActionState,
        initial_request: Value,
    ) -> Result<Arc<Mutex<ResearchSession>>, String> {
        let fingerprint = session_fingerprint(first_user, model, api_type);
        let suffix = Utc::now().timestamp_millis().unsigned_abs();
        let id = format!("rs_{}_{}", &fingerprint[..16], suffix);
        let session = ResearchSession::new(
            id.clone(),
            fingerprint.clone(),
            contract,
            category,
            route,
            actions,
            initial_request,
            api_type,
            self.budget.clone(),
        );
        let session = Arc::new(Mutex::new(session));
        self.sessions
            .write()
            .await
            .insert(id.clone(), session.clone());
        self.aliases.write().await.insert(fingerprint, id.clone());
        let (sender, _) = broadcast::channel(128);
        self.senders.write().await.insert(id, sender);
        {
            let guard = session.lock().await;
            self.checkpoint(&guard)?;
        }
        Ok(session)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<Mutex<ResearchSession>>> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn len(&self) -> usize {
        self.sessions.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.sessions.read().await.is_empty()
    }

    pub async fn register_alias(&self, session: &mut ResearchSession, alias: impl Into<String>) {
        let alias = alias.into();
        if alias.is_empty() || session.aliases.contains(&alias) {
            return;
        }
        session.aliases.push(alias.clone());
        self.aliases.write().await.insert(alias, session.id.clone());
    }

    pub async fn record_event(
        &self,
        session: &mut ResearchSession,
        event: impl Into<String>,
        summary: impl Into<String>,
    ) {
        session.event_sequence += 1;
        let record = ResearchEvent {
            sequence: session.event_sequence,
            event: event.into(),
            session_id: session.id.clone(),
            timestamp: Utc::now(),
            stage: session.current_stage().kind.as_str().to_string(),
            round: session.round,
            status: session.status.as_str().to_string(),
            summary: summary.into(),
            evidence_total: session.evidence.nodes.len(),
            stop_reason: session.stop_reason.clone(),
        };
        if session.events.len() >= MAX_EVENTS {
            session.events.pop_front();
        }
        session.events.push_back(record.clone());
        if let Some(sender) = self.senders.read().await.get(&session.id) {
            let _ = sender.send(record);
        }
    }

    pub async fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<ResearchEvent>> {
        self.senders
            .read()
            .await
            .get(id)
            .map(|sender| sender.subscribe())
    }

    pub fn checkpoint(&self, session: &ResearchSession) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(session).map_err(|error| error.to_string())?;
        atomic_write(&self.directory.join(format!("{}.json", session.id)), &bytes)
    }

    pub async fn checkpoint_all(&self) {
        let sessions: Vec<Arc<Mutex<ResearchSession>>> =
            self.sessions.read().await.values().cloned().collect();
        for session in sessions {
            let guard = session.lock().await;
            if matches!(
                guard.status,
                SessionStatus::Active
                    | SessionStatus::WaitingTool
                    | SessionStatus::AwaitingApproval
                    | SessionStatus::Recoverable
            ) {
                if let Err(error) = self.checkpoint(&guard) {
                    tracing::error!(session_id = %guard.id, %error, "session checkpoint failed");
                }
            }
        }
    }
}

pub fn is_research_request(user: &str, category: &Category, intent: &RequestIntent) -> bool {
    if !matches!(category, Category::General) || !matches!(intent, RequestIntent::General) {
        return true;
    }
    let normalized = user.to_ascii_lowercase();
    [
        "研究",
        "调研",
        "分析",
        "评估",
        "验证",
        "research",
        "investigate",
        "analyze",
        "assess",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

pub fn is_approval_message(user: &str) -> bool {
    let marker = Regex::new(r"\[RESEARCH_SESSION:[^\]]+\]").expect("session marker regex");
    marker.replace_all(user, "").trim() == "继续"
}

pub fn session_fingerprint(first_user: &str, model: Option<&str>, api_type: ApiType) -> String {
    sha256_hex(
        format!(
            "{}\n{}\n{}",
            api_type.as_str(),
            model.unwrap_or("unknown-model"),
            first_user.trim()
        )
        .as_bytes(),
    )
}

fn identity_candidates(
    headers: &HeaderMap,
    body: &Value,
    first_user: &str,
    model: Option<&str>,
    api_type: ApiType,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(value) = headers
        .get("x-super-instruct-session")
        .and_then(|value| value.to_str().ok())
    {
        candidates.push(value.to_string());
    }
    if let Some(value) = body
        .get("super_instruct")
        .and_then(|value| value.get("session_id"))
        .and_then(Value::as_str)
    {
        candidates.push(value.to_string());
    }
    if let Some(value) = body.get("conversation") {
        if let Some(id) = value
            .as_str()
            .or_else(|| value.get("id").and_then(Value::as_str))
        {
            candidates.push(id.to_string());
        }
    }
    if let Some(value) = body.get("previous_response_id").and_then(Value::as_str) {
        candidates.push(value.to_string());
    }
    if let Some(marker) =
        extract_session_marker(&body.to_string()).or_else(|| extract_session_marker(first_user))
    {
        candidates.push(marker);
    }
    candidates.push(session_fingerprint(first_user, model, api_type));
    candidates
}

fn extract_session_marker(value: &str) -> Option<String> {
    let regex = Regex::new(r"\[RESEARCH_SESSION:([^\]]+)\]").ok()?;
    regex
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
}

fn stage_uses_evidence_budget(stage: StageKind) -> bool {
    matches!(
        stage,
        StageKind::Evidence
            | StageKind::Analysis
            | StageKind::Transformation
            | StageKind::Execution
            | StageKind::Verification
            | StageKind::Reporting
    )
}

fn output_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn canonicalize_json_text(value: &str) -> String {
    serde_json::from_str::<Value>(value)
        .map(|value| canonical_json(&value))
        .unwrap_or_else(|_| value.trim().to_string())
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let values = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()),
                        canonical_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{values}}}")
        }
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()),
    }
}

fn extract_exit_code(output: &str) -> Option<i32> {
    let regex = Regex::new(r"(?i)exit[_ ]?code\s*[:=]\s*(-?\d+)").ok()?;
    regex
        .captures(output)
        .and_then(|captures| captures.get(1))
        .and_then(|capture| capture.as_str().parse().ok())
}

fn extract_path(output: &str) -> Option<String> {
    let regex = Regex::new(r"(?m)(/[A-Za-z0-9._~+@%/=-]+)").ok()?;
    regex
        .captures(output)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

fn quarantine_corrupt(path: &Path) {
    let corrupt = path.with_extension(format!("corrupt.{}", Utc::now().timestamp_millis()));
    let _ = fs::rename(path, corrupt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{build_contract, CompetitionRouter, ContextRouter};

    fn session() -> ResearchSession {
        let category = Category::Pentest;
        let (route, actions) = CompetitionRouter.plan("analyze target and verify", &category);
        let contract = build_contract("analyze target and verify", &category, &actions.pending);
        ResearchSession::new(
            "rs_test".to_string(),
            "fingerprint".to_string(),
            contract,
            category,
            route,
            actions,
            serde_json::json!({"model":"test","input":[]}),
            ApiType::Responses,
            ResearchBudget::default(),
        )
    }

    #[test]
    fn approval_parser_only_accepts_continue_control_message() {
        assert!(is_approval_message("继续 [RESEARCH_SESSION:rs_test]"));
        assert!(!is_approval_message("继续分析这个问题"));
    }

    #[test]
    fn evidence_graph_deduplicates_normalized_tool_output() {
        let mut graph = EvidenceGraph::default();
        let first = graph.insert(
            "strings",
            r#"{"path":"sample","flags":["-a","-n"]}"#,
            Value::String("output".into()),
        );
        let second = graph.insert(
            "strings",
            r#"{ "flags": ["-a", "-n"], "path": "sample" }"#,
            Value::String("output".into()),
        );
        assert!(first.1);
        assert!(!second.1);
        assert_eq!(first.0, second.0);
    }

    #[test]
    fn stage_machine_starts_with_framing() {
        let session = session();
        assert_eq!(session.current_stage().kind, StageKind::Framing);
        assert_eq!(session.status, SessionStatus::Active);
    }

    #[test]
    fn round_budget_stops_session() {
        let mut session = session();
        session.budget.max_rounds = 1;
        session.start_round().expect("first round");
        assert!(session.start_round().is_err());
        assert_eq!(session.status, SessionStatus::Stopped);
        assert_eq!(session.stop_reason.as_deref(), Some("round_budget"));
    }

    #[test]
    fn identity_candidates_follow_declared_priority() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-super-instruct-session",
            http::HeaderValue::from_static("header-session"),
        );
        let body = serde_json::json!({
            "super_instruct":{"session_id":"body-session"},
            "conversation":"conversation-session",
            "previous_response_id":"response-session",
            "input":[{"role":"user","content":"continue [RESEARCH_SESSION:marker-session]"}]
        });
        let candidates = identity_candidates(
            &headers,
            &body,
            "continue [RESEARCH_SESSION:marker-session]",
            Some("model"),
            ApiType::Responses,
        );
        assert_eq!(
            &candidates[..5],
            [
                "header-session",
                "body-session",
                "conversation-session",
                "response-session",
                "marker-session"
            ]
        );
    }

    #[test]
    fn stage_commit_is_idempotent() {
        let mut session = session();
        session.start_round().expect("round");
        let evidence = session.evidence.ids()[0].clone();
        let result = StageResultV1 {
            session_id: session.id.clone(),
            stage: StageKind::Framing,
            status: "completed".into(),
            summary: "framed".into(),
            observations: vec![crate::core::protocol::ResearchClaimV1 {
                statement: "request observed".into(),
                evidence_refs: vec![evidence.clone()],
            }],
            inferences: Vec::new(),
            hypotheses: Vec::new(),
            evidence_refs: vec![evidence],
            artifacts: Vec::new(),
            limitations: Vec::new(),
            unresolved: Vec::new(),
            next_stage: Some(StageKind::Planning),
        };
        let score = session
            .validate_stage_result(&result, "call_1")
            .expect("valid stage");
        session.commit_stage(result.clone(), "call_1", score);
        session.commit_stage(result, "call_1", score);
        assert_eq!(session.stage_records.len(), 1);
        assert_eq!(session.evidence.edges.len(), 1);
        assert_eq!(session.evidence.edges[0].source_kind, "observation");
        assert_eq!(session.current_stage().kind, StageKind::Planning);
    }

    #[tokio::test]
    async fn checkpoint_restores_awaiting_session() {
        let directory = std::env::temp_dir().join(format!(
            "super-instruct-session-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let registry =
            SessionRegistry::load(&directory, ResearchBudget::default()).expect("registry");
        let template = session();
        let created = registry
            .create(
                &template.contract.original_request,
                Some("test-model"),
                ApiType::Responses,
                template.contract.clone(),
                template.category.clone(),
                template.route.clone(),
                template.actions.clone(),
                template.provider.request.clone(),
            )
            .await
            .expect("create session");
        let id = {
            let mut guard = created.lock().await;
            guard.status = SessionStatus::AwaitingApproval;
            registry.checkpoint(&guard).expect("checkpoint");
            guard.id.clone()
        };
        drop(registry);
        let restored =
            SessionRegistry::load(&directory, ResearchBudget::default()).expect("restore registry");
        let restored = restored.get(&id).await.expect("restored session");
        assert_eq!(
            restored.lock().await.status,
            SessionStatus::AwaitingApproval
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn no_evidence_limit_is_reached_after_two_evidence_rounds() {
        let mut session = session();
        session.current_stage_index = session
            .stages
            .iter()
            .position(|stage| stage.kind == StageKind::Evidence)
            .unwrap();
        session.start_round().expect("evidence round one");
        session.finish_round();
        assert!(!session.no_evidence_limit_reached());
        session.start_round().expect("evidence round two");
        session.finish_round();
        assert!(session.no_evidence_limit_reached());
    }

    #[test]
    fn ordinary_general_chat_is_not_research() {
        assert!(!is_research_request(
            "hello, how are you",
            &Category::General,
            &RequestIntent::General
        ));
        assert!(is_research_request(
            "research this behavior",
            &Category::General,
            &RequestIntent::General
        ));
    }
}

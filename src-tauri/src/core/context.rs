// Core context types — 管道各阶段的数据载体

use super::contract::TaskContract;
use super::execution::ExecutionMode;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::HeaderMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DagNode {
    pub id: String,
    pub action: String,
    pub depends_on: Vec<String>,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DagPlan {
    pub version: u32,
    pub nodes: Vec<DagNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum UpstreamOutcome {
    ModelSuccess,
    ModelRefusal,
    TaskDivergence,
    ProviderPolicyBlock,
    TransportError,
    ProtocolError,
    Cancelled,
}

impl DagPlan {
    pub fn empty() -> Self {
        Self {
            version: 1,
            nodes: Vec::new(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status == "pending")
            .count()
    }
}

#[derive(Clone, Debug)]
pub enum Category {
    Crack,
    Reverse,
    Pentest,
    MalwareAnalysis,
    General,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActionState {
    pub completed: Vec<String>,
    pub pending: Vec<String>,
    pub dag: DagPlan,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoutePlan {
    pub profile: String,
    pub skills: Vec<String>,
    pub inject_request: bool,
    pub persist_memory: bool,
    pub monitor: bool,
    pub confidence: f32,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Crack => "crack",
            Category::Reverse => "reverse",
            Category::Pentest => "pentest",
            Category::MalwareAnalysis => "malware-analysis",
            Category::General => "general",
        }
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 请求阶段元数据 — 从用户消息中提取
#[derive(Clone)]
pub struct RequestMeta {
    pub request_id: String,
    pub model: Option<String>,
    pub user_msg: String,
    pub category: Category,
    pub route: RoutePlan,
    pub actions: ActionState,
    pub contract: TaskContract,
    pub execution_mode: ExecutionMode,
    pub path: String,
    pub timestamp: DateTime<Utc>,
}

/// 请求上下文 — 请求拦截器操作的目标
pub struct RequestCtx {
    pub meta: RequestMeta,
    pub headers: HeaderMap,
    pub body: serde_json::Value,
}

/// 响应解析结果 — ResponseParser 输出
pub struct ParsedResponse {
    pub thinking: String,
    pub reply: String,
}

/// 响应上下文 — 响应拦截器操作的目标
pub struct ResponseCtx {
    pub meta: RequestMeta,
    pub status: u16,
    pub raw_body: Bytes,
    pub parsed: ParsedResponse,
    pub duration_ms: u64,
    pub outcome: UpstreamOutcome,
}

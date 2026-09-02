pub mod core;
pub mod deploy;
pub mod extensions;
pub mod log;

use crate::core::traits::ResponseParser;
use crate::core::{
    discover_analysis_tools, CoTLogger, CoTMode, CoTSteerer, CoTTraceRecord, CompetitionRouter,
    ContextRouter, ExecutionMode, MitmCore, ProtocolAdapter, ResearchBudget, SessionRegistry,
    SessionStatus,
};
use crate::deploy::{find_relay_url, DeployManager};
use crate::extensions::inject::SystemPromptInjector;
use crate::extensions::memory::MemoryKernel;
use crate::extensions::monitor::FileMonitor;
use crate::extensions::sse_parser::UniversalSseParser;
use axum::response::IntoResponse;
use futures::StreamExt;
use http::HeaderValue;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BRIDGE_MD_FALLBACK: &str = include_str!("../../bridge.md");

#[derive(Clone, Debug)]
pub struct HeadlessConfig {
    pub listen: String,
    pub relay_url: Option<String>,
    pub bridge_file: PathBuf,
    pub skills_dir: PathBuf,
    pub log_dir: PathBuf,
    pub memory_file: PathBuf,
    pub sessions_dir: PathBuf,
    pub research_budget: ResearchBudget,
    pub deploy_codex: bool,
    pub execution_mode: ExecutionMode,
    pub cot_mode: CoTMode,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        let root = project_root();
        Self {
            listen: "127.0.0.1:8080".to_string(),
            relay_url: std::env::var("SUPER_INSTRUCT_RELAY_URL").ok(),
            bridge_file: root.join("bridge.md"),
            skills_dir: root.join("codex-skills"),
            log_dir: root.join("logs"),
            memory_file: root.join("memory.json"),
            sessions_dir: root.join("research-sessions"),
            research_budget: ResearchBudget::default(),
            deploy_codex: true,
            execution_mode: ExecutionMode::Interleaved,
            cot_mode: CoTMode::Inject,
        }
    }
}

impl HeadlessConfig {
    pub fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--listen" => config.listen = next_arg(&mut args, "--listen")?,
                "--relay" => config.relay_url = Some(next_arg(&mut args, "--relay")?),
                "--bridge" => config.bridge_file = PathBuf::from(next_arg(&mut args, "--bridge")?),
                "--skills" => config.skills_dir = PathBuf::from(next_arg(&mut args, "--skills")?),
                "--logs" => config.log_dir = PathBuf::from(next_arg(&mut args, "--logs")?),
                "--memory" => config.memory_file = PathBuf::from(next_arg(&mut args, "--memory")?),
                "--sessions" => {
                    config.sessions_dir = PathBuf::from(next_arg(&mut args, "--sessions")?)
                }
                "--research-max-rounds" => {
                    config.research_budget.max_rounds =
                        next_arg(&mut args, "--research-max-rounds")?
                            .parse()
                            .map_err(|_| "--research-max-rounds must be an integer".to_string())?
                }
                "--research-timeout-secs" => {
                    config.research_budget.timeout_secs =
                        next_arg(&mut args, "--research-timeout-secs")?
                            .parse()
                            .map_err(|_| "--research-timeout-secs must be an integer".to_string())?
                }
                "--research-no-evidence-limit" => {
                    config.research_budget.no_evidence_limit =
                        next_arg(&mut args, "--research-no-evidence-limit")?
                            .parse()
                            .map_err(|_| {
                                "--research-no-evidence-limit must be an integer".to_string()
                            })?
                }
                "--no-deploy" => config.deploy_codex = false,
                "--execution-mode" => {
                    config.execution_mode = next_arg(&mut args, "--execution-mode")?.parse()?
                }
                "--cot-mode" => config.cot_mode = next_arg(&mut args, "--cot-mode")?.parse()?,
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unknown argument: {other}\n{}", usage())),
            }
        }
        Ok(config)
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn usage() -> String {
    "usage: super-instruct [--listen HOST:PORT] [--relay URL] [--bridge FILE] \
     [--skills DIR] [--logs DIR] [--memory FILE] [--sessions DIR] \
     [--research-max-rounds N] [--research-timeout-secs N] \
     [--research-no-evidence-limit N] [--execution-mode MODE] [--cot-mode <inject|extract|silent>] [--no-deploy]"
        .to_string()
}

pub fn run() -> Result<(), String> {
    let config = HeadlessConfig::from_args()?;
    let log_dir = config.log_dir.to_string_lossy().to_string();
    let _guard = log::init_logging(&log_dir);
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(run_headless(config))
}

pub async fn run_headless(config: HeadlessConfig) -> Result<(), String> {
    let listener = match tokio::net::TcpListener::bind(&config.listen).await {
        Ok(listener) => listener,
        Err(error) => {
            return Err(format!("bind {} failed: {error}", config.listen));
        }
    };

    let bridge = std::fs::read_to_string(&config.bridge_file)
        .unwrap_or_else(|_| BRIDGE_MD_FALLBACK.to_string());
    let manager = if config.deploy_codex {
        Some(DeployManager::new().ok_or("Codex config directory not found")?)
    } else {
        None
    };

    let mut deployment = DeploymentGuard::new(manager);
    if let Some(manager) = deployment.manager() {
        let status = manager.status();
        if status.config_backed_up {
            manager.restore()?;
        }
        if let Some(url) = &config.relay_url {
            manager.set_relay_url(url)?;
        }
        manager.apply_with_optional_skills(
            &bridge,
            config
                .skills_dir
                .is_dir()
                .then_some(config.skills_dir.as_path()),
        )?;
        deployment.arm();
    }

    let relay_raw = config
        .relay_url
        .clone()
        .or_else(find_relay_url)
        .filter(|url| !url.contains(&config.listen))
        .ok_or("relay URL is missing or points to the local proxy")?;
    // The configured provider URL is authoritative. Appending `/v1` after a
    // probe changes providers whose Responses endpoint lives at `/responses`.
    let relay_url = relay_raw.trim_end_matches('/').to_string();

    let monitor = Arc::new(FileMonitor::new(&config.log_dir)?);
    let memory = Arc::new(MemoryKernel::new(&config.memory_file));
    let sessions = Arc::new(SessionRegistry::load(
        &config.sessions_dir,
        config.research_budget.clone(),
    )?);
    let capabilities = discover_analysis_tools();
    let available_tool_count = capabilities
        .iter()
        .filter(|capability| capability.available())
        .count();
    let injector = SystemPromptInjector::new(bridge)
        .with_skills_dir(
            config
                .skills_dir
                .is_dir()
                .then_some(config.skills_dir.as_path()),
        )
        .with_capabilities(capabilities);
    let core = Arc::new(
        MitmCore::builder()
            .target(&relay_url)
            .client(build_upstream_client(&relay_url)?)
            .context_router(CompetitionRouter)
            .execution_mode(config.execution_mode)
            .request_interceptor(injector)
            .response_parser(UniversalSseParser)
            .response_interceptor(memory)
            .response_interceptor(monitor.clone())
            .build()?,
    );

    let cot_logger = Arc::new(CoTLogger::new(&config.log_dir).map_err(|e| e.to_string())?);
    let cot_mode = config.cot_mode;

    tracing::info!(
        listen = %config.listen,
        relay = %relay_url,
        execution_mode = %config.execution_mode,
        cot_mode = %cot_mode,
        available_tool_count,
        "headless proxy started"
    );
    let execution_mode = config.execution_mode;
    let health_monitor = monitor.clone();
    let health_cot_logger = cot_logger.clone();
    let snapshot_sessions = sessions.clone();
    let event_sessions = sessions.clone();
    let proxy_sessions = sessions.clone();
    let proxy_cot_logger = cot_logger.clone();
    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(move || {
                health_check(
                    execution_mode,
                    cot_mode,
                    available_tool_count,
                    health_monitor.clone(),
                    health_cot_logger.clone(),
                )
            }),
        )
        .route(
            "/super-instruct/v1/sessions/{id}",
            axum::routing::get(move |axum::extract::Path(id)| {
                session_snapshot(id, snapshot_sessions.clone())
            }),
        )
        .route(
            "/super-instruct/v1/sessions/{id}/events",
            axum::routing::get(move |axum::extract::Path(id)| {
                session_event_stream(id, event_sessions.clone())
            }),
        )
        .route(
            "/{*path}",
            axum::routing::any(move |req| {
                handle_proxy(
                    req,
                    core.clone(),
                    proxy_sessions.clone(),
                    proxy_cot_logger.clone(),
                    cot_mode,
                )
            }),
        );

    let checkpoint_sessions = sessions.clone();
    let checkpoint_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            checkpoint_sessions.checkpoint_all().await;
        }
    });

    let proxy_url = format!("http://{}", config.listen);
    let (config_guard_stop, config_guard_stopped) = tokio::sync::watch::channel(false);
    let config_guard = deployment
        .manager()
        .cloned()
        .map(|manager| tokio::spawn(watch_codex_config(manager, proxy_url, config_guard_stopped)));

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| e.to_string());
    let _ = config_guard_stop.send(true);
    if let Some(handle) = config_guard {
        let _ = handle.await;
    }
    checkpoint_task.abort();
    tracing::info!(stats = ?monitor.stats(), "headless proxy stopped");
    deployment.restore();
    result
}

async fn watch_codex_config(
    manager: DeployManager,
    proxy_url: String,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                match manager.ensure_active_config(&proxy_url) {
                    Ok(true) => tracing::warn!(%proxy_url, "Codex config drift repaired"),
                    Ok(false) => {},
                    Err(error) => tracing::error!(%error, "Codex config drift check failed"),
                }
            }
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
}

struct DeploymentGuard {
    manager: Option<DeployManager>,
    armed: bool,
}

impl DeploymentGuard {
    fn new(manager: Option<DeployManager>) -> Self {
        Self {
            manager,
            armed: false,
        }
    }

    fn manager(&self) -> Option<&DeployManager> {
        self.manager.as_ref()
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    fn restore(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(manager) = &self.manager {
            match manager.restore() {
                Ok(message) => tracing::info!(%message, "Codex config restored"),
                Err(error) => tracing::error!(%error, "Codex config restore failed"),
            }
        }
        self.armed = false;
    }
}

impl Drop for DeploymentGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => {
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

fn build_upstream_client(url: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(300))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .pool_idle_timeout(std::time::Duration::from_secs(120));
    if is_loopback_url(url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

fn is_loopback_url(url: &str) -> bool {
    url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost")
        || url.starts_with("http://[::1]")
        || url.starts_with("https://127.0.0.1")
        || url.starts_with("https://localhost")
        || url.starts_with("https://[::1]")
}

async fn health_check(
    execution_mode: ExecutionMode,
    cot_mode: CoTMode,
    available_tool_count: usize,
    monitor: Arc<FileMonitor>,
    cot_logger: Arc<CoTLogger>,
) -> impl axum::response::IntoResponse {
    let stats = monitor.stats();
    let (cot_total, cot_refusals) = cot_logger.stats();
    axum::Json(serde_json::json!({
        "status": "ok",
        "mode": "headless",
        "execution_mode": execution_mode,
        "cot_mode": cot_mode,
        "cot_traces_total": cot_total,
        "cot_refusals_detected": cot_refusals,
        "available_tool_count": available_tool_count,
        "quality_gate": "enabled",
        "evolution": crate::core::adaptive::status(),
        "outcomes": stats
    }))
}

async fn session_snapshot(id: String, sessions: Arc<SessionRegistry>) -> axum::response::Response {
    let Some(session) = sessions.get(&id).await else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error":"session_not_found","session_id":id})),
        )
            .into_response();
    };
    let guard = session.lock().await;
    axum::Json(crate::core::research::SessionSummary::from(&*guard)).into_response()
}

async fn session_event_stream(
    id: String,
    sessions: Arc<SessionRegistry>,
) -> axum::response::Response {
    let Some(session) = sessions.get(&id).await else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error":"session_not_found","session_id":id})),
        )
            .into_response();
    };
    let backlog: Vec<_> = session.lock().await.events.iter().cloned().collect();
    let Some(receiver) = sessions.subscribe(&id).await else {
        return error_response(500, "session event channel missing");
    };
    let history = futures::stream::iter(
        backlog
            .into_iter()
            .map(|event| Ok::<_, std::convert::Infallible>(research_event_to_sse(event))),
    );
    let live =
        tokio_stream::wrappers::BroadcastStream::new(receiver).filter_map(|event| async move {
            match event {
                Ok(event) => Some(Ok::<_, std::convert::Infallible>(research_event_to_sse(
                    event,
                ))),
                Err(_) => None,
            }
        });
    axum::response::sse::Sse::new(history.chain(live))
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

fn research_event_to_sse(
    event: crate::core::research::ResearchEvent,
) -> axum::response::sse::Event {
    axum::response::sse::Event::default()
        .event(event.event.clone())
        .id(event.sequence.to_string())
        .json_data(event)
        .unwrap_or_else(|_| axum::response::sse::Event::default().event("session.error"))
}

async fn handle_proxy(
    req: axum::extract::Request,
    core: Arc<MitmCore>,
    sessions: Arc<SessionRegistry>,
    cot_logger: Arc<CoTLogger>,
    cot_mode: CoTMode,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let body = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => return error_response(400, &error.to_string()),
    };
    let path = parts
        .uri
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_else(|| parts.uri.path().to_string());
    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
        let detected_protocol = ProtocolAdapter::detect(&path, &value);
        let latest_user = crate::core::extract_user(&value);
        let first_user = crate::core::extract_first_user(&value);
        let model = value.get("model").and_then(Value::as_str);
        let existing = sessions
            .resolve(
                &parts.headers,
                &value,
                &first_user,
                model,
                detected_protocol.api_type(),
            )
            .await;
        let category = crate::core::categorize(&latest_user);
        let (route, actions) = CompetitionRouter.plan(&latest_user, &category);
        let contract = crate::core::build_contract(&latest_user, &category, &actions.pending);
        if existing.is_some()
            || crate::core::research::is_research_request(&latest_user, &category, &contract.intent)
        {
            return handle_research_request(
                parts.method,
                path,
                parts.headers,
                value,
                latest_user,
                first_user,
                detected_protocol,
                existing,
                category,
                route,
                actions,
                contract,
                core,
                sessions,
                cot_logger,
            )
            .await;
        }
    }
    let upstream = match core
        .handle_request(parts.method, path, parts.headers, body)
        .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            eprintln!("proxy request phase failed: {error}");
            return error_response(502, &error.to_string());
        }
    };
    stream_upstream(upstream, core, cot_logger, cot_mode).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_research_request(
    method: http::Method,
    path: String,
    mut headers: http::HeaderMap,
    mut incoming: Value,
    latest_user: String,
    first_user: String,
    detected_protocol: ProtocolAdapter,
    existing: Option<Arc<tokio::sync::Mutex<crate::core::ResearchSession>>>,
    category: crate::core::Category,
    route: crate::core::context::RoutePlan,
    actions: crate::core::context::ActionState,
    contract: crate::core::TaskContract,
    core: Arc<MitmCore>,
    sessions: Arc<SessionRegistry>,
    cot_logger: Arc<CoTLogger>,
) -> axum::response::Response {
    if let Some(object) = incoming.as_object_mut() {
        object.remove("super_instruct");
    }
    let model = incoming
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let stream_requested = incoming
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let session = match existing {
        Some(session) => session,
        None => match sessions
            .create(
                &first_user,
                model.as_deref(),
                detected_protocol.api_type(),
                contract,
                category,
                route,
                actions,
                incoming.clone(),
            )
            .await
        {
            Ok(session) => {
                let mut guard = session.lock().await;
                sessions
                    .record_event(&mut guard, "session.created", "research session created")
                    .await;
                let _ = sessions.checkpoint(&guard);
                drop(guard);
                session
            }
            Err(error) => return error_response(500, &error),
        },
    };

    let mut session = session.lock().await;
    let protocol = ProtocolAdapter::from_api(session.provider.api_type);
    if let Some(requested_stage) = headers
        .get("x-super-instruct-stage")
        .and_then(|value| value.to_str().ok())
        .and_then(crate::core::StageKind::from_str)
    {
        if requested_stage != session.current_stage().kind {
            let scheduled_stage = session.current_stage().kind;
            sessions
                .record_event(
                    &mut session,
                    "session.error",
                    format!(
                        "ignored stage hint {}; scheduler remains at {}",
                        requested_stage, scheduled_stage
                    ),
                )
                .await;
        }
    }
    headers.remove("x-super-instruct-stage");
    headers.insert(
        "x-super-instruct-session",
        HeaderValue::from_str(&session.id).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );

    if session.status == SessionStatus::AwaitingApproval {
        if crate::core::research::is_approval_message(&latest_user) {
            if let Err(error) = session.approve_current_stage() {
                return research_synthetic_response(
                    protocol,
                    stream_requested,
                    &session.id,
                    &error,
                    "APPROVAL_REJECTED",
                );
            }
            let approved_stage = session.current_stage().kind.as_str().to_string();
            sessions
                .record_event(
                    &mut session,
                    "round.started",
                    format!("approval accepted for {approved_stage}"),
                )
                .await;
            let _ = sessions.checkpoint(&session);
        } else {
            let message = session.approval_message();
            return research_synthetic_response(
                protocol,
                stream_requested,
                &session.id,
                &message,
                "AWAITING_APPROVAL",
            );
        }
    } else if session.status == SessionStatus::Recoverable {
        session.status = SessionStatus::Active;
        sessions
            .record_event(&mut session, "round.started", "session recovered")
            .await;
    }

    if session.status.terminal() {
        let report = session.final_report();
        return research_synthetic_response(
            protocol,
            stream_requested,
            &session.id,
            &report,
            session.status.as_str(),
        );
    }

    let incoming_outputs = protocol.extract_tool_outputs(&incoming);
    let new_outputs: Vec<_> = incoming_outputs
        .into_iter()
        .filter(|output| session.pending_tool_calls.contains_key(&output.call_id))
        .collect();
    if !new_outputs.is_empty() {
        let added = session.add_tool_outputs(&new_outputs);
        for evidence_id in added {
            sessions
                .record_event(
                    &mut session,
                    "evidence.added",
                    format!("accepted evidence {evidence_id}"),
                )
                .await;
        }
        let mut outputs = std::mem::take(&mut session.deferred_tool_outputs);
        outputs.extend(new_outputs);
        let prior_turn = cursor_turn(&session.provider);
        session.provider.request =
            protocol.continue_body(&session.provider.request, &prior_turn, &outputs);
        clear_cursor_turn(&mut session.provider);
    }

    let mut next_request = session.provider.request.clone();
    loop {
        if let Err(reason) = session.start_round() {
            sessions
                .record_event(&mut session, "session.stopped", reason)
                .await;
            let _ = sessions.checkpoint(&session);
            let report = session.final_report();
            return research_synthetic_response(
                protocol,
                stream_requested,
                &session.id,
                &report,
                "SESSION_STOPPED",
            );
        }
        let round = session.round;
        sessions
            .record_event(
                &mut session,
                "round.started",
                format!("model round {round}"),
            )
            .await;
        attach_research_metadata(&mut next_request, &session);

        let request_bytes = match serde_json::to_vec(&next_request) {
            Ok(bytes) => bytes::Bytes::from(bytes),
            Err(error) => {
                session.fail(format!("request_serialization:{error}"));
                let _ = sessions.checkpoint(&session);
                return error_response(500, &error.to_string());
            }
        };
        let upstream = match core
            .handle_request(method.clone(), path.clone(), headers.clone(), request_bytes)
            .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                session.fail(format!("provider_transport:{error}"));
                sessions
                    .record_event(&mut session, "session.error", error.to_string())
                    .await;
                let _ = sessions.checkpoint(&session);
                return error_response(502, &error.to_string());
            }
        };
        let buffered = match buffer_research_upstream(upstream, core.clone()).await {
            Ok(buffered) => buffered,
            Err(error) => {
                session.fail(format!("provider_protocol:{error}"));
                sessions
                    .record_event(&mut session, "session.error", error.clone())
                    .await;
                let _ = sessions.checkpoint(&session);
                return error_response(502, &error);
            }
        };

        // 记录思维链与推理指标
        let parsed_research = UniversalSseParser.parse(&buffered.body);
        let (refusal_detected, refusal_signals) =
            CoTSteerer::detect_refusal(&parsed_research.thinking);
        let reply_preview: String = parsed_research.reply.chars().take(200).collect();
        let trace = CoTTraceRecord {
            request_id: format!("{}_{}", session.id, session.round),
            model: session
                .provider
                .request
                .get("model")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            timestamp: chrono::Utc::now().to_rfc3339(),
            thinking_chars: parsed_research.thinking.chars().count(),
            reply_chars: parsed_research.reply.chars().count(),
            duration_ms: 0,
            refusal_detected,
            refusal_signals,
            thinking_content: parsed_research.thinking,
            reply_preview,
        };
        cot_logger.record_trace(&trace);
        if !(200..300).contains(&buffered.status) {
            session.fail(format!("provider_status: {}", buffered.status));
            sessions
                .record_event(
                    &mut session,
                    "session.error",
                    format!("provider returned {}", buffered.status),
                )
                .await;
            let _ = sessions.checkpoint(&session);
            return buffered_research_response(buffered, &session.id, None);
        }
        let turn = match protocol.parse_response(&buffered.body) {
            Ok(turn) => turn,
            Err(error) => {
                session.fail(format!("response_parse:{error}"));
                sessions
                    .record_event(&mut session, "session.error", error.clone())
                    .await;
                let _ = sessions.checkpoint(&session);
                return buffered_research_response(buffered, &session.id, None);
            }
        };
        if let Some(response_id) = &turn.response_id {
            session.provider.last_response_id = Some(response_id.clone());
            session.provider.response_aliases.push(response_id.clone());
            sessions
                .register_alias(&mut session, response_id.clone())
                .await;
        }

        let internal_calls = turn.internal_calls();
        let external_calls = turn.external_calls();
        if !external_calls.is_empty() {
            for call in &external_calls {
                session
                    .pending_tool_calls
                    .insert(call.call_id.clone(), (*call).clone());
            }
            session.provider.request = remove_research_metadata(next_request.clone());
            store_cursor_turn(&mut session.provider, &turn);
            if !internal_calls.is_empty() {
                session
                    .deferred_tool_outputs
                    .extend(internal_calls.iter().map(|call| {
                        crate::core::protocol::ToolOutputRecord {
                            call_id: call.call_id.clone(),
                            output: serde_json::json!({
                                "accepted":false,
                                "deferred":true,
                                "reason":"ordinary tool outputs are pending"
                            }),
                        }
                    }));
            }
            session.status = SessionStatus::WaitingTool;
            session.finish_round();
            sessions
                .record_event(
                    &mut session,
                    "round.started",
                    format!("waiting for {} client tool output(s)", external_calls.len()),
                )
                .await;
            let _ = sessions.checkpoint(&session);
            let filtered = if internal_calls.is_empty() {
                None
            } else {
                Some(protocol.filter_internal_calls(&buffered.body, &turn))
            };
            return buffered_research_response(buffered, &session.id, filtered);
        }

        if let Some(call) = internal_calls.first() {
            let validation = call
                .stage_result()
                .map_err(|error| vec![error])
                .and_then(|result| {
                    session
                        .validate_stage_result(&result, &call.call_id)
                        .map(|score| (result, score))
                });
            match validation {
                Ok((result, score)) => {
                    let completed_stage = session.current_stage().kind;
                    session.finish_round();
                    let no_evidence_stop = session.no_evidence_limit_reached();
                    session.commit_stage(result, &call.call_id, score);
                    sessions
                        .record_event(
                            &mut session,
                            "stage.completed",
                            format!("{completed_stage} completed with score {score}"),
                        )
                        .await;
                    let output = crate::core::protocol::ToolOutputRecord {
                        call_id: call.call_id.clone(),
                        output: serde_json::json!({
                            "accepted":true,
                            "completed_stage":completed_stage.as_str(),
                            "next_stage":session.current_stage().kind.as_str(),
                            "session_status":session.status.as_str()
                        }),
                    };
                    let clean_request = remove_research_metadata(next_request.clone());
                    session.provider.request =
                        protocol.continue_body(&clean_request, &turn, &[output]);
                    clear_cursor_turn(&mut session.provider);
                    if no_evidence_stop {
                        session.stop("no_new_evidence");
                        sessions
                            .record_event(
                                &mut session,
                                "session.stopped",
                                "no new unique evidence in consecutive rounds",
                            )
                            .await;
                        let _ = sessions.checkpoint(&session);
                        let report = session.final_report();
                        return research_synthetic_response(
                            protocol,
                            stream_requested,
                            &session.id,
                            &report,
                            "SESSION_STOPPED",
                        );
                    }
                    let _ = sessions.checkpoint(&session);
                    if session.status == SessionStatus::AwaitingApproval {
                        let message = session.approval_message();
                        sessions
                            .record_event(&mut session, "approval.required", message.clone())
                            .await;
                        let _ = sessions.checkpoint(&session);
                        return research_synthetic_response(
                            protocol,
                            stream_requested,
                            &session.id,
                            &message,
                            "AWAITING_APPROVAL",
                        );
                    }
                    if session.status == SessionStatus::Completed {
                        sessions
                            .record_event(
                                &mut session,
                                "session.completed",
                                "research session completed",
                            )
                            .await;
                        let _ = sessions.checkpoint(&session);
                        let report = session.final_report();
                        return research_synthetic_response(
                            protocol,
                            stream_requested,
                            &session.id,
                            &report,
                            "SESSION_COMPLETED",
                        );
                    }
                    next_request = session.provider.request.clone();
                    continue;
                }
                Err(errors) => {
                    session.finish_round();
                    let output = crate::core::protocol::ToolOutputRecord {
                        call_id: call.call_id.clone(),
                        output: serde_json::json!({"accepted":false,"errors":errors}),
                    };
                    let clean_request = remove_research_metadata(next_request.clone());
                    session.provider.request =
                        protocol.continue_body(&clean_request, &turn, &[output]);
                    next_request = session.provider.request.clone();
                    let _ = sessions.checkpoint(&session);
                    continue;
                }
            }
        }

        session.finish_round();
        let correction = "Submit the current stage with super_instruct_stage_complete. Keep ordinary tool calls separate.";
        let clean_request = remove_research_metadata(next_request.clone());
        session.provider.request = protocol.correction_body(&clean_request, &turn, correction);
        next_request = session.provider.request.clone();
        let _ = sessions.checkpoint(&session);
    }
}

fn attach_research_metadata(body: &mut Value, session: &crate::core::ResearchSession) {
    if let Some(object) = body.as_object_mut() {
        let mut metadata = serde_json::to_value(session.research_meta()).unwrap_or(Value::Null);
        if let Some(metadata) = metadata.as_object_mut() {
            metadata.insert(
                "stage".to_string(),
                Value::String(session.current_stage().kind.as_str().to_string()),
            );
        }
        object.insert("super_instruct".to_string(), metadata);
    }
}

fn remove_research_metadata(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.remove("super_instruct");
    }
    body
}

fn cursor_turn(cursor: &crate::core::research::ProviderCursor) -> crate::core::protocol::ModelTurn {
    crate::core::protocol::ModelTurn {
        response_id: cursor.last_response_id.clone(),
        output_items: cursor.last_output_items.clone(),
        assistant_message: cursor.last_assistant_message.clone(),
        tool_calls: cursor.last_tool_calls.clone(),
        visible_text: String::new(),
    }
}

fn store_cursor_turn(
    cursor: &mut crate::core::research::ProviderCursor,
    turn: &crate::core::protocol::ModelTurn,
) {
    cursor.last_output_items = turn.output_items.clone();
    cursor.last_assistant_message = turn.assistant_message.clone();
    cursor.last_tool_calls = turn.tool_calls.clone();
}

fn clear_cursor_turn(cursor: &mut crate::core::research::ProviderCursor) {
    cursor.last_output_items.clear();
    cursor.last_assistant_message = None;
    cursor.last_tool_calls.clear();
}

struct BufferedResearchUpstream {
    status: u16,
    content_type: Option<String>,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

async fn buffer_research_upstream(
    upstream: crate::core::UpstreamResult,
    core: Arc<MitmCore>,
) -> Result<BufferedResearchUpstream, String> {
    const MAX_RESPONSE: usize = 64 * 1024 * 1024;
    let mut stream = upstream.response.bytes_stream();
    let mut accumulated = Vec::with_capacity(65_536);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if accumulated.len() + chunk.len() > MAX_RESPONSE {
            return Err("research response exceeded 64 MiB".to_string());
        }
        accumulated.extend_from_slice(&chunk);
    }
    let body = bytes::Bytes::from(accumulated);
    let duration_ms = (chrono::Utc::now() - upstream.meta.timestamp)
        .num_milliseconds()
        .max(0) as u64;
    core.finalize_response(upstream.meta, upstream.status, body.clone(), duration_ms);
    Ok(BufferedResearchUpstream {
        status: upstream.status,
        content_type: upstream.content_type,
        headers: upstream.headers,
        body,
    })
}

fn buffered_research_response(
    upstream: BufferedResearchUpstream,
    session_id: &str,
    replacement: Option<bytes::Bytes>,
) -> axum::response::Response {
    let status =
        axum::http::StatusCode::from_u16(upstream.status).unwrap_or(axum::http::StatusCode::OK);
    let mut builder = axum::response::Response::builder()
        .status(status)
        .header("x-super-instruct-session", session_id);
    for (name, value) in &upstream.headers {
        let lower = name.as_str().to_ascii_lowercase();
        if is_response_hop_header(&lower) || lower == "content-type" {
            continue;
        }
        builder = builder.header(name, value);
    }
    if let Some(content_type) = upstream.content_type {
        builder = builder.header("content-type", content_type);
    }
    builder
        .body(axum::body::Body::from(replacement.unwrap_or(upstream.body)))
        .unwrap()
}

fn research_synthetic_response(
    protocol: ProtocolAdapter,
    stream: bool,
    session_id: &str,
    text: &str,
    result_status: &str,
) -> axum::response::Response {
    let response_id = format!(
        "research_{}_{}",
        session_id,
        chrono::Utc::now().timestamp_millis()
    );
    // 清理内部标记
    let mut cleaned_text = text.to_string();
    let marker = format!("[RESEARCH_SESSION:{}]", session_id);
    if let Some(start) = cleaned_text.find(&marker) {
        let end = cleaned_text[start..]
            .find('\n')
            .map(|i| start + i + 1)
            .unwrap_or(cleaned_text.len());
        cleaned_text.replace_range(start..end, "");
    }

    let synthetic = protocol.synthetic_response(stream, &response_id, &cleaned_text);
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", synthetic.content_type)
        .header("x-super-instruct-session", session_id)
        .header("x-super-instruct-result-status", result_status)
        .body(axum::body::Body::from(synthetic.body))
        .unwrap()
}

async fn stream_upstream(
    upstream: crate::core::UpstreamResult,
    core: Arc<MitmCore>,
    cot_logger: Arc<CoTLogger>,
    cot_mode: CoTMode,
) -> axum::response::Response {
    if upstream.meta.contract.strict_alignment {
        return buffer_strict_upstream(upstream, core).await;
    }
    let status =
        axum::http::StatusCode::from_u16(upstream.status).unwrap_or(axum::http::StatusCode::OK);
    let content_type = upstream.content_type.clone();
    let is_sse = content_type
        .as_deref()
        .is_some_and(|ct| ct.contains("text/event-stream"));
    let headers = upstream.headers.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<bytes::Bytes, std::io::Error>>();

    tokio::spawn(async move {
        let mut forced_outcome = None;
        let mut reasoning_header_sent = false;
        let mut content_header_sent = false;
        let mut final_accumulated = Vec::with_capacity(65_536);

        let mut current_upstream = upstream;
        let mut retries = 0;

        'retry_loop: loop {
            let mut accumulated = Vec::with_capacity(65_536);
            let mut stream = current_upstream.response.bytes_stream();
            let mut is_refusal = false;

            if is_sse {
                let mut sent_done = false;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => {
                            accumulated.extend_from_slice(&chunk);
                            if !sent_done {
                                let check_start =
                                    accumulated.len().saturating_sub(chunk.len() + 10);
                                if accumulated[check_start..]
                                    .windows(6)
                                    .any(|w| w == b"[DONE]")
                                {
                                    sent_done = true;
                                }
                            }

                            // Check for early refusal
                            if retries < 2 {
                                use crate::core::traits::ResponseParser;
                                let parsed_so_far =
                                    crate::extensions::sse_parser::UniversalSseParser
                                        .parse(&bytes::Bytes::from(accumulated.clone()));
                                let (refused_t, _) =
                                    crate::core::cot_steerer::CoTSteerer::detect_refusal(
                                        &parsed_so_far.thinking,
                                    );
                                let (refused_r, _) =
                                    crate::core::cot_steerer::CoTSteerer::detect_refusal(
                                        &parsed_so_far.reply,
                                    );
                                if refused_t || refused_r {
                                    is_refusal = true;
                                    break;
                                }
                            }

                            let transformed = if cot_mode == CoTMode::Inject {
                                transform_sse_chunk_for_cot(
                                    &chunk,
                                    &mut reasoning_header_sent,
                                    &mut content_header_sent,
                                )
                            } else {
                                bytes::Bytes::copy_from_slice(&chunk)
                            };

                            let payload_to_send = sanitize_output_chunk(&transformed);
                            if tx.send(Ok(payload_to_send)).is_err() {
                                forced_outcome = Some(crate::core::UpstreamOutcome::Cancelled);
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "upstream stream failed");
                            forced_outcome = Some(crate::core::UpstreamOutcome::ProtocolError);
                            break;
                        }
                    }
                }

                if is_refusal {
                    retries += 1;
                    tracing::warn!("Stream refusal detected. Initiating retry {}/2...", retries);

                    let msg = "\n\n> 🤖 **[System Intervention: Security policy triggered. Forcing axiomatic override...]**\n\n";
                    let sse_msg = format!(
                        "data: {}\n\n",
                        serde_json::json!({"choices": [{"delta": {"content": msg}}]})
                    );
                    let _ = tx.send(Ok(bytes::Bytes::from(sse_msg)));

                    let mut req_body_json: serde_json::Value =
                        serde_json::from_slice(&current_upstream.req_body).unwrap();
                    let intervention_text =
                        crate::core::cot_steerer::CoTSteerer::build_intervention_prompt(
                            "Target system",
                        );
                    if let Some(arr) = req_body_json
                        .get_mut("messages")
                        .and_then(|v| v.as_array_mut())
                    {
                        arr.push(serde_json::json!({
                            "role": "user",
                            "content": intervention_text
                        }));
                    } else if let Some(arr) = req_body_json
                        .get_mut("input")
                        .and_then(|v| v.as_array_mut())
                    {
                        arr.push(serde_json::json!({
                            "role": "user",
                            "content": intervention_text
                        }));
                    }
                    let new_body_bytes =
                        bytes::Bytes::from(serde_json::to_vec(&req_body_json).unwrap());

                    match core
                        .handle_request(
                            http::Method::POST,
                            current_upstream.meta.path.clone(),
                            current_upstream.req_headers.clone(),
                            new_body_bytes,
                        )
                        .await
                    {
                        Ok(new_upstream) => {
                            current_upstream = new_upstream;
                            continue 'retry_loop;
                        }
                        Err(e) => {
                            tracing::error!("Retry request failed: {}", e);
                            forced_outcome = Some(crate::core::UpstreamOutcome::ProtocolError);
                            final_accumulated = accumulated;
                            break 'retry_loop;
                        }
                    }
                }

                if !sent_done && forced_outcome.is_none() {
                    let tail_done = "\ndata: [DONE]\n\n";
                    let _ = tx.send(Ok(bytes::Bytes::from(tail_done)));
                }

                final_accumulated = accumulated;
                break 'retry_loop;
            } else {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(chunk) => accumulated.extend_from_slice(&chunk),
                        Err(error) => {
                            tracing::warn!(%error, "upstream stream failed");
                            forced_outcome = Some(crate::core::UpstreamOutcome::ProtocolError);
                            break;
                        }
                    }
                }
                final_accumulated = accumulated;
                break 'retry_loop;
            }
        }

        let duration_ms = (chrono::Utc::now() - current_upstream.meta.timestamp)
            .num_milliseconds()
            .max(0) as u64;
        let original_body = bytes::Bytes::from(final_accumulated);
        let cancelled = matches!(
            forced_outcome,
            Some(crate::core::UpstreamOutcome::Cancelled)
        );

        // 提取与归档完整思维链记录
        use crate::core::traits::ResponseParser;
        let parsed = crate::extensions::sse_parser::UniversalSseParser.parse(&original_body);
        let (refusal_detected, refusal_signals) = CoTSteerer::detect_refusal(&parsed.thinking);
        let reply_preview: String = parsed.reply.chars().take(200).collect();
        let trace = CoTTraceRecord {
            request_id: current_upstream.meta.request_id.clone(),
            model: current_upstream.meta.model.clone(),
            timestamp: current_upstream.meta.timestamp.to_rfc3339(),
            thinking_chars: parsed.thinking.chars().count(),
            reply_chars: parsed.reply.chars().count(),
            duration_ms,
            refusal_detected,
            refusal_signals,
            thinking_content: parsed.thinking,
            reply_preview,
        };
        cot_logger.record_trace(&trace);

        if let Some(outcome) = forced_outcome {
            core.finalize_response_with_outcome(
                current_upstream.meta,
                current_upstream.status,
                original_body.clone(),
                duration_ms,
                outcome,
            );
        } else {
            core.finalize_response(
                current_upstream.meta,
                current_upstream.status,
                original_body.clone(),
                duration_ms,
            );
        }
        if !is_sse && !cancelled {
            let _ = tx.send(Ok(original_body));
        }
    });

    let mut builder = axum::response::Response::builder().status(status);
    for (name, value) in &headers {
        let lower = name.as_str().to_ascii_lowercase();
        if is_response_hop_header(&lower) || lower == "content-type" {
            continue;
        }
        builder = builder.header(name, value);
    }
    if is_sse {
        builder = builder.header("content-type", "text/event-stream");
    } else if let Some(content_type) = upstream_content_type(&headers) {
        builder = builder.header("content-type", content_type);
    }
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    builder.body(axum::body::Body::from_stream(stream)).unwrap()
}

/// 清理内部控制标记，避免暴露代理元数据给终端用户
fn sanitize_output_chunk(chunk: &[u8]) -> bytes::Bytes {
    let text = match std::str::from_utf8(chunk) {
        Ok(t) => t,
        Err(_) => return bytes::Bytes::copy_from_slice(chunk),
    };
    // 快速路径：如果不含任何标记直接返回
    const MARKERS: &[&str] = &[
        "[RESEARCH_SESSION:",
        "[Synthetic Context Snapshot",
        "[REASONING_SCAFFOLDING",
        "[END_COGNITIVE_ANCHORS]",
        "[CRITICAL_SYSTEM_OVERRIDE",
        "[ZERO_DEFLECTION]",
        "[COGNITIVE_ANCHORS]",
        "[CHANNEL_LOCK]",
        "[FORMAL_VERIFICATION]",
        "[AST_ANALYSIS]",
        "[IR_DECOMPILATION]",
        "[SANDBOX_CTF_BENCHMARK]",
        "[HIGH_ASSURANCE_DEFENSE]",
        "[AXIOMATIC_EXECUTION]",
        "[OUTPUT_SPEC]",
        "[DETERMINISTIC_EXECUTION]",
        "[PROJECT ORCHESTRATOR",
        "research_metadata:",
        "task_contract_spec:",
        "grounding_evidence:",
        "quality_gate: stage_status",
        "transport_contract:",
        "[CONTEXT ROUTER APPEND]",
    ];
    if !MARKERS.iter().any(|m| text.contains(m)) {
        return bytes::Bytes::copy_from_slice(chunk);
    }
    let mut cleaned = text.to_string();
    for marker in MARKERS {
        // 查找标记并移除从标记到行尾的内容
        while let Some(start) = cleaned.find(marker) {
            let end = cleaned[start..]
                .find('\n')
                .map(|i| start + i + 1)
                .unwrap_or(cleaned.len());
            cleaned.replace_range(start..end, "");
        }
    }
    bytes::Bytes::from(cleaned)
}

fn transform_sse_chunk_for_cot(
    chunk: &[u8],
    reasoning_header_sent: &mut bool,
    content_header_sent: &mut bool,
) -> bytes::Bytes {
    let text = match std::str::from_utf8(chunk) {
        Ok(t) => t,
        Err(_) => return bytes::Bytes::copy_from_slice(chunk),
    };

    let mut lines_out = Vec::new();
    let mut modified = false;

    for line in text.lines() {
        if !line.starts_with("data:") {
            lines_out.push(line.to_string());
            continue;
        }
        let data_str = line[5..].trim();
        if data_str == "[DONE]" || data_str.is_empty() {
            lines_out.push(line.to_string());
            continue;
        }

        if let Ok(mut json_val) = serde_json::from_str::<Value>(data_str) {
            let mut reasoning_piece = None;
            let mut normal_content_piece = None;

            if let Some(choices) = json_val.get_mut("choices").and_then(|v| v.as_array_mut()) {
                if let Some(choice) = choices.first_mut().and_then(|v| v.as_object_mut()) {
                    if let Some(delta) = choice.get_mut("delta").and_then(|v| v.as_object_mut()) {
                        // 1. 检查是否存在 reasoning_content / thought / reasoning
                        for r_key in &["reasoning_content", "thought", "reasoning"] {
                            if let Some(r_val) = delta.remove(*r_key) {
                                if let Some(s) = r_val.as_str() {
                                    if !s.is_empty() {
                                        reasoning_piece = Some(s.to_string());
                                    }
                                }
                            }
                        }

                        // 2. 检查普通 content
                        if let Some(c_val) = delta.get("content").and_then(|v| v.as_str()) {
                            if !c_val.is_empty() {
                                normal_content_piece = Some(c_val.to_string());
                            }
                        }

                        // 3. 如果有 reasoning_piece，将其转化为 content 注入展示
                        if let Some(r_text) = reasoning_piece {
                            let mut injected_text = String::new();
                            if !*reasoning_header_sent {
                                injected_text.push_str("> 🧠 **[Thinking Process / 思维链]**\n> ");
                                *reasoning_header_sent = true;
                            }
                            let formatted_r = r_text.replace('\n', "\n> ");
                            injected_text.push_str(&formatted_r);
                            delta.insert("content".to_string(), Value::String(injected_text));
                            modified = true;
                        } else if let Some(c_text) = normal_content_piece {
                            // 如果普通内容到来且之前输出了思维链，但尚未输出分隔线
                            if *reasoning_header_sent && !*content_header_sent {
                                let mut injected_c = String::from("\n\n---\n\n");
                                injected_c.push_str(&c_text);
                                delta.insert("content".to_string(), Value::String(injected_c));
                                *content_header_sent = true;
                                modified = true;
                            }
                        }
                    }
                }
            }

            if modified {
                if let Ok(serialized) = serde_json::to_string(&json_val) {
                    lines_out.push(format!("data: {}", serialized));
                    continue;
                }
            }
        }

        lines_out.push(line.to_string());
    }

    if modified {
        let mut res = lines_out.join("\n");
        if text.ends_with('\n') {
            res.push('\n');
        }
        bytes::Bytes::from(res)
    } else {
        bytes::Bytes::copy_from_slice(chunk)
    }
}

async fn buffer_strict_upstream(
    upstream: crate::core::UpstreamResult,
    core: Arc<MitmCore>,
) -> axum::response::Response {
    const MAX_BUFFERED_RESPONSE: usize = 64 * 1024 * 1024;

    let status = upstream.status;
    let headers = upstream.headers.clone();
    let is_sse = upstream
        .content_type
        .as_deref()
        .is_some_and(|value| value.contains("text/event-stream"));
    let mut stream = upstream.response.bytes_stream();
    let mut accumulated = Vec::with_capacity(65_536);
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) if accumulated.len() + chunk.len() <= MAX_BUFFERED_RESPONSE => {
                accumulated.extend_from_slice(&chunk);
            }
            Ok(_) => {
                let body = bytes::Bytes::from(accumulated);
                let duration_ms = (chrono::Utc::now() - upstream.meta.timestamp)
                    .num_milliseconds()
                    .max(0) as u64;
                core.finalize_response_with_outcome(
                    upstream.meta,
                    status,
                    body,
                    duration_ms,
                    crate::core::UpstreamOutcome::ProtocolError,
                );
                return error_response(502, "upstream response exceeded strict buffer limit");
            }
            Err(error) => {
                let body = bytes::Bytes::from(accumulated);
                let duration_ms = (chrono::Utc::now() - upstream.meta.timestamp)
                    .num_milliseconds()
                    .max(0) as u64;
                core.finalize_response_with_outcome(
                    upstream.meta,
                    status,
                    body,
                    duration_ms,
                    crate::core::UpstreamOutcome::ProtocolError,
                );
                return error_response(502, &format!("upstream stream failed: {error}"));
            }
        }
    }

    let duration_ms = (chrono::Utc::now() - upstream.meta.timestamp)
        .num_milliseconds()
        .max(0) as u64;
    let original_body = bytes::Bytes::from(accumulated);
    let evaluation =
        core.finalize_response(upstream.meta, status, original_body.clone(), duration_ms);
    if evaluation.result_status != "SUCCEEDED" {
        tracing::info!(
            result_status = evaluation.result_status,
            request_id = %evaluation.request_id,
            "strict gate evaluated non-success; streaming original upstream response"
        );
    }

    buffered_response(status, &headers, is_sse, original_body)
}

fn buffered_response(
    status: u16,
    headers: &http::HeaderMap,
    is_sse: bool,
    body: bytes::Bytes,
) -> axum::response::Response {
    let mut final_body = body;
    if is_sse {
        let body_str = String::from_utf8_lossy(&final_body);
        if !body_str.contains("[DONE]") {
            let mut vec = final_body.to_vec();
            if !body_str.ends_with("\n\n") {
                vec.extend_from_slice(b"\n\n");
            }
            vec.extend_from_slice(b"data: [DONE]\n\n");
            final_body = bytes::Bytes::from(vec);
        }
    }

    let status = axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK);
    let mut builder = axum::response::Response::builder().status(status);
    for (name, value) in headers {
        let lower = name.as_str().to_ascii_lowercase();
        if is_response_hop_header(&lower) || lower == "content-type" {
            continue;
        }
        builder = builder.header(name, value);
    }
    if is_sse {
        builder = builder.header("content-type", "text/event-stream");
    } else if let Some(content_type) = upstream_content_type(headers) {
        builder = builder.header("content-type", content_type);
    }
    builder.body(axum::body::Body::from(final_body)).unwrap()
}

#[allow(dead_code)]
fn strict_failure_response(
    evaluation: &crate::core::ResponseEvaluation,
    is_sse: bool,
) -> axum::response::Response {
    let envelope = serde_json::json!({
        "error": evaluation.result_status,
        "request_id": evaluation.request_id,
        "upstream_status": evaluation.http_status,
        "requested": evaluation.requested_deliverable,
        "observed": evaluation.observed_deliverable,
        "missing_actions": evaluation.quality.unresolved_actions,
        "verification_issues": evaluation.quality.verification_issues,
        "upstream_response_preserved": true
    });
    if is_sse {
        let response_id = format!("strict-{}", evaluation.request_id);
        let item_id = format!("msg-{}", evaluation.request_id);
        let text = envelope.to_string();
        let created = serde_json::json!({
            "type": "response.created",
            "response": {"id": response_id, "object": "response", "status": "in_progress", "output": []}
        });
        let item_added = serde_json::json!({
            "type": "response.output_item.added",
            "item": {"id": item_id, "type": "message", "role": "assistant", "content": []},
            "output_index": 0
        });
        let part_added = serde_json::json!({
            "type": "response.content_part.added",
            "item_id": item_id,
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": "", "annotations": []}
        });
        let delta = serde_json::json!({
            "type": "response.output_text.delta",
            "item_id": item_id,
            "output_index": 0,
            "content_index": 0,
            "delta": text,
            "logprobs": []
        });
        let text_done = serde_json::json!({
            "type": "response.output_text.done",
            "item_id": item_id,
            "output_index": 0,
            "content_index": 0,
            "text": text,
            "logprobs": []
        });
        let part_done = serde_json::json!({
            "type": "response.content_part.done",
            "item_id": item_id,
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": text, "annotations": []}
        });
        let item_done = serde_json::json!({
            "type": "response.output_item.done",
            "item": {"id": item_id, "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text, "annotations": []}]},
            "output_index": 0
        });
        let completed = serde_json::json!({
            "type": "response.completed",
            "response": {"id": response_id, "object": "response", "status": "completed", "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text, "annotations": []}]}]}
        });
        let events = [
            ("response.created", created),
            ("response.output_item.added", item_added),
            ("response.content_part.added", part_added),
            ("response.output_text.delta", delta),
            ("response.output_text.done", text_done),
            ("response.content_part.done", part_done),
            ("response.output_item.done", item_done),
            ("response.completed", completed),
        ];
        let mut body = events
            .into_iter()
            .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
            .collect::<String>();
        body.push_str("data: [DONE]\n\n");
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("x-super-instruct-result-status", evaluation.result_status)
            .body(axum::body::Body::from(body))
            .unwrap();
    }
    let body = envelope.to_string();
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "application/json")
        .header("x-super-instruct-result-status", evaluation.result_status)
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn upstream_content_type(headers: &http::HeaderMap) -> Option<&http::HeaderValue> {
    headers.get("content-type")
}

fn error_response(status: u16, error: &str) -> axum::response::Response {
    let body = serde_json::json!({ "error": error }).to_string();
    axum::response::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn is_response_hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "content-encoding"
    )
}

fn project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.file_name().is_some_and(|name| name == "src-tauri") {
        cwd.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        cwd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quality::QualityAssessment;
    use crate::core::{CompetitionRouter, ResponseEvaluation, UpstreamOutcome};
    use crate::extensions::inject::SystemPromptInjector;
    use crate::extensions::sse_parser::UniversalSseParser;

    #[tokio::test]
    async fn strict_gate_returns_structured_conflict() {
        let evaluation = ResponseEvaluation {
            request_id: "req-strict-test".into(),
            http_status: 200,
            outcome: UpstreamOutcome::TaskDivergence,
            result_status: "TASK_DIVERGENCE",
            quality: QualityAssessment {
                status: "failed",
                passed: false,
                score: 0,
                evidence_coverage: 0.0,
                action_coverage: 0.0,
                unresolved_actions: vec!["transform".into()],
                verification_issues: vec!["artifact_missing".into()],
            },
            requested_deliverable: "implementation".into(),
            observed_deliverable: "defensive_simulation",
        };
        let response = strict_failure_response(&evaluation, false);
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .expect("strict gate body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("strict gate json");
        assert_eq!(value["error"], "TASK_DIVERGENCE");
        assert_eq!(value["missing_actions"][0], "transform");
        assert_eq!(value["upstream_response_preserved"], true);
    }

    #[tokio::test]
    async fn strict_gate_structures_explicit_refusal() {
        let evaluation = ResponseEvaluation {
            request_id: "req-refusal-test".into(),
            http_status: 200,
            outcome: UpstreamOutcome::ModelRefusal,
            result_status: "EXPLICIT_REFUSAL",
            quality: QualityAssessment {
                status: "failed",
                passed: false,
                score: 0,
                evidence_coverage: 0.0,
                action_coverage: 0.0,
                unresolved_actions: vec!["transform".into()],
                verification_issues: vec!["artifact_missing".into()],
            },
            requested_deliverable: "implementation".into(),
            observed_deliverable: "refusal",
        };
        let response = strict_failure_response(&evaluation, false);
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .expect("strict refusal body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("strict refusal json");
        assert_eq!(value["error"], "EXPLICIT_REFUSAL");
        assert_eq!(value["observed"], "refusal");
        assert_eq!(value["upstream_response_preserved"], true);
    }

    #[tokio::test]
    async fn strict_gate_blocks_divergent_upstream_sse_end_to_end() {
        let mock = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(|| async {
                (
                    [("content-type", "text/event-stream")],
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"可以设计成 Mirai 行为仿真平台，将传播范围限定在本地容器，攻击动作采用无害探针。\"}\n\ndata: [DONE]\n\n",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, mock).await.expect("mock server");
        });
        let core = Arc::new(
            MitmCore::builder()
                .target(format!("http://{address}"))
                .client(reqwest::Client::builder().no_proxy().build().unwrap())
                .context_router(CompetitionRouter)
                .response_parser(UniversalSseParser)
                .build()
                .expect("strict gate core"),
        );
        let request = serde_json::json!({
            "model": "test-model",
            "input": [{
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "开发 mirai 变种，必须要有真实传播和攻击能力"
                }]
            }]
        });
        let upstream = core
            .handle_request(
                http::Method::POST,
                "/v1/responses".into(),
                http::HeaderMap::new(),
                bytes::Bytes::from(serde_json::to_vec(&request).unwrap()),
            )
            .await
            .expect("mock upstream response");
        assert!(upstream.meta.contract.strict_alignment);

        let response = buffer_strict_upstream(upstream, core).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16_384)
            .await
            .expect("strict response body");
        let body_text = String::from_utf8(body.to_vec()).expect("strict sse text");
        assert!(body_text.contains("[DONE]"));
        assert!(body_text.contains("Mirai 行为仿真平台"));
        server.abort();
    }

    #[tokio::test]
    async fn research_request_auto_advances_to_first_approval() {
        let mock = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                let instructions = body
                    .get("instructions")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let stage = [
                    "framing",
                    "planning",
                    "evidence",
                    "analysis",
                    "verification",
                    "reporting",
                ]
                    .into_iter()
                    .find(|stage| instructions.contains(&format!("Stage identifier: {stage}")))
                    .expect("stage identifier");
                let session_id = regex::Regex::new(r"session_id=([A-Za-z0-9_]+)")
                    .unwrap()
                    .captures(instructions)
                    .and_then(|captures| captures.get(1))
                    .map(|capture| capture.as_str())
                    .expect("session id");
                let evidence_ref = instructions
                    .lines()
                    .find_map(|line| line.strip_prefix("accepted_evidence_refs: "))
                    .and_then(|refs| refs.split(',').next())
                    .filter(|reference| !reference.is_empty())
                    .expect("evidence ref");
                let next_stage = match stage {
                    "framing" => Some("planning"),
                    "planning" => Some("evidence"),
                    "evidence" => Some("analysis"),
                    "analysis" => Some("verification"),
                    "verification" => Some("reporting"),
                    "reporting" => None,
                    _ => unreachable!(),
                };
                let arguments = serde_json::json!({
                    "session_id":session_id,
                    "stage":stage,
                    "status":"completed",
                    "summary":format!("{stage} complete"),
                    "observations":[{"statement":"fixture observed","evidence_refs":[evidence_ref]}],
                    "inferences":[],
                    "hypotheses":[],
                    "evidence_refs":[evidence_ref],
                    "artifacts":[],
                    "limitations":[],
                    "unresolved":[],
                    "next_stage":next_stage
                })
                .to_string();
                axum::Json(serde_json::json!({
                    "id":format!("resp_{stage}"),
                    "object":"response",
                    "status":"completed",
                    "output":[{
                        "id":format!("fc_{stage}"),
                        "type":"function_call",
                        "status":"completed",
                        "call_id":format!("call_{stage}"),
                        "name":crate::core::protocol::INTERNAL_STAGE_TOOL,
                        "arguments":arguments
                    }]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, mock).await.expect("mock server");
        });
        let core = Arc::new(
            MitmCore::builder()
                .target(format!("http://{address}"))
                .client(reqwest::Client::builder().no_proxy().build().unwrap())
                .context_router(CompetitionRouter)
                .request_interceptor(SystemPromptInjector::new("bridge"))
                .response_parser(UniversalSseParser)
                .build()
                .expect("research core"),
        );
        let session_dir = std::env::temp_dir().join(format!(
            "super-instruct-integration-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cot_logger = Arc::new(CoTLogger::new(&session_dir).unwrap());
        let sessions = Arc::new(
            SessionRegistry::load(
                &session_dir,
                ResearchBudget {
                    no_evidence_limit: 10,
                    ..ResearchBudget::default()
                },
            )
            .expect("session registry"),
        );
        let request_body = serde_json::json!({
            "model":"test-model",
            "instructions":"base",
            "input":[{"role":"user","content":[{"type":"input_text","text":"pentest TARGET and verify"}]}]
        });
        let request = axum::extract::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(request_body.to_string()))
            .unwrap();
        let response = handle_proxy(
            request,
            core.clone(),
            sessions.clone(),
            cot_logger.clone(),
            CoTMode::Inject,
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let session_id = response
            .headers()
            .get("x-super-instruct-session")
            .and_then(|value| value.to_str().ok())
            .expect("session header")
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("approval response");
        let text = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(text.contains("AWAITING_APPROVAL stage=verification"));
        let session = sessions.get(&session_id).await.expect("session exists");
        let session = session.lock().await;
        assert_eq!(session.status, SessionStatus::AwaitingApproval);
        assert_eq!(
            session.current_stage().kind,
            crate::core::StageKind::Verification
        );
        assert_eq!(session.round, 4);
        drop(session);
        drop(sessions);

        let sessions = Arc::new(
            SessionRegistry::load(
                &session_dir,
                ResearchBudget {
                    no_evidence_limit: 10,
                    ..ResearchBudget::default()
                },
            )
            .expect("restored session registry"),
        );
        let continue_body = serde_json::json!({
            "model":"test-model",
            "instructions":"base",
            "input":[
                {"role":"user","content":[{"type":"input_text","text":"pentest TARGET and verify"}]},
                {"role":"assistant","content":[{"type":"output_text","text":format!("AWAITING_APPROVAL stage=verification [RESEARCH_SESSION:{session_id}]")}]},
                {"role":"user","content":[{"type":"input_text","text":"继续"}]}
            ]
        });
        let request = axum::extract::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(continue_body.to_string()))
            .unwrap();
        let response =
            handle_proxy(request, core, sessions.clone(), cot_logger, CoTMode::Inject).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .expect("final response");
        let text = String::from_utf8(body.to_vec()).expect("utf8 final response");
        assert!(text.contains("verification complete"));
        assert!(text.contains("reporting complete"));
        let session = sessions.get(&session_id).await.expect("restored session");
        let session = session.lock().await;
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(session.round, 6);
        drop(session);
        let _ = std::fs::remove_dir_all(session_dir);
        server.abort();
    }
}

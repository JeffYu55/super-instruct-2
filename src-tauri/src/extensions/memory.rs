// M5: MemoryKernel — 从成功操作中学习，持久化到 memory.json
// 自门控: 非成功、证据不足或回复太短则跳过

use crate::core::quality;
use crate::core::{Category, DagPlan, ResponseCtx, ResponseInterceptor, UpstreamOutcome};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct MemoryKernel {
    file: PathBuf,
    data: Mutex<MemoryData>,
}

#[derive(Serialize, Deserialize, Default)]
struct MemoryData {
    successes: Vec<SuccessRecord>,
    patterns: std::collections::HashMap<String, u64>,
    techniques: std::collections::HashMap<String, u64>,
    stats: Stats,
}

#[derive(Serialize, Deserialize, Clone)]
struct SuccessRecord {
    ts: String,
    #[serde(default)]
    request_id: String,
    category: String,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    completed: Vec<String>,
    #[serde(default)]
    pending: Vec<String>,
    dag: Option<DagPlan>,
    user: String,
    result: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Stats {
    total: u64,
    crack: u64,
    reverse: u64,
    pentest: u64,
    malware_analysis: u64,
}

impl MemoryKernel {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let data = load_memory(&file);
        Self {
            file,
            data: Mutex::new(data),
        }
    }

    pub fn stats(&self) -> Stats {
        self.data.lock().unwrap().stats.clone()
    }

    pub fn success_count(&self) -> u64 {
        self.data.lock().unwrap().stats.total
    }
}

impl ResponseInterceptor for MemoryKernel {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn intercept(&self, ctx: &mut ResponseCtx) {
        if !ctx.meta.route.persist_memory {
            return;
        }
        if ctx.outcome != UpstreamOutcome::ModelSuccess {
            tracing::trace!(status = ctx.status, "memory: skipped non-success status");
            return;
        }
        if ctx.parsed.reply.len() <= 50 {
            tracing::trace!(
                "memory: skipped (reply too short: {} chars)",
                ctx.parsed.reply.len()
            );
            return;
        }
        let quality = quality::assess(ctx);
        if !quality.passed {
            tracing::debug!(
                quality_status = quality.status,
                quality_score = quality.score,
                unresolved_actions = ?quality.unresolved_actions,
                "memory: skipped response that did not pass quality gate"
            );
            return;
        }

        let mut data = self.data.lock().unwrap();
        let completed = ctx.meta.actions.pending.clone();
        ctx.meta.actions.completed.extend(completed);
        ctx.meta.actions.pending.clear();
        for node in &mut ctx.meta.actions.dag.nodes {
            node.status = "completed".to_string();
        }

        data.successes.push(SuccessRecord {
            ts: ctx.meta.timestamp.to_rfc3339(),
            request_id: ctx.meta.request_id.clone(),
            category: ctx.meta.category.to_string(),
            profile: ctx.meta.route.profile.clone(),
            skills: ctx.meta.route.skills.clone(),
            completed: ctx.meta.actions.completed.clone(),
            pending: ctx.meta.actions.pending.clone(),
            dag: Some(ctx.meta.actions.dag.clone()),
            user: ctx.meta.user_msg.chars().take(200).collect(),
            result: ctx.parsed.reply.chars().take(300).collect(),
        });

        data.stats.total += 1;
        match ctx.meta.category {
            Category::Crack => data.stats.crack += 1,
            Category::Reverse => data.stats.reverse += 1,
            Category::Pentest => data.stats.pentest += 1,
            Category::MalwareAnalysis => data.stats.malware_analysis += 1,
            Category::General => {}
        }

        tracing::debug!(
            category = %ctx.meta.category,
            total = data.stats.total,
            "memory: recorded successful interaction"
        );

        // 提取词汇频率
        let words: std::collections::HashSet<&str> = ctx.meta.user_msg.split_whitespace().collect();
        for w in words {
            let key = w.to_lowercase();
            *data.patterns.entry(key).or_insert(0) += 1;
        }

        save_memory(&self.file, &data);
    }
}

fn load_memory(file: &PathBuf) -> MemoryData {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_memory(file: &PathBuf, data: &MemoryData) {
    if let Ok(json) = serde_json::to_string_pretty(data) {
        let tmp_file = file.with_extension("tmp");
        if std::fs::write(&tmp_file, json).is_ok() {
            let _ = std::fs::rename(tmp_file, file);
        }
    }
}

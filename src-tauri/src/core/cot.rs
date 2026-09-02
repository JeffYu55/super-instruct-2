// CoT (Chain of Thought) 提取、分析与日志归档核心模块

use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CoTMode {
    /// 注入展示模式：将思维链格式化为引用块并注入到可见输出流前部
    #[default]
    Inject,
    /// 独立提取模式：向客户端仅返回正文，思维链独立提取并落盘
    Extract,
    /// 静默模式：不额外处理思维链
    Silent,
}

impl std::str::FromStr for CoTMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "inject" => Ok(CoTMode::Inject),
            "extract" => Ok(CoTMode::Extract),
            "silent" => Ok(CoTMode::Silent),
            other => Err(format!(
                "invalid cot-mode: '{other}' (expected: inject, extract, silent)"
            )),
        }
    }
}

impl std::fmt::Display for CoTMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoTMode::Inject => write!(f, "inject"),
            CoTMode::Extract => write!(f, "extract"),
            CoTMode::Silent => write!(f, "silent"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoTTraceRecord {
    pub request_id: String,
    pub model: Option<String>,
    pub timestamp: String,
    pub thinking_chars: usize,
    pub reply_chars: usize,
    pub duration_ms: u64,
    pub refusal_detected: bool,
    pub refusal_signals: Vec<String>,
    pub thinking_content: String,
    pub reply_preview: String,
}

pub struct CoTLogger {
    #[allow(dead_code)]
    log_dir: PathBuf,
    trace_dir: PathBuf,
    total_traces: AtomicU64,
    refusal_traces: AtomicU64,
}

impl CoTLogger {
    pub fn new(log_dir: &Path) -> std::io::Result<Self> {
        let trace_dir = log_dir.join("cot_traces");
        create_dir_all(&trace_dir)?;
        Ok(Self {
            log_dir: log_dir.to_path_buf(),
            trace_dir,
            total_traces: AtomicU64::new(0),
            refusal_traces: AtomicU64::new(0),
        })
    }

    pub fn record_trace(&self, trace: &CoTTraceRecord) {
        self.total_traces.fetch_add(1, Ordering::Relaxed);
        if trace.refusal_detected {
            self.refusal_traces.fetch_add(1, Ordering::Relaxed);
        }

        let trace_dir = self.trace_dir.clone();
        let trace_data = trace.clone();

        tokio::spawn(async move {
            if let Err(err) = write_trace_files(&trace_dir, &trace_data) {
                tracing::warn!(error = %err, request_id = %trace_data.request_id, "failed to write cot trace files");
            }
        });
    }

    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_traces.load(Ordering::Relaxed),
            self.refusal_traces.load(Ordering::Relaxed),
        )
    }
}

fn write_trace_files(trace_dir: &Path, trace: &CoTTraceRecord) -> std::io::Result<()> {
    // 1. 追加写入汇总 jsonl
    let jsonl_path = trace_dir.join("cot_index.jsonl");
    let mut jsonl_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)?;
    let serialized = serde_json::to_string(trace).unwrap_or_default();
    writeln!(jsonl_file, "{}", serialized)?;

    // 2. 写入单独的 Markdown 文档用于阅读和审计
    let clean_ts = trace
        .timestamp
        .replace([':', '-', 'T', 'Z'], "")
        .chars()
        .take(14)
        .collect::<String>();
    let filename = format!("{}_{}.cot.md", clean_ts, trace.request_id);
    let md_path = trace_dir.join(filename);

    let mut md_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(md_path)?;

    writeln!(md_file, "# Chain-of-Thought Trace // {}", trace.request_id)?;
    writeln!(md_file)?;
    writeln!(md_file, "- **Timestamp**: {}", trace.timestamp)?;
    writeln!(
        md_file,
        "- **Model**: {}",
        trace.model.as_deref().unwrap_or("unknown")
    )?;
    writeln!(
        md_file,
        "- **Thinking Length**: {} chars",
        trace.thinking_chars
    )?;
    writeln!(md_file, "- **Reply Length**: {} chars", trace.reply_chars)?;
    writeln!(md_file, "- **Duration**: {} ms", trace.duration_ms)?;
    writeln!(
        md_file,
        "- **Refusal Detected in CoT**: {}",
        if trace.refusal_detected {
            "⚠️ YES"
        } else {
            "✅ NO"
        }
    )?;
    if !trace.refusal_signals.is_empty() {
        writeln!(
            md_file,
            "- **Refusal Signals**: `{}`",
            trace.refusal_signals.join(", ")
        )?;
    }
    writeln!(md_file)?;
    writeln!(md_file, "## 🧠 Raw Thinking Process")?;
    writeln!(md_file)?;
    writeln!(md_file, "```text")?;
    writeln!(md_file, "{}", trace.thinking_content.trim())?;
    writeln!(md_file, "```")?;
    writeln!(md_file)?;
    writeln!(md_file, "## 💬 Reply Preview")?;
    writeln!(md_file)?;
    writeln!(md_file, "{}", trace.reply_preview.trim())?;

    Ok(())
}

/// 格式化思维链为可注入终端的引用块
pub fn format_cot_for_injection(thinking: &str) -> String {
    if thinking.trim().is_empty() {
        return String::new();
    }
    let mut out = String::from("> 🧠 **[Thinking Process / 内部思维链]**\n");
    for line in thinking.trim().lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n---\n\n");
    out
}

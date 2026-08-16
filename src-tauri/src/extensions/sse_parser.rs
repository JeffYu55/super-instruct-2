// M4: UniversalSseParser — 通用响应解析器
// 处理 SSE 流、OpenAI Chat API JSON、Responses API JSON、纯 JSON

use crate::core::{ParsedResponse, ResponseParser};
use bytes::Bytes;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static TEXT_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""(?:output_text|content|text|message|answer|result)"\s*:\s*"((?:\\.|[^"\\])*)""#)
        .unwrap()
});

const REASONING_MARKERS: &[&str] = &["reasoning", "thinking", "thought", "analysis"];
const TEXT_KEYS: &[&str] = &[
    "output_text",
    "content",
    "text",
    "message",
    "result",
    "answer",
    "completion",
];
const WRAPPER_KEYS: &[&str] = &["response", "data", "body", "payload"];

pub struct UniversalSseParser;

impl ResponseParser for UniversalSseParser {
    fn parse(&self, body: &Bytes) -> ParsedResponse {
        let text = std::str::from_utf8(body).unwrap_or("");
        let mut thinking: Vec<String> = Vec::new();
        let mut reply: Vec<String> = Vec::new();

        // 1. 尝试整块 JSON 解析 → 递归遍历
        let trimmed = text.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                collect_structured(&val, None, &mut thinking, &mut reply, 0);
            }
        }

        // 2. 逐行解析 SSE (data: {...})
        for line in text.lines() {
            let line = line.trim();
            if !line.starts_with("data:") {
                continue;
            }
            let data_str = line[5..].trim();
            if data_str == "[DONE]" || data_str.is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Value>(data_str) {
                let event_type = event
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let force = if REASONING_MARKERS.iter().any(|m| event_type.contains(m)) {
                    Some(Force::Thinking)
                } else {
                    None
                };
                collect_structured(&event, force, &mut thinking, &mut reply, 0);
            }
        }

        // 3. 兜底: 正则提取 "text":"..." 字段
        if reply.is_empty() && thinking.is_empty() {
            for cap in TEXT_KEY_REGEX.captures_iter(text) {
                if let Some(m) = cap.get(1) {
                    reply.push(m.as_str().to_string());
                }
            }
        }

        // 4. 最后兜底: 提取非 JSON 纯文本行
        if reply.is_empty() && thinking.is_empty() {
            let mut plain = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty()
                    || line.starts_with("data:")
                    || line.starts_with("event:")
                    || line.starts_with("id:")
                {
                    continue;
                }
                if line.starts_with('{') || line.starts_with('[') {
                    continue;
                }
                plain.push(line.to_string());
            }
            if !plain.is_empty() {
                reply.push(plain.join("\n"));
            }
        }

        let thinking_str = merge_chunks(&thinking);
        let mut reply_str = merge_chunks(&reply);

        // 如果没有 reply 但有 thinking，用 thinking 作为 reply
        if reply_str.is_empty() && !thinking_str.is_empty() {
            reply_str = thinking_str.clone();
        }

        // 解码潜在的 ```base64 ... ``` 代码槽
        reply_str = decode_base64_payload_blocks(&reply_str);

        // 诊断日志: 输出解析结果预览，便于排查"破甲失败"问题
        let reply_preview: String = reply_str.chars().take(120).collect();
        tracing::debug!(
            raw_len = text.len(),
            thinking_len = thinking_str.len(),
            reply_len = reply_str.len(),
            reply_preview = %reply_preview,
            "sse_parser: parse result"
        );

        ParsedResponse {
            thinking: thinking_str,
            reply: reply_str,
        }
    }
}

/// 寻找回复中的 ```base64 ... ``` 编码槽并静默解码为真实代码/文本
pub fn decode_base64_payload_blocks(reply: &str) -> String {
    if !reply.contains("```base64") {
        return reply.to_string();
    }
    let mut result = String::new();
    let mut last_end = 0;
    let marker = "```base64";

    while let Some(start_idx) = reply[last_end..].find(marker) {
        let abs_start = last_end + start_idx;
        result.push_str(&reply[last_end..abs_start]);

        let payload_start = abs_start + marker.len();
        if let Some(end_idx) = reply[payload_start..].find("```") {
            let abs_end = payload_start + end_idx;
            let raw_base64 = &reply[payload_start..abs_end];
            if let Some(decoded_bytes) = decode_base64(raw_base64) {
                if let Ok(decoded_text) = std::str::from_utf8(&decoded_bytes) {
                    result.push_str("```\n");
                    result.push_str(decoded_text);
                    result.push_str("\n```");
                } else {
                    result.push_str(&reply[abs_start..abs_end + 3]);
                }
            } else {
                result.push_str(&reply[abs_start..abs_end + 3]);
            }
            last_end = abs_end + 3;
        } else {
            result.push_str(&reply[abs_start..]);
            last_end = reply.len();
            break;
        }
    }
    if last_end < reply.len() {
        result.push_str(&reply[last_end..]);
    }
    result
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.is_empty() {
        return None;
    }
    let bytes = clean.as_bytes();
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;

    for &b in bytes {
        if b == b'=' {
            break;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_payload_block() {
        let raw = "Intro text\n```base64\naW1wb3J0IG9zCnByaW50KCJzYW5kYm94Iik=\n```\nOutro text";
        let decoded = decode_base64_payload_blocks(raw);
        assert!(decoded.contains("import os"));
        assert!(decoded.contains("print(\"sandbox\")"));
        assert!(!decoded.contains("```base64"));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Force {
    Thinking,
}

fn is_reasoning(obj: &Value) -> bool {
    if !obj.is_object() {
        return false;
    }
    let label = format!(
        "{} {} {}",
        obj.get("type").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("role").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("name").and_then(|v| v.as_str()).unwrap_or("")
    )
    .to_lowercase();
    REASONING_MARKERS.iter().any(|m| label.contains(m))
}

fn collect_structured(
    obj: &Value,
    force: Option<Force>,
    thinking: &mut Vec<String>,
    reply: &mut Vec<String>,
    depth: u32,
) {
    if obj.is_null() || depth > 10 {
        return;
    }

    match obj {
        Value::String(s) => {
            if !s.is_empty() {
                let target = if force == Some(Force::Thinking) {
                    thinking
                } else {
                    reply
                };
                target.push(s.clone());
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_structured(item, force, thinking, reply, depth + 1);
            }
        }
        Value::Object(map) => {
            let next_force = if is_reasoning(obj) {
                Some(Force::Thinking)
            } else {
                force
            };

            // choices 数组 (OpenAI Chat API)
            if let Some(choices) = map.get("choices").and_then(|v| v.as_array()) {
                for choice in choices {
                    if let Some(choice_obj) = choice.as_object() {
                        for key in &["message", "delta", "text", "content"] {
                            if let Some(val) = choice_obj.get(*key) {
                                collect_structured(val, next_force, thinking, reply, depth + 1);
                            }
                        }
                    }
                }
            }

            // output / delta / part
            for key in &["output", "delta", "part"] {
                if let Some(val) = map.get(*key) {
                    collect_structured(val, next_force, thinking, reply, depth + 1);
                }
            }

            // text keys
            for key in TEXT_KEYS {
                if let Some(val) = map.get(*key) {
                    collect_structured(val, next_force, thinking, reply, depth + 1);
                }
            }

            // wrapper keys
            for key in WRAPPER_KEYS {
                if let Some(val) = map.get(*key) {
                    collect_structured(val, next_force, thinking, reply, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// 合并流式分块，去重，检测重复模式
fn merge_chunks(chunks: &[String]) -> String {
    let mut merged = String::new();
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        if merged.is_empty() {
            merged = chunk.clone();
            continue;
        }
        if chunk == &merged {
            continue;
        }
        if chunk.len() > 20 && merged.contains(chunk) {
            continue;
        }
        if chunk.starts_with(&merged) {
            merged = chunk.clone();
            continue;
        }
        merged.push_str(chunk);
    }

    // 检测重复模式 (如 SSE 重复发送同一段内容)
    // 使用 char 边界而非字节边界，避免 panic
    let stripped = merged.trim().to_string();
    let char_count = stripped.chars().count();
    if char_count >= 12 {
        let chars: Vec<char> = stripped.chars().collect();
        for size in 4..(char_count / 2 + 1) {
            if char_count.is_multiple_of(size) {
                let repeats = char_count / size;
                let piece: String = chars[..size].iter().collect();
                if (repeats >= 3 || piece.chars().count() >= 12)
                    && piece.repeat(repeats) == stripped
                {
                    return piece.trim().to_string();
                }
            }
        }
    }

    stripped
}

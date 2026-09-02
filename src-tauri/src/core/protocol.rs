use crate::core::stages::StageKind;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

pub const INTERNAL_STAGE_TOOL: &str = "super_instruct_stage_complete";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiType {
    Responses,
    ChatCompletions,
}

impl ApiType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResearchClaimV1 {
    pub statement: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResearchArtifactV1 {
    pub kind: String,
    pub path: String,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StageResultV1 {
    pub session_id: String,
    pub stage: StageKind,
    pub status: String,
    pub summary: String,
    pub observations: Vec<ResearchClaimV1>,
    pub inferences: Vec<ResearchClaimV1>,
    pub hypotheses: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub artifacts: Vec<ResearchArtifactV1>,
    pub limitations: Vec<String>,
    pub unresolved: Vec<String>,
    pub next_stage: Option<StageKind>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolCallRecord {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub output_index: usize,
}

impl ToolCallRecord {
    pub fn is_internal(&self) -> bool {
        self.name == INTERNAL_STAGE_TOOL
    }

    pub fn stage_result(&self) -> Result<StageResultV1, String> {
        serde_json::from_str(&self.arguments)
            .map_err(|error| format!("invalid StageResultV1: {error}"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolOutputRecord {
    pub call_id: String,
    pub output: Value,
}

#[derive(Clone, Debug)]
pub struct ModelTurn {
    pub response_id: Option<String>,
    pub output_items: Vec<Value>,
    pub assistant_message: Option<Value>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub visible_text: String,
}

impl ModelTurn {
    pub fn internal_calls(&self) -> Vec<&ToolCallRecord> {
        self.tool_calls
            .iter()
            .filter(|call| call.is_internal())
            .collect()
    }

    pub fn external_calls(&self) -> Vec<&ToolCallRecord> {
        self.tool_calls
            .iter()
            .filter(|call| !call.is_internal())
            .collect()
    }
}

pub struct SyntheticResponse {
    pub content_type: &'static str,
    pub body: Bytes,
}

pub trait ProviderProtocol {
    fn api_type(&self) -> ApiType;
    fn inject_internal_tool(&self, body: &mut Value);
    fn parse_response(&self, body: &Bytes) -> Result<ModelTurn, String>;
    fn extract_tool_outputs(&self, body: &Value) -> Vec<ToolOutputRecord>;
    fn continue_body(
        &self,
        prior_request: &Value,
        turn: &ModelTurn,
        outputs: &[ToolOutputRecord],
    ) -> Value;
    fn correction_body(&self, prior_request: &Value, turn: &ModelTurn, message: &str) -> Value;
    fn filter_internal_calls(&self, body: &Bytes, turn: &ModelTurn) -> Bytes;
    fn synthetic_response(&self, stream: bool, id: &str, text: &str) -> SyntheticResponse;
}

#[derive(Clone, Copy, Debug)]
pub enum ProtocolAdapter {
    Responses,
    Chat,
}

impl ProtocolAdapter {
    pub fn detect(path: &str, body: &Value) -> Self {
        if path.contains("chat/completions") || body.get("messages").is_some() {
            Self::Chat
        } else {
            Self::Responses
        }
    }

    pub fn api_type(self) -> ApiType {
        match self {
            Self::Responses => ApiType::Responses,
            Self::Chat => ApiType::ChatCompletions,
        }
    }

    pub fn from_api(api_type: ApiType) -> Self {
        match api_type {
            ApiType::Responses => Self::Responses,
            ApiType::ChatCompletions => Self::Chat,
        }
    }

    pub fn inject_internal_tool(self, body: &mut Value) {
        match self {
            Self::Responses => ResponsesProtocol.inject_internal_tool(body),
            Self::Chat => ChatProtocol.inject_internal_tool(body),
        }
    }

    pub fn parse_response(self, body: &Bytes) -> Result<ModelTurn, String> {
        match self {
            Self::Responses => ResponsesProtocol.parse_response(body),
            Self::Chat => ChatProtocol.parse_response(body),
        }
    }

    pub fn extract_tool_outputs(self, body: &Value) -> Vec<ToolOutputRecord> {
        match self {
            Self::Responses => ResponsesProtocol.extract_tool_outputs(body),
            Self::Chat => ChatProtocol.extract_tool_outputs(body),
        }
    }

    pub fn continue_body(
        self,
        prior_request: &Value,
        turn: &ModelTurn,
        outputs: &[ToolOutputRecord],
    ) -> Value {
        match self {
            Self::Responses => ResponsesProtocol.continue_body(prior_request, turn, outputs),
            Self::Chat => ChatProtocol.continue_body(prior_request, turn, outputs),
        }
    }

    pub fn correction_body(self, prior_request: &Value, turn: &ModelTurn, message: &str) -> Value {
        match self {
            Self::Responses => ResponsesProtocol.correction_body(prior_request, turn, message),
            Self::Chat => ChatProtocol.correction_body(prior_request, turn, message),
        }
    }

    pub fn filter_internal_calls(self, body: &Bytes, turn: &ModelTurn) -> Bytes {
        match self {
            Self::Responses => ResponsesProtocol.filter_internal_calls(body, turn),
            Self::Chat => ChatProtocol.filter_internal_calls(body, turn),
        }
    }

    pub fn synthetic_response(self, stream: bool, id: &str, text: &str) -> SyntheticResponse {
        match self {
            Self::Responses => ResponsesProtocol.synthetic_response(stream, id, text),
            Self::Chat => ChatProtocol.synthetic_response(stream, id, text),
        }
    }
}

pub struct ResponsesProtocol;
pub struct ChatProtocol;

impl ProviderProtocol for ResponsesProtocol {
    fn api_type(&self) -> ApiType {
        ApiType::Responses
    }

    fn inject_internal_tool(&self, body: &mut Value) {
        append_tool(body, responses_tool_schema());
    }

    fn parse_response(&self, body: &Bytes) -> Result<ModelTurn, String> {
        let values = response_values(body)?;
        let mut response_id = None;
        let mut output_items = Vec::new();
        for value in &values {
            if response_id.is_none() {
                response_id = value
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .or_else(|| value.get("id"))
                    .or_else(|| value.get("response_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if let Some(output) = value
                .get("response")
                .and_then(|response| response.get("output"))
                .or_else(|| value.get("output"))
                .and_then(Value::as_array)
            {
                output_items = output.clone();
            } else if let Some(item) = value.get("item") {
                if value
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.starts_with("response.output_item."))
                {
                    upsert_output_item(&mut output_items, item.clone());
                }
            }
            if value.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.done")
            {
                apply_response_arguments(&mut output_items, value);
            }
        }
        if output_items.is_empty() && values.len() == 1 {
            if let Some(output) = values[0].get("output").and_then(Value::as_array) {
                output_items = output.clone();
            }
        }
        let tool_calls = calls_from_response_items(&output_items);
        let visible_text = text_from_response_items(&output_items);
        Ok(ModelTurn {
            response_id,
            output_items,
            assistant_message: None,
            tool_calls,
            visible_text,
        })
    }

    fn extract_tool_outputs(&self, body: &Value) -> Vec<ToolOutputRecord> {
        let mut outputs = Vec::new();
        collect_response_outputs(body.get("input").unwrap_or(body), &mut outputs);
        outputs
    }

    fn continue_body(
        &self,
        prior_request: &Value,
        turn: &ModelTurn,
        outputs: &[ToolOutputRecord],
    ) -> Value {
        let mut next = prior_request.clone();
        let has_cursor = responses_cursor_available(&mut next, turn);
        let input = normalized_responses_input(&mut next);
        if !has_cursor {
            input.extend(turn.output_items.clone());
        }
        input.extend(outputs.iter().map(|output| {
            json!({
                "type": "function_call_output",
                "call_id": output.call_id,
                "output": responses_output_value(&output.output)
            })
        }));
        next
    }

    fn correction_body(&self, prior_request: &Value, turn: &ModelTurn, message: &str) -> Value {
        let mut next = prior_request.clone();
        let has_cursor = responses_cursor_available(&mut next, turn);
        let input = normalized_responses_input(&mut next);
        if !has_cursor {
            input.extend(turn.output_items.clone());
        }
        input.push(json!({"role":"developer","content":[{"type":"input_text","text":message}]}));
        next
    }

    fn filter_internal_calls(&self, body: &Bytes, turn: &ModelTurn) -> Bytes {
        filter_responses_body(body, turn)
    }

    fn synthetic_response(&self, stream: bool, id: &str, text: &str) -> SyntheticResponse {
        responses_synthetic(stream, id, text)
    }
}

impl ProviderProtocol for ChatProtocol {
    fn api_type(&self) -> ApiType {
        ApiType::ChatCompletions
    }

    fn inject_internal_tool(&self, body: &mut Value) {
        append_tool(body, chat_tool_schema());
    }

    fn parse_response(&self, body: &Bytes) -> Result<ModelTurn, String> {
        let values = response_values(body)?;
        let is_stream = values.iter().any(|value| {
            value.get("object").and_then(Value::as_str) == Some("chat.completion.chunk")
        });
        let (response_id, assistant_message) = if is_stream {
            aggregate_chat_stream(&values)
        } else {
            let value = values.first().ok_or("empty chat response")?;
            let message = value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"))
                .cloned();
            (
                value.get("id").and_then(Value::as_str).map(str::to_string),
                message,
            )
        };
        let assistant_message =
            assistant_message.ok_or("chat response missing assistant message")?;
        let tool_calls = calls_from_chat_message(&assistant_message);
        let visible_text = content_to_string(assistant_message.get("content"));
        Ok(ModelTurn {
            response_id,
            output_items: Vec::new(),
            assistant_message: Some(assistant_message),
            tool_calls,
            visible_text,
        })
    }

    fn extract_tool_outputs(&self, body: &Value) -> Vec<ToolOutputRecord> {
        body.get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
            .filter_map(|message| {
                Some(ToolOutputRecord {
                    call_id: message.get("tool_call_id")?.as_str()?.to_string(),
                    output: message.get("content").cloned().unwrap_or(Value::Null),
                })
            })
            .collect()
    }

    fn continue_body(
        &self,
        prior_request: &Value,
        turn: &ModelTurn,
        outputs: &[ToolOutputRecord],
    ) -> Value {
        let mut next = prior_request.clone();
        let messages = normalized_chat_messages(&mut next);
        if let Some(message) = &turn.assistant_message {
            messages.push(message.clone());
        }
        messages.extend(outputs.iter().map(|output| {
            json!({
                "role": "tool",
                "tool_call_id": output.call_id,
                "content": output_value(&output.output)
            })
        }));
        next
    }

    fn correction_body(&self, prior_request: &Value, turn: &ModelTurn, message: &str) -> Value {
        let mut next = prior_request.clone();
        let messages = normalized_chat_messages(&mut next);
        if let Some(assistant) = &turn.assistant_message {
            messages.push(assistant.clone());
        }
        messages.push(json!({"role":"system","content":message}));
        next
    }

    fn filter_internal_calls(&self, body: &Bytes, turn: &ModelTurn) -> Bytes {
        filter_chat_body(body, turn)
    }

    fn synthetic_response(&self, stream: bool, id: &str, text: &str) -> SyntheticResponse {
        chat_synthetic(stream, id, text)
    }
}

fn responses_tool_schema() -> Value {
    let claim = json!({
        "type":"object",
        "properties":{
            "statement":{"type":"string"},
            "evidence_refs":{"type":"array","items":{"type":"string"}}
        },
        "required":["statement","evidence_refs"],
        "additionalProperties":false
    });
    json!({
        "type":"function",
        "name":INTERNAL_STAGE_TOOL,
        "description":"Submit the complete result for the current research stage. Call this only when no ordinary tool call is pending.",
        "strict":true,
        "parameters":stage_result_schema(claim)
    })
}

fn chat_tool_schema() -> Value {
    let response = responses_tool_schema();
    json!({
        "type":"function",
        "function":{
            "name":response["name"],
            "description":response["description"],
            "strict":true,
            "parameters":response["parameters"]
        }
    })
}

fn stage_result_schema(claim: Value) -> Value {
    json!({
        "type":"object",
        "properties":{
            "session_id":{"type":"string"},
            "stage":{"type":"string","enum":["framing","planning","evidence","analysis","transformation","execution","verification","reporting"]},
            "status":{"type":"string","enum":["completed","partial","pending"]},
            "summary":{"type":"string"},
            "observations":{"type":"array","items":claim.clone()},
            "inferences":{"type":"array","items":claim},
            "hypotheses":{"type":"array","items":{"type":"string"}},
            "evidence_refs":{"type":"array","items":{"type":"string"}},
            "artifacts":{"type":"array","items":{
                "type":"object",
                "properties":{
                    "kind":{"type":"string"},
                    "path":{"type":"string"},
                    "sha256":{"type":["string","null"]}
                },
                "required":["kind","path","sha256"],
                "additionalProperties":false
            }},
            "limitations":{"type":"array","items":{"type":"string"}},
            "unresolved":{"type":"array","items":{"type":"string"}},
            "next_stage":{"type":["string","null"],"enum":["framing","planning","evidence","analysis","transformation","execution","verification","reporting",null]}
        },
        "required":["session_id","stage","status","summary","observations","inferences","hypotheses","evidence_refs","artifacts","limitations","unresolved","next_stage"],
        "additionalProperties":false
    })
}

fn append_tool(body: &mut Value, tool: Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let tools = object
        .entry("tools")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(tools) = tools.as_array_mut() else {
        return;
    };
    tools.retain(|candidate| tool_name(candidate).as_deref() != Some(INTERNAL_STAGE_TOOL));
    tools.push(tool);
}

fn responses_cursor_available(next: &mut Value, turn: &ModelTurn) -> bool {
    let Some(object) = next.as_object_mut() else {
        return false;
    };
    if object
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        object.remove("previous_response_id");
        return true;
    }
    if let Some(response_id) = &turn.response_id {
        object.insert(
            "previous_response_id".to_string(),
            Value::String(response_id.clone()),
        );
        return true;
    }
    if object
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return true;
    }
    object.remove("conversation");
    object.remove("previous_response_id");
    false
}

fn tool_name(tool: &Value) -> Option<String> {
    tool.get("name")
        .or_else(|| {
            tool.get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn response_values(body: &Bytes) -> Result<Vec<Value>, String> {
    let text = std::str::from_utf8(body).map_err(|error| error.to_string())?;
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str(trimmed)
            .map(|value| vec![value])
            .map_err(|error| error.to_string());
    }
    let mut values = Vec::new();
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str(data) {
            values.push(value);
        }
    }
    if values.is_empty() {
        Err("provider response contained no JSON events".to_string())
    } else {
        Ok(values)
    }
}

fn upsert_output_item(items: &mut Vec<Value>, item: Value) {
    let identity = item
        .get("id")
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str);
    if let Some(identity) = identity {
        if let Some(position) = items.iter().position(|existing| {
            existing
                .get("id")
                .or_else(|| existing.get("call_id"))
                .and_then(Value::as_str)
                == Some(identity)
        }) {
            items[position] = item;
            return;
        }
    }
    items.push(item);
}

fn apply_response_arguments(items: &mut [Value], event: &Value) {
    let identity = event.get("item_id").and_then(Value::as_str);
    let output_index = event
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|index| index as usize);
    let arguments = event
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let position = identity
        .and_then(|identity| {
            items.iter().position(|item| {
                item.get("id").and_then(Value::as_str) == Some(identity)
                    || item.get("call_id").and_then(Value::as_str) == Some(identity)
            })
        })
        .or(output_index.filter(|index| *index < items.len()));
    if let Some(item) = position.and_then(|position| items.get_mut(position)) {
        item["arguments"] = Value::String(arguments.to_string());
    }
}

fn calls_from_response_items(items: &[Value]) -> Vec<ToolCallRecord> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .filter_map(|(index, item)| {
            Some(ToolCallRecord {
                call_id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))?
                    .as_str()?
                    .to_string(),
                name: item.get("name")?.as_str()?.to_string(),
                arguments: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}")
                    .to_string(),
                output_index: index,
            })
        })
        .collect()
}

fn calls_from_chat_message(message: &Value) -> Vec<ToolCallRecord> {
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, call)| {
            let function = call.get("function")?;
            Some(ToolCallRecord {
                call_id: call.get("id")?.as_str()?.to_string(),
                name: function.get("name")?.as_str()?.to_string(),
                arguments: function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}")
                    .to_string(),
                output_index: index,
            })
        })
        .collect()
}

fn text_from_response_items(items: &[Value]) -> String {
    let mut parts = Vec::new();
    for item in items {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("")
}

fn aggregate_chat_stream(values: &[Value]) -> (Option<String>, Option<Value>) {
    let mut response_id = None;
    let mut role = "assistant".to_string();
    let mut content = String::new();
    let mut calls: BTreeMap<usize, Value> = BTreeMap::new();
    for value in values {
        if response_id.is_none() {
            response_id = value.get("id").and_then(Value::as_str).map(str::to_string);
        }
        let Some(delta) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
        else {
            continue;
        };
        if let Some(value) = delta.get("role").and_then(Value::as_str) {
            role = value.to_string();
        }
        if let Some(value) = delta.get("content").and_then(Value::as_str) {
            content.push_str(value);
        }
        for tool_delta in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = tool_delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = calls.entry(index).or_insert_with(
                || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
            );
            if let Some(id) = tool_delta.get("id").and_then(Value::as_str) {
                entry["id"] = Value::String(id.to_string());
            }
            if let Some(function) = tool_delta.get("function") {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    entry["function"]["name"] = Value::String(name.to_string());
                }
                if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                    let current = entry["function"]["arguments"].as_str().unwrap_or("");
                    entry["function"]["arguments"] = Value::String(format!("{current}{arguments}"));
                }
            }
        }
    }
    let mut message = json!({"role":role,"content":if content.is_empty(){Value::Null}else{Value::String(content)}});
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls.into_values().collect());
    }
    (response_id, Some(message))
}

fn collect_response_outputs(value: &Value, outputs: &mut Vec<ToolOutputRecord>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_response_outputs(value, outputs);
            }
        }
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "function_call_output" || kind.ends_with("_call_output") {
                if let Some(call_id) = object
                    .get("call_id")
                    .or_else(|| object.get("id"))
                    .and_then(Value::as_str)
                {
                    outputs.push(ToolOutputRecord {
                        call_id: call_id.to_string(),
                        output: object.get("output").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            for nested in object.values() {
                if nested.is_array() || nested.is_object() {
                    collect_response_outputs(nested, outputs);
                }
            }
        }
        _ => {}
    }
}

fn normalized_responses_input(body: &mut Value) -> &mut Vec<Value> {
    let object = body.as_object_mut().expect("request body is an object");
    let input = object
        .entry("input")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !input.is_array() {
        let previous = std::mem::replace(input, Value::Array(Vec::new()));
        *input = Value::Array(vec![json!({"role":"user","content":previous})]);
    }
    input.as_array_mut().expect("input normalized to array")
}

fn normalized_chat_messages(body: &mut Value) -> &mut Vec<Value> {
    body.as_object_mut()
        .expect("request body is an object")
        .entry("messages")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("messages must be an array")
}

fn output_value(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        other => Value::String(other.to_string()),
    }
}

fn responses_output_value(value: &Value) -> Value {
    match value {
        Value::String(_) | Value::Array(_) => value.clone(),
        other => Value::String(other.to_string()),
    }
}

fn content_to_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn filter_responses_body(body: &Bytes, turn: &ModelTurn) -> Bytes {
    let internal: HashSet<usize> = turn
        .tool_calls
        .iter()
        .filter(|call| call.is_internal())
        .map(|call| call.output_index)
        .collect();
    if internal.is_empty() {
        return body.clone();
    }
    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('{') {
        if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
            if let Some(output) = value.get_mut("output").and_then(Value::as_array_mut) {
                output.retain(|item| {
                    item.get("name").and_then(Value::as_str) != Some(INTERNAL_STAGE_TOOL)
                });
            }
            return Bytes::from(value.to_string());
        }
        return body.clone();
    }
    filter_sse_frames(&text, |value| {
        if value
            .get("output_index")
            .and_then(Value::as_u64)
            .is_some_and(|index| internal.contains(&(index as usize)))
        {
            return false;
        }
        if value
            .get("item")
            .and_then(|item| item.get("name"))
            .and_then(Value::as_str)
            == Some(INTERNAL_STAGE_TOOL)
        {
            return false;
        }
        if let Some(output) = value
            .get_mut("response")
            .and_then(|response| response.get_mut("output"))
            .and_then(Value::as_array_mut)
        {
            output.retain(|item| {
                item.get("name").and_then(Value::as_str) != Some(INTERNAL_STAGE_TOOL)
            });
        }
        true
    })
}

fn filter_chat_body(body: &Bytes, turn: &ModelTurn) -> Bytes {
    let internal: HashSet<usize> = turn
        .tool_calls
        .iter()
        .filter(|call| call.is_internal())
        .map(|call| call.output_index)
        .collect();
    if internal.is_empty() {
        return body.clone();
    }
    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('{') {
        if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
            if let Some(calls) = value
                .get_mut("choices")
                .and_then(Value::as_array_mut)
                .and_then(|choices| choices.first_mut())
                .and_then(|choice| choice.get_mut("message"))
                .and_then(|message| message.get_mut("tool_calls"))
                .and_then(Value::as_array_mut)
            {
                calls.retain(|call| {
                    call.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        != Some(INTERNAL_STAGE_TOOL)
                });
            }
            return Bytes::from(value.to_string());
        }
        return body.clone();
    }
    filter_sse_frames(&text, |value| {
        if let Some(calls) = value
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.first_mut())
            .and_then(|choice| choice.get_mut("delta"))
            .and_then(|delta| delta.get_mut("tool_calls"))
            .and_then(Value::as_array_mut)
        {
            calls.retain(|call| {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                !internal.contains(&index)
            });
        }
        true
    })
}

fn filter_sse_frames(text: &str, mut retain: impl FnMut(&mut Value) -> bool) -> Bytes {
    let mut output = String::new();
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut event_name = None;
        let mut data = None;
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("event:") {
                event_name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data = Some(value.trim().to_string());
            }
        }
        let Some(data) = data else { continue };
        if data == "[DONE]" {
            output.push_str("data: [DONE]\n\n");
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if !retain(&mut value) {
            continue;
        }
        if let Some(event_name) = event_name.or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        }) {
            output.push_str(&format!("event: {event_name}\n"));
        }
        output.push_str(&format!("data: {value}\n\n"));
    }
    Bytes::from(output)
}

fn responses_synthetic(stream: bool, id: &str, text: &str) -> SyntheticResponse {
    let item_id = format!("msg_{id}");
    let message = json!({
        "id":item_id,
        "type":"message",
        "role":"assistant",
        "status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]
    });
    let response = json!({
        "id":id,
        "object":"response",
        "status":"completed",
        "output":[message.clone()],
        "output_text":text
    });
    if !stream {
        return SyntheticResponse {
            content_type: "application/json",
            body: Bytes::from(response.to_string()),
        };
    }
    let events = vec![
        json!({"type":"response.created","response":{"id":id,"object":"response","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","response_id":id,"output_index":0,"item":{"id":item_id,"type":"message","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","response_id":id,"item_id":item_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","response_id":id,"item_id":item_id,"output_index":0,"content_index":0,"delta":text}),
        json!({"type":"response.output_text.done","response_id":id,"item_id":item_id,"output_index":0,"content_index":0,"text":text}),
        json!({"type":"response.content_part.done","response_id":id,"item_id":item_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":text,"annotations":[]}}),
        json!({"type":"response.output_item.done","response_id":id,"output_index":0,"item":message}),
        json!({"type":"response.completed","response":response}),
    ];
    let mut body = events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {}\n\n",
                event["type"].as_str().unwrap_or("message"),
                event
            )
        })
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    SyntheticResponse {
        content_type: "text/event-stream",
        body: Bytes::from(body),
    }
}

fn chat_synthetic(stream: bool, id: &str, text: &str) -> SyntheticResponse {
    if !stream {
        let body = json!({
            "id":id,
            "object":"chat.completion",
            "choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]
        });
        return SyntheticResponse {
            content_type: "application/json",
            body: Bytes::from(body.to_string()),
        };
    }
    let chunks = [
        json!({"id":id,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}),
        json!({"id":id,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}),
        json!({"id":id,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
    ];
    let mut body = chunks
        .into_iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    SyntheticResponse {
        content_type: "text/event-stream",
        body: Bytes::from(body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_parses_function_call_and_complete_event() {
        let body = Bytes::from_static(
            b"event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"super_instruct_stage_complete\",\"arguments\":\"{}\"}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"super_instruct_stage_complete\",\"arguments\":\"{}\"}]}}\n\ndata: [DONE]\n\n",
        );
        let turn = ResponsesProtocol
            .parse_response(&body)
            .expect("responses turn");
        assert_eq!(turn.response_id.as_deref(), Some("resp_1"));
        assert_eq!(turn.internal_calls().len(), 1);
    }

    #[test]
    fn responses_aggregates_incremental_function_arguments_without_completed_snapshot() {
        let body = Bytes::from_static(
            b"event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"response_id\":\"resp_2\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_2\",\"name\":\"shell\",\"arguments\":\"\"}}\n\nevent: response.function_call_arguments.done\ndata: {\"type\":\"response.function_call_arguments.done\",\"response_id\":\"resp_2\",\"item_id\":\"fc_1\",\"output_index\":0,\"arguments\":\"{\\\"cmd\\\":\\\"pwd\\\"}\"}\n\ndata: [DONE]\n\n",
        );
        let turn = ResponsesProtocol
            .parse_response(&body)
            .expect("incremental responses turn");
        assert_eq!(turn.response_id.as_deref(), Some("resp_2"));
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].call_id, "call_2");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"cmd":"pwd"}"#);
    }

    #[test]
    fn chat_stream_aggregates_tool_call_arguments() {
        let body = Bytes::from_static(
            b"data: {\"id\":\"chat_1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"tool\",\"arguments\":\"{\\\"x\\\":\"}}]}}]}\n\ndata: {\"id\":\"chat_1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\ndata: [DONE]\n\n",
        );
        let turn = ChatProtocol.parse_response(&body).expect("chat turn");
        assert_eq!(turn.tool_calls[0].arguments, "{\"x\":1}");
    }

    #[test]
    fn chat_outputs_use_tool_call_id() {
        let body = json!({"messages":[{"role":"tool","tool_call_id":"call_7","content":"ok"}]});
        let outputs = ChatProtocol.extract_tool_outputs(&body);
        assert_eq!(outputs[0].call_id, "call_7");
    }

    #[test]
    fn responses_replay_preserves_reasoning_items() {
        let request = json!({"model":"test","input":[{"role":"user","content":"task"}]});
        let turn = ModelTurn {
            response_id: None,
            output_items: vec![
                json!({"id":"rs_1","type":"reasoning","encrypted_content":"ciphertext"}),
                json!({"id":"fc_1","type":"function_call","call_id":"call_1","name":INTERNAL_STAGE_TOOL,"arguments":"{}"}),
            ],
            assistant_message: None,
            tool_calls: Vec::new(),
            visible_text: String::new(),
        };
        let next = ResponsesProtocol.continue_body(
            &request,
            &turn,
            &[ToolOutputRecord {
                call_id: "call_1".into(),
                output: Value::String("accepted".into()),
            }],
        );
        assert!(next["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "reasoning"));
    }

    #[test]
    fn responses_continue_prefers_conversation_then_response_cursor() {
        let turn = ModelTurn {
            response_id: Some("resp_2".into()),
            output_items: vec![json!({"id":"rs_1","type":"reasoning"})],
            assistant_message: None,
            tool_calls: Vec::new(),
            visible_text: String::new(),
        };
        let output = ToolOutputRecord {
            call_id: "call_1".into(),
            output: Value::String("accepted".into()),
        };

        let conversation = ResponsesProtocol.continue_body(
            &json!({"model":"test","conversation":"conv_1","input":[]}),
            &turn,
            std::slice::from_ref(&output),
        );
        assert_eq!(conversation["conversation"], "conv_1");
        assert!(conversation.get("previous_response_id").is_none());
        assert_eq!(conversation["input"].as_array().unwrap().len(), 1);

        let previous =
            ResponsesProtocol.continue_body(&json!({"model":"test","input":[]}), &turn, &[output]);
        assert_eq!(previous["previous_response_id"], "resp_2");
        assert_eq!(previous["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn mixed_chat_calls_hide_only_internal_control_call() {
        let body = Bytes::from(
            json!({
                "id":"chat_1",
                "object":"chat.completion",
                "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[
                    {"id":"call_internal","type":"function","function":{"name":INTERNAL_STAGE_TOOL,"arguments":"{}"}},
                    {"id":"call_external","type":"function","function":{"name":"shell","arguments":"{}"}}
                ]},"finish_reason":"tool_calls"}]
            })
            .to_string(),
        );
        let turn = ChatProtocol.parse_response(&body).expect("mixed turn");
        let filtered = ChatProtocol.filter_internal_calls(&body, &turn);
        let value: Value = serde_json::from_slice(&filtered).expect("filtered json");
        let calls = value["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["function"]["name"], "shell");
    }
}

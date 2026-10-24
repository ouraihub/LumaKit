use crate::{AiConnection, Message, Protocol, ToolCall, ToolSpec};
use serde_json::{Map, Value, json};

pub(crate) fn path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::AnthropicMessages => "messages",
        Protocol::OpenAiChat => "chat/completions",
        Protocol::OpenAiResponses => "responses",
    }
}

pub(crate) fn encode(
    connection: &AiConnection,
    system: Option<&str>,
    messages: &[Message],
    tools: &[ToolSpec],
) -> Value {
    let mut encoded = Vec::new();
    for message in messages {
        match (connection.protocol, message) {
            (Protocol::AnthropicMessages, Message::User { text, images }) => {
                let content = if images.is_empty() { json!(text) } else {
                    let mut content = vec![json!({"type":"text", "text":text})];
                    content.extend(images.iter().map(|image| json!({"type":"image", "source":{"type":"base64", "media_type":image.mime, "data":image.data}})));
                    json!(content)
                };
                encoded.push(json!({"role":"user", "content": content}));
            }
            (Protocol::AnthropicMessages, Message::Assistant { text, tool_calls, reasoning_blocks }) => {
                let mut content = reasoning_blocks.clone();
                if !text.is_empty() {
                    content.push(json!({"type":"text", "text":text}));
                }
                content.extend(tool_calls.iter().map(|call| {
                    json!({
                        "type":"tool_use", "id":call.id, "name":call.name, "input":call.arguments
                    })
                }));
                encoded.push(json!({"role":"assistant", "content": if content.is_empty() { json!(text) } else { json!(content) }}));
            }
            (Protocol::AnthropicMessages, Message::Tool { call_id, text }) => {
                if let Some(Value::Object(last)) = encoded.last_mut() {
                    if last.get("role") == Some(&Value::String("user".into()))
                        && last["content"].is_array()
                    {
                        last["content"]
                            .as_array_mut()
                            .expect("array checked")
                            .push(json!({
                                "type":"tool_result", "tool_use_id":call_id, "content":text
                            }));
                        continue;
                    }
                }
                encoded.push(json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":call_id, "content":text}]}));
            }
            (Protocol::OpenAiChat, Message::User { text, images }) => {
                if images.is_empty() { encoded.push(json!({"role":"user", "content":text})); }
                else {
                    let mut content = vec![json!({"type":"text", "text":text})];
                    content.extend(images.iter().map(|image| json!({"type":"image_url", "image_url":{"url":crate::images::data_url(image)}})));
                    encoded.push(json!({"role":"user", "content":content}));
                }
            }
            (Protocol::OpenAiResponses, Message::User { text, images }) => {
                let content = if images.is_empty() { json!(text) } else {
                    let mut content = vec![json!({"type":"input_text", "text":text})];
                    content.extend(images.iter().map(|image| json!({"type":"input_image", "image_url":crate::images::data_url(image)})));
                    json!(content)
                };
                encoded.push(json!({"role":"user", "content":content}));
            }
            (_, Message::Assistant { text, tool_calls, .. }) if connection.protocol == Protocol::OpenAiResponses => {
                if !text.is_empty() || tool_calls.is_empty() { encoded.push(json!({"role":"assistant", "content":text})); }
                encoded.extend(tool_calls.iter().map(|call| {
                    json!({
                        "type":"function_call", "call_id":call.id, "name":call.name,
                        "arguments":call.arguments.to_string()
                    })
                }));
            }
            (_, Message::Assistant { text, tool_calls, .. }) => {
                let mut value = json!({"role":"assistant", "content":text});
                if !tool_calls.is_empty() {
                    value["tool_calls"] = tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id":call.id, "type":"function", "function":{
                                    "name":call.name, "arguments":call.arguments.to_string()
                                }
                            })
                        })
                        .collect();
                }
                encoded.push(value);
            }
            (Protocol::OpenAiResponses, Message::Tool { call_id, text }) => {
                encoded
                    .push(json!({"type":"function_call_output", "call_id":call_id, "output":text}));
            }
            (_, Message::Tool { call_id, text }) => {
                encoded.push(json!({"role":"tool", "tool_call_id":call_id, "content":text}));
            }
        }
    }
    let mut body = match connection.protocol {
        Protocol::AnthropicMessages => {
            let mut body = json!({"model":connection.model,"max_tokens":connection.settings.output_limit(),"stream":true,"messages":encoded});
            if let Some(system) = system.filter(|text| !text.is_empty()) {
                body["system"] = json!(system);
            }
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(|tool| json!({
                "name":tool.name,"description":tool.description,"input_schema":tool.parameters
            })).collect();
            }
            body
        }
        Protocol::OpenAiChat => {
            if let Some(system) = system.filter(|text| !text.is_empty()) {
                encoded.insert(0, json!({"role":"system","content":system}));
            }
            let mut body = json!({"model":connection.model,"max_tokens":connection.settings.output_limit(),"stream":true,"stream_options":{"include_usage":true},"messages":encoded});
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(|tool| json!({
                "type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.parameters}
            })).collect();
            }
            body
        }
        Protocol::OpenAiResponses => {
            let mut body = json!({"model":connection.model,"max_output_tokens":connection.settings.output_limit(),"stream":true,"input":encoded});
            if let Some(system) = system.filter(|text| !text.is_empty()) {
                body["instructions"] = json!(system);
            }
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(|tool| json!({
                "type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters
            })).collect();
            }
            body
        }
    };
    if let Some(config) = &connection.settings.reasoning {
        if let Some(&effort) = config.efforts.first() {
            use crate::model_config::{ReasoningDialect::*, ReasoningEffort};
            let on = effort != ReasoningEffort::None;
            match config.dialect {
                OpenAi if connection.protocol == Protocol::OpenAiResponses => {
                    body["reasoning"] = json!({"effort":effort});
                    if reqwest::Url::parse(&connection.base_url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("api.openai.com")) { body["reasoning"]["summary"] = json!("auto"); }
                }
                OpenAi => body["reasoning_effort"] = json!(effort),
                ThinkingType => {
                    body["thinking"] = json!({"type":if on {"enabled"} else {"disabled"}});
                    if on && effort != ReasoningEffort::Auto { body["reasoning_effort"] = json!(effort); }
                }
                Qwen => {
                    body["enable_thinking"] = json!(on);
                    if let Some(cap) = config.budget_tokens { if on && effort != ReasoningEffort::Auto { body["thinking_budget"] = json!(scaled(cap, effort).max(1)); } }
                }
                AnthropicBudget => {
                    body["thinking"] = if on { json!({"type":"enabled","budget_tokens":scaled(config.budget_tokens.unwrap_or(1024),effort).max(1024).min(connection.settings.output_limit()-1)}) } else { json!({"type":"disabled"}) };
                }
                AnthropicAdaptive => {
                    body["thinking"] = json!({"type":if on {"adaptive"} else {"disabled"}});
                    if on { body["output_config"] = json!({"effort":effort}); }
                }
            }
        }
    }
    if connection.settings.native_web_search {
        let tool = if connection.protocol == Protocol::AnthropicMessages { json!({"type":"web_search_20250305","name":"web_search","max_uses":5}) } else { json!({"type":"web_search"}) };
        if body["tools"].is_null() { body["tools"] = json!([]); }
        body["tools"].as_array_mut().expect("tools encoded as array").push(tool);
    }
    body["stream"] = json!(connection.settings.supports_streaming);
    if !connection.settings.supports_streaming { body.as_object_mut().expect("body object").remove("stream_options"); }
    body
}

fn scaled(cap: u32, effort: crate::model_config::ReasoningEffort) -> u32 {
    use crate::model_config::ReasoningEffort::*;
    let percent = match effort { None | Auto => 0, Minimal => 5, Low => 10, Medium => 50, High => 80, Xhigh => 90, Max => 100 };
    (u64::from(cap) * percent / 100) as u32
}
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Option<String>,
}
impl SseParser {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.buffer[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            let line = self.buffer[start..end].to_vec();
            let line = line.strip_suffix(b"\r").unwrap_or(&line).to_vec();
            self.line(&line, &mut events);
            start = end + 1;
        }
        self.buffer.drain(..start);
        events
    }
    pub(crate) fn finish(mut self) -> Vec<String> {
        let line = std::mem::take(&mut self.buffer);
        let mut events = Vec::new();
        if !line.is_empty() {
            let line = line.strip_suffix(b"\r").unwrap_or(&line).to_vec();
            self.line(&line, &mut events);
        }
        self.line(b"", &mut events);
        events
    }
    fn line(&mut self, line: &[u8], events: &mut Vec<String>) {
        if line.is_empty() {
            if let Some(data) = self.data.take() {
                events.push(data);
            }
            self.event = None;
            return;
        }
        let line = String::from_utf8_lossy(line);
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = (!value.is_empty()).then(|| value.to_owned()),
            "data" => match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            },
            _ => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct Decoder {
    pub done: bool,
    calls: Vec<PartialCall>,
    pub usage: Option<crate::TokenUsage>,
    pub reasoning_blocks: Vec<Value>,
}
struct PartialCall {
    key: Value,
    id: String,
    name: String,
    arguments: String,
}
impl Decoder {
    pub(crate) fn complete(&mut self, protocol: Protocol, value: &Value) -> Result<String, String> {
        if !value["error"].is_null() { return Err(value["error"]["message"].as_str().unwrap_or("provider error").into()); }
        self.read_usage(protocol, value)?;
        let mut text = String::new();
        match protocol {
            Protocol::OpenAiChat => {
                if !matches!(value["choices"][0]["finish_reason"].as_str(), Some("stop" | "tool_calls")) { return Err("provider response incomplete".into()); }
                let message = &value["choices"][0]["message"];
                text.push_str(message["content"].as_str().unwrap_or_default());
                if let Some(calls) = message["tool_calls"].as_array() { for call in calls { self.start(&call["id"], &call["id"], &call["function"]["name"])?; self.append(&call["id"], &call["function"]["arguments"])?; } }
            }
            Protocol::AnthropicMessages => {
                if !matches!(value["stop_reason"].as_str(), Some("end_turn" | "tool_use" | "stop_sequence")) { return Err("provider response incomplete".into()); }
                for block in value["content"].as_array().ok_or("invalid Anthropic content")? {
                    match block["type"].as_str() {
                        Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
                        Some("thinking" | "redacted_thinking") => self.reasoning_blocks.push(block.clone()),
                        Some("tool_use") => { self.start(&block["id"], &block["id"], &block["name"])?; self.append(&block["id"], &json!(block["input"].to_string()))?; }
                        _ => {}
                    }
                }
            }
            Protocol::OpenAiResponses => {
                if value["status"] != "completed" { return Err("provider response incomplete".into()); }
                for item in value["output"].as_array().ok_or("invalid Responses output")? {
                    if item["type"] == "function_call" { self.start(&item["id"], &item["call_id"], &item["name"])?; self.append(&item["id"], &item["arguments"])?; }
                    if let Some(content) = item["content"].as_array() { for block in content { if block["type"] == "output_text" { text.push_str(block["text"].as_str().unwrap_or_default()); } } }
                }
            }
        }
        self.done = true;
        Ok(text)
    }
    pub(crate) fn event(
        &mut self,
        protocol: Protocol,
        data: &str,
    ) -> Result<Option<String>, String> {
        if data == "[DONE]" {
            self.done = true;
            return Ok(None);
        }
        let value: Value = serde_json::from_str(data).map_err(|_| "invalid SSE JSON".to_owned())?;
        self.read_usage(protocol, &value)?;
        if protocol == Protocol::AnthropicMessages {
            match value["type"].as_str().unwrap_or_default() {
                "content_block_start" if matches!(value["content_block"]["type"].as_str(), Some("thinking" | "redacted_thinking")) => self.reasoning_blocks.push(value["content_block"].clone()),
                "content_block_delta" => {
                    if let Some(block) = self.reasoning_blocks.last_mut() {
                        let pair = match value["delta"]["type"].as_str() { Some("thinking_delta") => Some(("thinking", "thinking")), Some("signature_delta") => Some(("signature", "signature")), _ => None };
                        if let Some((target, source)) = pair { if let Some(fragment) = value["delta"][source].as_str() { let mut text = block[target].as_str().unwrap_or_default().to_owned(); text.push_str(fragment); block[target] = json!(text); } }
                    }
                }
                _ => {}
            }
        }
        match protocol {
            Protocol::AnthropicMessages => self.anthropic(&value),
            Protocol::OpenAiChat => self.chat(&value),
            Protocol::OpenAiResponses => self.responses(&value),
        }
    }
    fn read_usage(&mut self, protocol: Protocol, value: &Value) -> Result<(), String> {
        let usage = match protocol {
            Protocol::AnthropicMessages if value["type"] == "message_start" => &value["message"]["usage"],
            Protocol::OpenAiResponses if value["response"].is_object() => &value["response"]["usage"],
            _ => &value["usage"],
        };
        if !usage.is_object() { return Ok(()); }
        let mut parsed = self.usage.clone().unwrap_or_default();
        let read = |key: &str| -> Result<Option<u64>, String> { match usage.get(key) { None | Some(Value::Null) => Ok(None), Some(n) => n.as_u64().map(Some).ok_or_else(|| format!("invalid token usage: {key}")) } };
        let input_key = if protocol == Protocol::OpenAiChat { "prompt_tokens" } else { "input_tokens" };
        let output_key = if protocol == Protocol::OpenAiChat { "completion_tokens" } else { "output_tokens" };
        if self.usage.is_none() && (read(input_key)?.is_none() || read(output_key)?.is_none()) { return Ok(()); }
        if let Some(n) = read(output_key)? { parsed.output_tokens = n; }
        if protocol == Protocol::AnthropicMessages {
            if let Some(n) = read("cache_read_input_tokens")? { parsed.cache_read_tokens = n; }
            if let Some(n) = read("cache_creation_input_tokens")? { parsed.cache_write_tokens = n; }
            if let Some(n) = read(input_key)? { parsed.input_tokens = n.checked_add(parsed.cache_read_tokens).and_then(|n| n.checked_add(parsed.cache_write_tokens)).ok_or("token usage overflow")?; }
        } else {
            if let Some(n) = read(input_key)? { parsed.input_tokens = n; }
            let details = if protocol == Protocol::OpenAiChat { &usage["prompt_tokens_details"] } else { &usage["input_tokens_details"] };
            if let Some(n) = details.get("cached_tokens") { parsed.cache_read_tokens = n.as_u64().ok_or("invalid cached token usage")?; }
        }
        if parsed.cache_read_tokens.checked_add(parsed.cache_write_tokens).is_none_or(|n| n > parsed.input_tokens) { return Err("cached token usage exceeds total input".into()); }
        self.usage = Some(parsed);
        Ok(())
    }
    fn anthropic(&mut self, value: &Value) -> Result<Option<String>, String> {
        match value["type"].as_str().unwrap_or_default() {
            "content_block_start" if value["content_block"]["type"] == "tool_use" => self.start(
                &value["index"],
                &value["content_block"]["id"],
                &value["content_block"]["name"],
            ),
            "content_block_delta" if value["delta"]["type"] == "input_json_delta" => {
                self.append(&value["index"], &value["delta"]["partial_json"])
            }
            "content_block_delta" if value["delta"]["type"] == "text_delta" => {
                Ok(value["delta"]["text"].as_str().map(str::to_owned))
            }
            "message_delta" if matches!(value["delta"]["stop_reason"].as_str(), Some("max_tokens" | "refusal")) => Err("provider response incomplete".into()),
            "message_stop" => {
                self.done = true;
                Ok(None)
            }
            "error" => Err(value["error"]["message"]
                .as_str()
                .unwrap_or("provider error")
                .to_owned()),
            _ => Ok(None),
        }
    }
    fn chat(&mut self, value: &Value) -> Result<Option<String>, String> {
        if !value["error"].is_null() {
            return Err(value["error"]["message"]
                .as_str()
                .unwrap_or("provider error")
                .to_owned());
        }
        let choice = &value["choices"][0];
        if let Some(deltas) = choice["delta"]["tool_calls"].as_array() {
            for delta in deltas {
                let key = &delta["index"];
                if self.find(key).is_none() {
                    self.start(key, &delta["id"], &delta["function"]["name"])?;
                }
                self.append(key, &delta["function"]["arguments"])?;
            }
        }
        if choice["finish_reason"].is_string() {
            if !matches!(choice["finish_reason"].as_str(), Some("stop" | "tool_calls")) { return Err("provider response incomplete".into()); }
            self.done = true;
        }
        Ok(choice["delta"]["content"].as_str().map(str::to_owned))
    }
    fn responses(&mut self, value: &Value) -> Result<Option<String>, String> {
        match value["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => Ok(value["delta"].as_str().map(str::to_owned)),
            "response.output_item.added" if value["item"]["type"] == "function_call" => self.start(
                &value["item"]["id"],
                &value["item"]["call_id"],
                &value["item"]["name"],
            ),
            "response.function_call_arguments.delta" => {
                self.append(&value["item_id"], &value["delta"])
            }
            "response.completed" => {
                self.done = true;
                Ok(None)
            }
            "response.incomplete" => Err("provider response incomplete".into()),
            "response.failed" | "error" => Err(value["response"]["error"]["message"]
                .as_str()
                .or_else(|| value["error"]["message"].as_str())
                .unwrap_or("provider error")
                .to_owned()),
            _ => Ok(None),
        }
    }
    fn find(&mut self, key: &Value) -> Option<&mut PartialCall> {
        self.calls.iter_mut().find(|call| call.key == *key)
    }
    fn start(&mut self, key: &Value, id: &Value, name: &Value) -> Result<Option<String>, String> {
        self.calls.push(PartialCall {
            key: key.clone(),
            id: id.as_str().unwrap_or_default().to_owned(),
            name: name.as_str().unwrap_or_default().to_owned(),
            arguments: String::new(),
        });
        Ok(None)
    }
    fn append(&mut self, key: &Value, fragment: &Value) -> Result<Option<String>, String> {
        if let (Some(call), Some(fragment)) = (self.find(key), fragment.as_str()) {
            call.arguments.push_str(fragment);
        }
        Ok(None)
    }
    pub(crate) fn finish(self) -> Result<Vec<ToolCall>, String> {
        self.calls
            .into_iter()
            .map(|call| {
                let arguments = if call.arguments.is_empty() {
                    "{}"
                } else {
                    &call.arguments
                };
                let arguments: Map<String, Value> = serde_json::from_str(arguments)
                    .map_err(|_| "tool arguments are not a JSON object".to_owned())?;
                Ok(ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: Value::Object(arguments),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_keeps_utf8_and_multiline_data() {
        let mut parser = SseParser::default();
        let mut events = parser.feed("data: 你".as_bytes());
        events.extend(parser.feed("好\n\ndata: [DONE]\n\n".as_bytes()));
        assert_eq!(events, vec!["你好".to_owned(), "[DONE]".to_owned()]);
    }

    #[test]
    fn openai_chat_request_contains_system_and_tools() {
        let connection = AiConnection {
            base_url: "http://127.0.0.1:1".into(),
            protocol: Protocol::OpenAiChat,
            api_key: None,
            model: "demo".into(),
            settings: Default::default(),
        };
        let body = encode(
            &connection,
            Some("system"),
            &[Message::User { text: "hello".into(), images: Vec::new() }],
            &[ToolSpec {
                name: "read".into(),
                description: "read status".into(),
                parameters: json!({"type":"object"}),
                read_only: true,
                requires_approval: false,
            }],
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn configured_requests_usage_and_token_prices() {
        use crate::model_config::*;
        let mut c = AiConnection { base_url: "https://api.openai.com/v1".into(), protocol: Protocol::OpenAiResponses, api_key: None, model: "model".into(), settings: ModelSettings { max_output: Some(8192), native_web_search: true, reasoning: Some(ReasoningConfig { dialect: ReasoningDialect::OpenAi, efforts: vec![ReasoningEffort::High], budget_tokens: None }), ..Default::default() } };
        let body = encode(&c, None, &[], &[]);
        assert_eq!(body["max_output_tokens"], 8192);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["tools"][0]["type"], "web_search");
        c.protocol = Protocol::AnthropicMessages;
        c.settings.reasoning = Some(ReasoningConfig { dialect: ReasoningDialect::AnthropicBudget, efforts: vec![ReasoningEffort::High], budget_tokens: Some(2048) });
        assert!(c.settings.validate(c.protocol).is_ok());
        let body = encode(&c, None, &[], &[]);
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["thinking"]["budget_tokens"], 1638);
        assert_eq!(body["tools"][0]["type"], "web_search_20250305");
        let mut d = Decoder::default();
        d.event(c.protocol, r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":50,"cache_creation_input_tokens":25}}}"#).unwrap();
        d.event(c.protocol, r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"web","name":"web_search"}}"#).unwrap();
        d.event(c.protocol, r#"{"type":"message_delta","usage":{"output_tokens":20}}"#).unwrap();
        let u = d.usage.clone().unwrap();
        assert_eq!(u.input_tokens, 175);
        assert_eq!(u.output_tokens, 20);
        assert!(d.finish().unwrap().is_empty());
        let p = ModelPricing { currency: "USD".into(), tiers: vec![PriceTier { min_input_tokens: 0, input: 2.0, output: 4.0, cache_read: Some(0.5), cache_write: Some(3.0) }] };
        assert!((p.cost(&u).unwrap() - 0.00038).abs() < 1e-12);
        c.settings.native_web_search = false;
        c.protocol = Protocol::OpenAiChat;
        assert!(c.settings.validate(c.protocol).is_err());
        c.settings.reasoning = Some(ReasoningConfig { dialect: ReasoningDialect::Qwen, efforts: vec![ReasoningEffort::Low], budget_tokens: Some(2000) });
        let body = encode(&c, None, &[], &[]);
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["enable_thinking"], true);
        assert_eq!(body["thinking_budget"], 200);
        c.settings.reasoning.as_mut().unwrap().dialect = ReasoningDialect::ThinkingType;
        c.settings.reasoning.as_mut().unwrap().budget_tokens = None;
        assert_eq!(encode(&c,None,&[],&[])["thinking"]["type"], "enabled");
        c.settings.supports_streaming = false;
        let body = encode(&c,None,&[],&[]);
        assert_eq!(body["stream"], false);
        assert!(body.get("stream_options").is_none());
        let mut decoder = Decoder::default();
        assert_eq!(decoder.complete(Protocol::OpenAiChat, &json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).unwrap(), "done");
        assert_eq!(decoder.usage.unwrap().output_tokens, 2);
        let mut decoder = Decoder::default();
        assert_eq!(decoder.complete(Protocol::OpenAiResponses, &json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":2}})).unwrap(), "done");
        assert_eq!(decoder.usage.unwrap().input_tokens, 10);
        let mut decoder = Decoder::default();
        assert_eq!(decoder.complete(Protocol::AnthropicMessages, &json!({"stop_reason":"end_turn","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":2}})).unwrap(), "done");
        c.settings.reasoning = Some(ReasoningConfig { dialect: ReasoningDialect::AnthropicAdaptive, efforts:vec![ReasoningEffort::High],budget_tokens:None });
        c.protocol = Protocol::AnthropicMessages;
        assert_eq!(encode(&c,None,&[],&[])["thinking"]["type"], "adaptive");
    }
}

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
            (Protocol::AnthropicMessages, Message::User(text)) => {
                encoded.push(json!({"role":"user", "content": text}));
            }
            (Protocol::AnthropicMessages, Message::Assistant { text, tool_calls }) => {
                let mut content = Vec::new();
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
            (_, Message::User(text)) => encoded.push(json!({"role":"user", "content":text})),
            (_, Message::Assistant { text, tool_calls })
                if connection.protocol == Protocol::OpenAiResponses =>
            {
                if !text.is_empty() || tool_calls.is_empty() {
                    encoded.push(json!({"role":"assistant", "content":text}));
                }
                encoded.extend(tool_calls.iter().map(|call| {
                    json!({
                        "type":"function_call", "call_id":call.id, "name":call.name,
                        "arguments":call.arguments.to_string()
                    })
                }));
            }
            (_, Message::Assistant { text, tool_calls }) => {
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
    match connection.protocol {
        Protocol::AnthropicMessages => {
            let mut body = json!({"model":connection.model,"max_tokens":4096,"stream":true,"messages":encoded});
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
            let mut body = json!({"model":connection.model,"max_tokens":4096,"stream":true,"stream_options":{"include_usage":true},"messages":encoded});
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(|tool| json!({
                "type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.parameters}
            })).collect();
            }
            body
        }
        Protocol::OpenAiResponses => {
            let mut body = json!({"model":connection.model,"max_output_tokens":4096,"stream":true,"input":encoded});
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
    }
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
}
struct PartialCall {
    key: Value,
    id: String,
    name: String,
    arguments: String,
}
impl Decoder {
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
        match protocol {
            Protocol::AnthropicMessages => self.anthropic(&value),
            Protocol::OpenAiChat => self.chat(&value),
            Protocol::OpenAiResponses => self.responses(&value),
        }
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
        };
        let body = encode(
            &connection,
            Some("system"),
            &[Message::User("hello".into())],
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
}

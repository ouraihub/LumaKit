//! 输入控制和附件快照；与聊天历史在同一 SQLite 事务提交。
use super::{AttemptKind, ChatMessage, ChatSession, ImageRef, MAX_TEXT, ModelRef, Turn};
use crate::{history::TurnStatus, images, model_config::ReasoningEffort};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// 可固定到输入区工具栏的操作；只控制显示，不改变请求设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolbarTool {
    /// 新建对话。
    NewConversation,
    /// 原生网络搜索。
    WebSearch,
    /// 上传附件。
    Attachment,
    /// 生成图片。
    GenerateImage,
}

fn default_toolbar() -> Vec<ToolbarTool> {
    vec![ToolbarTool::NewConversation, ToolbarTool::WebSearch]
}

fn canonical_toolbar(tools: &[ToolbarTool]) -> Vec<ToolbarTool> {
    let mut ordered = Vec::with_capacity(tools.len());
    for tool in tools {
        if !ordered.contains(tool) {
            ordered.push(*tool);
        }
    }
    ordered
}

fn deserialize_toolbar<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ToolbarTool>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    // 新宿主保存的未知工具只忽略该项，不能让整份草稿和历史读取失败。
    let tools: Vec<ToolbarTool> = values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    Ok(canonical_toolbar(&tools))
}

/// 每个对话保存的输入控制；None 不指定推理强度，使用服务商默认行为。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComposerSettings {
    /// 使用服务商原生联网搜索。
    pub web_search: bool,
    /// 显式推理强度；None 不发送推理参数，Some(ReasoningEffort::None) 仅在模型支持时关闭推理。
    pub reasoning: Option<ReasoningEffort>,
    /// 向聊天模型公开绘图工具；保留原字段名以兼容已保存控制。
    pub image_mode: bool,
    /// 图片生成使用的模型。
    pub image_model: Option<ModelRef>,
}

/// 已读取并持久化的附件；不再依赖原文件路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// 稳定附件标识。
    pub id: String,
    /// 原文件名。
    pub name: String,
    /// 内容 MIME。
    pub mime: String,
    /// 原文件字节数。
    pub bytes: u64,
    text: Option<String>,
    image: Option<ImageRef>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct InputSnapshot {
    pub settings: ComposerSettings,
    pub attachments: Vec<Attachment>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ComposerData {
    pub drafts: BTreeMap<String, InputSnapshot>,
    pub turns: BTreeMap<String, InputSnapshot>,
    #[serde(default = "default_toolbar", deserialize_with = "deserialize_toolbar")]
    pub toolbar: Vec<ToolbarTool>,
    #[serde(default)]
    pub model_picker: super::ModelPickerPreferences,
    #[serde(default)]
    pub default_composer: ComposerSettings,
    #[serde(default)]
    pub conversation_models: BTreeMap<String, ModelRef>,
}

impl Default for ComposerData {
    fn default() -> Self {
        Self {
            drafts: BTreeMap::new(),
            turns: BTreeMap::new(),
            toolbar: default_toolbar(),
            model_picker: Default::default(),
            default_composer: Default::default(),
            conversation_models: BTreeMap::new(),
        }
    }
}

impl ChatSession {
    /// 所有对话共用的工具栏显示偏好。
    pub fn toolbar(&self) -> &[ToolbarTool] {
        &self.composer_data.toolbar
    }
    /// 保存工具栏显示偏好；不创建对话或更改输入控制，失败时撤销。
    pub fn set_toolbar(&mut self, tools: Vec<ToolbarTool>) {
        if !self.mutable() {
            return;
        }
        let tools = canonical_toolbar(&tools);
        if self.composer_data.toolbar == tools {
            return;
        }
        let before = std::mem::replace(&mut self.composer_data.toolbar, tools);
        if !self.save() {
            self.composer_data.toolbar = before;
        }
    }
    /// 当前对话的输入控制。
    pub fn composer(&self) -> &ComposerSettings {
        self.history
            .selected
            .as_ref()
            .and_then(|id| self.composer_data.drafts.get(id))
            .map(|draft| &draft.settings)
            .unwrap_or(&self.default_composer)
    }
    /// 更新当前对话控制；保存失败时撤销修改。
    pub fn set_composer(&mut self, settings: ComposerSettings) {
        if !self.mutable() {
            return;
        }
        self.ensure_conversation();
        if !self.mutable() {
            return;
        }
        let Some(id) = self.history.selected.clone() else {
            return;
        };
        let before = self.composer_data.clone();
        self.composer_data.drafts.entry(id).or_default().settings = settings;
        if !self.save() {
            self.composer_data = before;
        }
    }
    /// 当前草稿的附件。
    pub fn draft_attachments(&self) -> &[Attachment] {
        self.history
            .selected
            .as_ref()
            .and_then(|id| self.composer_data.drafts.get(id))
            .map_or(&[], |draft| draft.attachments.as_slice())
    }
    /// 已发送问题的附件，包括重启后和重试时使用的快照。
    pub fn turn_attachments(&self, turn_id: &str) -> &[Attachment] {
        self.composer_data
            .turns
            .get(turn_id)
            .map_or(&[], |turn| turn.attachments.as_slice())
    }
    /// 有界读取 UTF-8 文本或 PNG/JPEG/WebP/GIF；不截断超限文件。
    ///
    /// # Errors
    /// Returns invalid or oversized attachments, media I/O, busy session or persistence errors.
    pub fn add_attachment(&mut self, path: &Path) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        self.ensure_conversation();
        if !self.mutable() {
            return Err(self.notice.clone());
        }
        let id = self.history.selected.clone().ok_or("请先新建对话。")?;
        if self.draft_attachments().len() >= images::MAX_IMAGES {
            return Err("每个问题最多 4 个附件。".into());
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.chars().any(char::is_control))
            .ok_or("附件文件名无效。")?
            .to_owned();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mime = match extension.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "webp" => Some("image/webp"),
            "gif" => Some("image/gif"),
            _ => None,
        };
        let limit = if mime.is_some() {
            images::MAX_IMAGE_BYTES
        } else {
            MAX_TEXT
        };
        let file = std::fs::File::open(path).map_err(|_| "无法读取附件文件。")?;
        if !file.metadata().map_err(|_| "无法读取附件信息。")?.is_file() {
            return Err("附件必须是普通文件。".into());
        }
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "读取附件失败。")?;
        if bytes.is_empty() || bytes.len() > limit {
            return Err(if mime.is_some() {
                "图片为空或超过 4 MiB。"
            } else {
                "文本为空或超过 1 MiB；不会截断附件。"
            }
            .into());
        }
        let length = bytes.len() as u64;
        let attachment_id = lumakit_support::new_id().map_err(|_| "无法生成附件标识。")?;
        let (mime, text, image) = if let Some(mime) = mime {
            images::from_bytes(mime, &bytes).map_err(|error| format!("图片校验失败：{error}"))?;
            std::fs::create_dir_all(&self.media_root).map_err(|_| "无法创建附件媒体目录。")?;
            let key = format!("{attachment_id}.{extension}");
            let path = self.media_root.join(&key);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| "无法保存图片附件。")?;
            if file
                .write_all(&bytes)
                .and_then(|()| file.sync_all())
                .is_err()
            {
                drop(file);
                super::diagnostics::remove_media(&path);
                return Err("无法完整保存图片附件。".into());
            }
            let image = ImageRef {
                id: attachment_id.clone(),
                mime: mime.into(),
                bytes: length,
                width: None,
                height: None,
                storage_key: key,
            };
            (mime.to_owned(), None, Some(image))
        } else {
            let text = String::from_utf8(bytes)
                .map_err(|_| "附件必须是 UTF-8 文本或支持的图片；不支持二进制文件。")?;
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err("附件含二进制或不支持的控制字符。".into());
            }
            let total: usize = self
                .draft_attachments()
                .iter()
                .filter_map(|attachment| attachment.text.as_ref())
                .map(String::len)
                .sum();
            if text.len() > MAX_TEXT.saturating_sub(total) {
                return Err("文本附件合计超过 1 MiB；不会截断附件。".into());
            }
            (
                if matches!(extension.as_str(), "md" | "markdown") {
                    "text/markdown"
                } else {
                    "text/plain"
                }
                .to_owned(),
                Some(text),
                None,
            )
        };
        let attachment = Attachment {
            id: attachment_id,
            name,
            mime,
            bytes: length,
            text,
            image,
        };
        let before = self.composer_data.clone();
        let media_key = attachment
            .image
            .as_ref()
            .map(|image| image.storage_key.clone());
        self.composer_data
            .drafts
            .entry(id)
            .or_default()
            .attachments
            .push(attachment);
        if self.save() {
            Ok(())
        } else {
            self.composer_data = before;
            if let Some(key) = media_key {
                super::diagnostics::remove_media(&self.media_root.join(key));
            }
            Err(self.notice.clone())
        }
    }
    /// 删除当前草稿附件；保存失败时撤销。
    pub fn remove_attachment(&mut self, id: &str) {
        if !self.mutable() {
            return;
        }
        if !self
            .draft_attachments()
            .iter()
            .any(|attachment| attachment.id == id)
        {
            return;
        }
        let Some(conversation) = self.history.selected.clone() else {
            return;
        };
        let before = self.composer_data.clone();
        if let Some(draft) = self.composer_data.drafts.get_mut(&conversation) {
            draft.attachments.retain(|attachment| attachment.id != id);
        }
        if !self.save() {
            self.composer_data = before;
        }
    }
    /// 仅作预览缓存的范围身份，避免不同数据目录的同 ID 图片串图；读取仍须经过 media_path 校验。
    pub fn media_scope(&self) -> &Path {
        &self.media_root
    }
    /// 只允许打开历史已登记且位于受控目录内的媒体。
    ///
    /// # Errors
    /// Returns unknown, invalid or unavailable media instead of opening an arbitrary path.
    pub fn media_path(&self, id: &str) -> Result<PathBuf, String> {
        let image = self
            .history
            .conversations
            .iter()
            .flat_map(|conversation| &conversation.turns)
            .flat_map(|turn| &turn.attempts)
            .flat_map(|attempt| &attempt.generated)
            .chain(
                self.composer_data
                    .drafts
                    .values()
                    .chain(self.composer_data.turns.values())
                    .flat_map(|snapshot| &snapshot.attachments)
                    .filter_map(|attachment| attachment.image.as_ref()),
            )
            .find(|image| image.id == id)
            .ok_or("媒体不存在。")?;
        read_media(&self.media_root, image).map(|(path, _)| path)
    }
}

pub(super) fn user_message(
    input: &str,
    attachments: &[Attachment],
    vision: bool,
    root: &Path,
) -> Result<ChatMessage, String> {
    let mut text = input.to_owned();
    let mut parts = Vec::new();
    for attachment in attachments {
        if let Some(content) = &attachment.text {
            text.push_str(&format!("\n\n--- 附件：{} ---\n", attachment.name));
            text.push_str(content);
            if text.len() > MAX_TEXT {
                return Err("问题和文本附件合计超过 1 MiB；草稿已保留。".into());
            }
        }
        if let Some(image) = &attachment.image {
            if !vision {
                return Err("所选模型不支持视觉输入，请移除图片或选择视觉模型。".into());
            }
            parts.push(read_media(root, image)?.1);
        }
    }
    if text.trim().is_empty() && parts.is_empty() {
        return Err("请输入问题或添加附件。".into());
    }
    if text.len() > MAX_TEXT {
        return Err("问题超过 1 MiB；草稿已保留。".into());
    }
    if parts.is_empty() {
        Ok(ChatMessage::User(text))
    } else {
        images::validate(&parts).map_err(|error| error.to_string())?;
        Ok(ChatMessage::UserImages {
            text,
            images: parts,
        })
    }
}

pub(super) fn context(
    turns: &[Turn],
    input: &str,
    attachments: &[Attachment],
    data: &ComposerData,
    vision: bool,
    root: &Path,
) -> Result<Vec<ChatMessage>, String> {
    let mut messages = Vec::new();
    let mut bytes = 0usize;
    for turn in turns {
        let selected = turn
            .selected_attempt
            .as_ref()
            .and_then(|id| turn.attempts.iter().find(|attempt| &attempt.id == id));
        let attempt = selected.or_else(|| turn.attempts.last()).filter(|attempt| {
            attempt.status == TurnStatus::Completed
                && attempt.kind == AttemptKind::Reply
                && (!attempt.text.trim().is_empty()
                    || (attempt.generated.is_empty()
                        && !data
                            .turns
                            .get(&turn.id)
                            .is_some_and(|snapshot| snapshot.settings.image_mode)))
        });
        if let Some(attempt) = attempt {
            if attempt.text.trim().is_empty() {
                return Err("已完成的历史回答为空，无法形成有效上下文。".into());
            }
            let attachments = data
                .turns
                .get(&turn.id)
                .map_or(&[][..], |snapshot| snapshot.attachments.as_slice());
            messages.push(user_message(&turn.input, attachments, vision, root)?);
            messages.push(ChatMessage::Assistant(attempt.text.clone()));
        }
    }
    messages.push(user_message(input, attachments, vision, root)?);
    for message in &messages {
        let text = match message {
            ChatMessage::User(text)
            | ChatMessage::Assistant(text)
            | ChatMessage::UserImages { text, .. } => text,
        };
        bytes = bytes.saturating_add(text.len());
        if bytes > MAX_TEXT {
            return Err(
                "完整上下文超过 1 MiB；不会自动截断，请新建对话后继续。草稿已保留。".into(),
            );
        }
    }
    Ok(messages)
}

fn read_media(root: &Path, image: &ImageRef) -> Result<(PathBuf, images::ImagePart), String> {
    let root = root.canonicalize().map_err(|_| "媒体目录不存在。")?;
    if Path::new(&image.storage_key).components().count() != 1 {
        return Err("媒体存储键无效。".into());
    }
    let path = root
        .join(&image.storage_key)
        .canonicalize()
        .map_err(|_| "媒体文件不存在。")?;
    if path.parent() != Some(root.as_path()) {
        return Err("媒体路径超出数据目录。".into());
    }
    let file = std::fs::File::open(&path).map_err(|_| "无法读取媒体。")?;
    if !file.metadata().map_err(|_| "无法读取媒体信息。")?.is_file() {
        return Err("媒体必须是普通文件。".into());
    }
    let mut bytes = Vec::new();
    file.take((images::MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取媒体。")?;
    if bytes.len() as u64 != image.bytes {
        return Err("媒体大小与保存的引用不符。".into());
    }
    let part = images::from_bytes(image.mime.clone(), &bytes).map_err(|_| "媒体内容校验失败。")?;
    Ok((path, part))
}

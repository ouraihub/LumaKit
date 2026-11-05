//! 模型选择、偏好和输入推理设置的原子持久化边界。
use super::{ChatSession, ModelOption, ModelRef};
use crate::model_config::ReasoningEffort;
use serde::{Deserialize, Serialize};

/// 模型完整来源与推理选择；`None` 使用服务商默认行为，`Some(ReasoningEffort::None)` 在支持时显式关闭。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPair {
    /// 包含提供方、端点、模型标识和协议的完整身份。
    pub model: ModelRef,
    /// 显式强度；空值不发送推理参数，交由服务商决定。
    pub effort: Option<ReasoningEffort>,
}

/// 所有对话共用并随输入状态持久化的模型选择器偏好。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ModelPickerPreferences {
    /// 按收藏顺序保存的完整模型身份。
    pub favorites: Vec<ModelRef>,
    /// 最近选择优先的唯一模型身份，最多十项；收藏不从此列表删除。
    pub recents: Vec<ModelRef>,
    /// 仅新建对话时应用，不改变已存在对话或已发送请求快照。
    pub default_pair: Option<ModelPair>,
}

fn supports_effort(option: &ModelOption, effort: Option<ReasoningEffort>) -> bool {
    effort.is_none_or(|effort| {
        crate::model_reference::effective_settings(&option.model.model, option.model.protocol, &option.settings)
            .reasoning.as_ref().is_some_and(|config| config.efforts.contains(&effort))
    })
}

fn validate_pair(option: &ModelOption, effort: Option<ReasoningEffort>) -> Result<(), String> {
    if [
        &option.model.provider,
        &option.model.endpoint,
        &option.model.model,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Err("模型来源或模型标识为空。".into());
    }
    if !supports_effort(option, effort) {
        return Err("所选模型不支持指定的推理强度。".into());
    }
    Ok(())
}

impl ChatSession {
    /// 返回已持久化的全局收藏、最近模型和新对话默认模型对。
    pub fn model_picker_preferences(&self) -> &ModelPickerPreferences {
        &self.composer_data.model_picker
    }

    /// 在一次 SQLite 事务中保存模型、草稿推理强度和最近模型；不创建对话。
    /// `None` 不发送推理参数，交由服务商决定，不表示关闭推理；无对话时保存在待用输入控制中。
    ///
    /// # Errors
    /// 忙碌、尚有保存错误、身份无效、显式强度不受支持或保存失败时返回错误；不改变原状态。
    pub fn select_model_pair(
        &mut self,
        option: &ModelOption,
        effort: Option<ReasoningEffort>,
    ) -> Result<(), String> {
        validate_pair(option, effort)?;
        self.update_model_picker(|session| session.apply_model_pair(option.model.clone(), effort))
    }

    /// 按完整身份切换收藏；新增项追加，移除不改变其它项顺序；保存失败时撤销。
    /// 不改变当前模型、草稿或已发送请求。
    pub fn toggle_model_favorite(&mut self, model: ModelRef) {
        let _ = self.update_model_picker(|session| {
            let favorites = &mut session.composer_data.model_picker.favorites;
            if let Some(index) = favorites.iter().position(|favorite| favorite == &model) {
                favorites.remove(index);
            } else {
                favorites.push(model);
            }
        });
    }

    /// 保存新对话的默认模型与强度，不修改当前选择、草稿或最近模型。
    ///
    /// # Errors
    /// 忙碌、尚有保存错误、身份无效、显式强度不受支持或保存失败时返回错误并撤销修改。
    pub fn set_default_model_pair(
        &mut self,
        option: &ModelOption,
        effort: Option<ReasoningEffort>,
    ) -> Result<(), String> {
        validate_pair(option, effort)?;
        self.update_model_picker(|session| {
            session.composer_data.model_picker.default_pair = Some(ModelPair {
                model: option.model.clone(),
                effort,
            });
        })
    }

    /// 用完整身份裁剪已消失的偏好引用，将当前已不支持的显式强度恢复为服务商默认行为。
    /// 默认模型仍存在但强度不再支持时也清除显式选择；不猜测替代模型，不更改已发送请求快照。
    ///
    /// # Errors
    /// 忙碌、尚有保存错误或保存失败时返回错误，所有内存状态保持修改前的值。
    pub fn reconcile_model_catalog(&mut self, options: &[ModelOption]) -> Result<(), String> {
        if !self.mutable() {
            return Err(if self.save_error {
                self.notice.clone()
            } else {
                "当前聊天不可修改。".into()
            });
        }
        let mut preferences = self.composer_data.model_picker.clone();
        let live = |model: &ModelRef| options.iter().find(|option| &option.model == model);
        preferences.favorites.retain(|model| live(model).is_some());
        preferences.recents.retain(|model| live(model).is_some());
        if let Some(pair) = &mut preferences.default_pair {
            if let Some(option) = live(&pair.model) {
                if !supports_effort(option, pair.effort) {
                    pair.effort = None;
                }
            } else {
                preferences.default_pair = None;
            }
        }
        let effort = self.composer().reasoning;
        let valid = effort.is_none()
            || self
                .history
                .model
                .as_ref()
                .and_then(live)
                .is_some_and(|option| supports_effort(option, effort));
        if preferences == self.composer_data.model_picker && valid {
            return Ok(());
        }
        self.update_model_picker(|session| {
            session.composer_data.model_picker = preferences;
            if !valid {
                session.set_picker_effort(None);
            }
        })
    }

    pub(super) fn remember_conversation_model(&mut self) {
        if let (Some(id), Some(model)) = (&self.history.selected, &self.history.model) {
            if self.composer_data.conversation_models.get(id) != Some(model) {
                self.composer_data
                    .conversation_models
                    .insert(id.clone(), model.clone());
            }
        }
    }

    pub(super) fn apply_model_pair(&mut self, model: ModelRef, effort: Option<ReasoningEffort>) {
        self.history.model = Some(model.clone());
        self.set_picker_effort(effort);
        self.remember_conversation_model();
        let recents = &mut self.composer_data.model_picker.recents;
        recents.retain(|recent| recent != &model);
        recents.insert(0, model);
        recents.truncate(10);
    }

    fn set_picker_effort(&mut self, effort: Option<ReasoningEffort>) {
        if let Some(id) = self
            .history
            .selected
            .clone()
            .filter(|_| self.selected().is_some())
        {
            // 旧对话没有草稿快照时，首次沿用待用输入控制。
            let settings = &self.default_composer;
            self.composer_data
                .drafts
                .entry(id)
                .or_insert_with(|| super::composer::InputSnapshot {
                    settings: settings.clone(),
                    attachments: Vec::new(),
                })
                .settings
                .reasoning = effort;
        } else {
            self.default_composer.reasoning = effort;
        }
    }

    // 此边界只修改选择器/输入状态、当前选择或追加新对话，
    // 回滚仅需保留会触及的历史字段，不复制所有回答正文。
    pub(super) fn update_model_picker(
        &mut self,
        update: impl FnOnce(&mut Self),
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err(if self.save_error {
                self.notice.clone()
            } else {
                "当前聊天不可修改。".into()
            });
        }
        let model = self.history.model.clone();
        let selected = self.history.selected.clone();
        let conversations = self.history.conversations.len();
        let composer = self.composer_data.clone();
        let default_composer = self.default_composer.clone();
        update(self);
        if self.save() {
            return Ok(());
        }
        self.history.model = model;
        self.history.selected = selected;
        self.history.conversations.truncate(conversations);
        self.composer_data = composer;
        self.default_composer = default_composer;
        Err(self.notice.clone())
    }
}

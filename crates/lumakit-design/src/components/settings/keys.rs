//! Saved API-key controls shared by settings pages, after ThinkRail Native's provider key pane
//! and keys dialog: a key value box with icon actions, the key manager body, its entry button and
//! the delete confirmation. Stateless: hosts own secrets, drafts and storage; every intent is a
//! callback. All actions are Kit buttons, so they take keyboard focus and expose names and roles.

use gpui_kit::{
    App, ClickEvent, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window,
    assets::IconName,
    base::{h_flex, v_flex},
    component::{
        Disableable as _, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    prelude::FluentBuilder as _,
    px,
};

use super::{field, label};
use crate::{
    Roles,
    components::{Handler, roles},
    metrics::{radius, size, space, text},
};

/// One saved key: its masked (or explicitly revealed) value in a bordered box with icon actions.
///
/// The eye is always shown; copy, edit and delete appear once revealed or with
/// [`KeyRow::all_actions`]. An action without a handler is not drawn. `id` prefixes the ids of
/// the row's buttons, so it must be unique on the page.
#[derive(IntoElement)]
pub struct KeyRow {
    id: SharedString,
    value: SharedString,
    revealed: bool,
    more: usize,
    all_actions: bool,
    selected: Option<bool>,
    disabled: bool,
    on_reveal: Option<Handler>,
    on_copy: Option<Handler>,
    on_edit: Option<Handler>,
    on_delete: Option<Handler>,
    on_select: Option<Handler>,
}

impl KeyRow {
    /// `value` is the masked key, or the plaintext while `revealed`.
    pub fn new(
        id: impl Into<SharedString>,
        value: impl Into<SharedString>,
        revealed: bool,
    ) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            revealed,
            more: 0,
            all_actions: false,
            selected: None,
            disabled: false,
            on_reveal: None,
            on_copy: None,
            on_edit: None,
            on_delete: None,
            on_select: None,
        }
    }

    /// How many other keys exist; shown as `+N` after the value when non-zero.
    pub fn more(mut self, more: usize) -> Self {
        self.more = more;
        self
    }

    /// Shows copy, edit and delete even while the value is masked (the key manager's rows).
    pub fn all_actions(mut self, all: bool) -> Self {
        self.all_actions = all;
        self
    }

    /// Disables edit, delete and select; reveal and copy stay available.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_reveal(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reveal = Some(Box::new(handler));
        self
    }

    pub fn on_copy(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy = Some(Box::new(handler));
        self
    }

    pub fn on_edit(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_edit = Some(Box::new(handler));
        self
    }

    pub fn on_delete(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_delete = Some(Box::new(handler));
        self
    }

    /// Adds the 使用此密钥 toggle, checked when `selected` is the preferred key.
    pub fn on_select(
        mut self,
        selected: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.selected = Some(selected);
        self.on_select = Some(Box::new(handler));
        self
    }
}

fn action(id: SharedString, icon: IconName, name: &'static str, handler: Handler) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon)
        .accessibility_label(name)
        .tooltip(name)
        .on_click(handler)
}

impl RenderOnce for KeyRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let Self {
            id,
            value,
            revealed,
            more,
            all_actions,
            selected,
            disabled,
            on_reveal,
            on_copy,
            on_edit,
            on_delete,
            on_select,
        } = self;
        let child_id = |suffix: &str| SharedString::from(format!("{id}-{suffix}"));
        let mut actions = h_flex().flex_none().gap(space::S2);
        if let Some(handler) = on_reveal {
            let (icon, name) = if revealed {
                (IconName::EyeOff, "隐藏密钥")
            } else {
                (IconName::Eye, "显示密钥")
            };
            actions = actions.child(action(child_id("reveal"), icon, name, handler));
        }
        if revealed || all_actions {
            if let Some(handler) = on_copy {
                actions = actions.child(action(
                    child_id("copy"),
                    IconName::Copy,
                    "复制密钥",
                    handler,
                ));
            }
            if let Some(handler) = on_edit {
                actions = actions.child(
                    action(child_id("edit"), IconName::Pencil, "修改密钥", handler)
                        .disabled(disabled),
                );
            }
            if let Some(handler) = on_delete {
                actions = actions.child(
                    action(child_id("delete"), IconName::Trash, "删除密钥…", handler)
                        .disabled(disabled),
                );
            }
        }
        h_flex()
            .flex_1()
            .min_w(px(0.))
            .h(size::SETTINGS_ROW)
            .px(space::S8)
            .gap(space::S8)
            .rounded(radius::MD)
            .border_1()
            .border_color(r.border_default)
            .child(
                label(text::CODE_TEXT, value)
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
            )
            .when(more > 0, |d| {
                d.child(label(text::METADATA, format!("+{more}")).text_color(r.text_subtle))
            })
            .child(actions)
            .when_some(selected.zip(on_select), |d, (selected, handler)| {
                d.child(
                    Button::new(child_id("select"))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Check)
                        .label("使用此密钥")
                        .selected(selected)
                        .disabled(disabled)
                        .on_click(handler),
                )
            })
    }
}

/// The key manager's body: an optional note, the saved [`KeyRow`]s, the add / edit field with its
/// actions. The host wraps it in its dialog (title, 完成, Escape) and owns the `input` entity.
#[derive(IntoElement)]
pub struct KeyManager {
    input: gpui_kit::AnyElement,
    note: Option<SharedString>,
    rows: Vec<KeyRow>,
    editing: bool,
    can_save: bool,
    on_save: Option<Handler>,
    on_cancel_edit: Option<Handler>,
}

impl KeyManager {
    /// `input` is the masked text input for a new key or an edited key's replacement.
    pub fn new(input: impl IntoElement) -> Self {
        Self {
            input: input.into_any_element(),
            note: None,
            rows: Vec::new(),
            editing: false,
            can_save: true,
            on_save: None,
            on_cancel_edit: None,
        }
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = KeyRow>) -> Self {
        self.rows = rows.into_iter().collect();
        self
    }

    /// Whether the field replaces an existing key (修改密钥) instead of adding one.
    pub fn editing(mut self, editing: bool) -> Self {
        self.editing = editing;
        self
    }

    /// Enables the save button (e.g. a non-empty draft below the key limit).
    pub fn can_save(mut self, can_save: bool) -> Self {
        self.can_save = can_save;
        self
    }

    pub fn on_save(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_save = Some(Box::new(handler));
        self
    }

    /// Leaves edit mode without saving; drawn only while editing.
    pub fn on_cancel_edit(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel_edit = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for KeyManager {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = roles(cx);
        let editing = self.editing;
        let mut actions = h_flex().justify_end().gap(space::S8);
        if editing && let Some(handler) = self.on_cancel_edit {
            actions = actions.child(
                Button::new("key-manager-cancel-edit")
                    .outline()
                    .label("取消修改")
                    .on_click(handler),
            );
        }
        if let Some(handler) = self.on_save {
            actions = actions.child(
                Button::new("key-manager-save")
                    .primary()
                    .label(if editing {
                        "保存修改后的密钥"
                    } else {
                        "保存新密钥"
                    })
                    .disabled(!self.can_save)
                    .on_click(handler),
            );
        }
        v_flex()
            .w_full()
            .gap(space::S16)
            .when_some(self.note, |d, note| {
                d.child(label(text::METADATA, note).text_color(r.text_subtle))
            })
            .when(!self.rows.is_empty(), |d| {
                d.child(v_flex().gap(space::S6).children(self.rows))
            })
            .child(
                field(
                    if editing {
                        "修改密钥"
                    } else {
                        "添加密钥"
                    },
                    self.input,
                    &r,
                )
                .when(editing, |d| {
                    d.child(label(text::METADATA, "空草稿不会清空原密钥").text_color(r.text_subtle))
                }),
            )
            .child(actions)
    }
}

/// The outlined key icon button that opens the key manager.
pub fn key_manage_button(id: impl Into<gpui_kit::ElementId>) -> Button {
    Button::new(id)
        .outline()
        .icon(IconName::KeyRound)
        .accessibility_label("密钥管理")
        .tooltip("密钥管理")
}

/// A delete confirmation: title, what goes away, a quiet cancel and a destructive confirm.
pub fn delete_confirm(
    note: impl Into<SharedString>,
    r: &Roles,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .gap(space::S16)
        .child(
            v_flex()
                .gap(space::S4)
                .child(label(text::TITLE_DIALOG, "确认删除"))
                .child(label(text::UI, note).text_color(r.text_muted)),
        )
        .child(
            h_flex()
                .justify_end()
                .gap(space::S8)
                .child(
                    Button::new("cancel-delete")
                        .outline()
                        .label("取消")
                        .on_click(on_cancel),
                )
                .child(
                    Button::new("confirm-delete")
                        .danger()
                        .label("确认删除")
                        .on_click(on_confirm),
                ),
        )
}

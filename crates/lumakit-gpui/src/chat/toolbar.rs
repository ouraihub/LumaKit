//! Composer tool popover: visibility switches and ordered pinned actions.
use super::*;
use gpui_kit::component::{Icon, switch::Switch};

impl ChatPanel {
    pub(super) fn tool_button(
        &self,
        tool: ToolbarTool,
        composer: &ComposerSettings,
        can_image: bool,
        cx: &Context<Self>,
    ) -> Button {
        let (label, action, enabled, selected) = match tool {
            ToolbarTool::NewConversation => ("新对话", ChatAction::New, self.unlocked, false),
            ToolbarTool::Attachment => ("附件", ChatAction::UploadAttachment, self.unlocked, false),
            ToolbarTool::WebSearch => {
                let mut next = composer.clone();
                next.web_search = !next.web_search;
                (
                    "联网",
                    ChatAction::SetComposer(next),
                    self.unlocked,
                    composer.web_search,
                )
            }
            ToolbarTool::GenerateImage => {
                let mut next = composer.clone();
                next.image_mode = !next.image_mode;
                (
                    "生图",
                    ChatAction::SetComposer(next),
                    self.unlocked,
                    composer.image_mode,
                )
            }
        };
        let icon = match tool {
            ToolbarTool::NewConversation => IconName::MessageSquarePlus,
            ToolbarTool::Attachment => IconName::Paperclip,
            ToolbarTool::WebSearch => IconName::Globe,
            ToolbarTool::GenerateImage => IconName::Image,
        };
        icon_button(label, icon, label)
            .disabled(!enabled)
            .selected(selected)
            .when(tool == ToolbarTool::GenerateImage, |button| {
                button.tooltip(if composer.image_mode {
                    "关闭生成图片"
                } else {
                    self.image_notice
                })
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if tool == ToolbarTool::GenerateImage && !selected && !can_image {
                    this.open(
                        Overlay::ImageModels,
                        "tool-GenerateImage".into(),
                        window,
                        cx,
                    );
                } else {
                    this.emit(action.clone(), cx);
                    this.close(window, cx);
                }
            }))
    }
}
pub(super) const TOOLS: &[(ToolbarTool, &str)] = &[
    (ToolbarTool::NewConversation, "新对话"),
    (ToolbarTool::WebSearch, "网络搜索"),
    (ToolbarTool::Attachment, "上传附件"),
    (ToolbarTool::GenerateImage, "生成图片"),
];
pub(super) const TOOL_MENU_WIDTH: Pixels = px(256.);
const TOOL_ROW_HEIGHT: Pixels = px(32.);
const DEFAULT_TOOLS: &[ToolbarTool] = &[ToolbarTool::NewConversation, ToolbarTool::WebSearch];

fn icon(tool: ToolbarTool) -> IconName {
    match tool {
        ToolbarTool::NewConversation => IconName::MessageSquarePlus,
        ToolbarTool::WebSearch => IconName::Globe,
        ToolbarTool::Attachment => IconName::Paperclip,
        ToolbarTool::GenerateImage => IconName::Image,
    }
}
fn title(tool: ToolbarTool) -> &'static str {
    match tool {
        ToolbarTool::NewConversation => "新对话",
        ToolbarTool::WebSearch => "网络搜索",
        ToolbarTool::Attachment => "上传附件",
        ToolbarTool::GenerateImage => "生成图片",
    }
}
fn ordered(pinned: &[ToolbarTool]) -> Vec<ToolbarTool> {
    pinned
        .iter()
        .copied()
        .chain(
            TOOLS
                .iter()
                .map(|(tool, _)| *tool)
                .filter(|tool| !pinned.contains(tool)),
        )
        .collect()
}
fn moved(pinned: &[ToolbarTool], tool: ToolbarTool, to: usize) -> Vec<ToolbarTool> {
    let mut order = ordered(pinned);
    order.retain(|item| *item != tool);
    order.insert(to.min(order.len()), tool);
    order
        .into_iter()
        .filter(|item| pinned.contains(item))
        .collect()
}

#[derive(Clone)]
struct DragTool {
    panel: EntityId,
    tool: ToolbarTool,
}
impl Render for DragTool {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let r = LumaTheme::global(cx).roles;
        row()
            .flex_nowrap()
            .p(space::S8)
            .rounded(radius::MD)
            .bg(r.container_elevated_bg)
            .text_color(r.text_default)
            .text_style(text::UI)
            .child(Icon::new(icon(self.tool)).small())
            .child(title(self.tool))
    }
}

impl ChatPanel {
    pub(super) fn tools_menu(
        &self,
        session: &ChatSession,
        image_options: &[ModelOption],
        can_image: bool,
        cx: &Context<Self>,
    ) -> Div {
        let r = LumaTheme::global(cx).roles;
        let composer = session.composer();
        let valid_image = composer
            .image_model
            .as_ref()
            .is_some_and(|m| image_options.iter().any(|o| same_model(m, &o.model)));
        let mut menu = col().gap(space::S2);
        for &(tool, label) in TOOLS {
            if session.toolbar().contains(&tool) {
                continue;
            }
            menu = menu.child(
                self.tool_button(tool, composer, can_image && valid_image, cx)
                    .w_full()
                    .accessibility_label(label)
                    .child(div().flex_1().text_left().child(label)),
            );
        }
        menu.child(
            col()
                .mt(space::S4)
                .pt(space::S4)
                .border_t_1()
                .border_color(r.border_muted)
                .child(self.menu_layer(
                    "image-model",
                    "选择生图模型",
                    self.unlocked,
                    Overlay::ImageModels,
                    cx,
                ))
                .child(
                    self.menu_layer(
                        "customize",
                        "自定义工具栏",
                        self.unlocked,
                        Overlay::Quick,
                        cx,
                    )
                    .icon(IconName::Settings2),
                )
                .child(
                    Button::new("expand")
                        .small()
                        .ghost()
                        .w_full()
                        .label(if self.expanded {
                            "收起输入"
                        } else {
                            "展开输入"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.expanded = !this.expanded;
                            this.close(window, cx);
                        })),
                ),
        )
    }

    pub(super) fn customize_toolbar(&self, session: &ChatSession, cx: &Context<Self>) -> Div {
        let r = LumaTheme::global(cx).roles;
        let pinned = session.toolbar();
        let mut menu = col().gap(space::S2).child(
            div()
                .px(space::S8)
                .py(space::S6)
                .text_style(text::TITLE_COMPACT)
                .child("自定义工具栏"),
        );
        for (index, tool) in ordered(pinned).into_iter().enumerate() {
            let name = title(tool);
            let on = pinned.contains(&tool);
            let mut next = ordered(pinned);
            next.retain(|item| {
                if *item == tool {
                    !on
                } else {
                    pinned.contains(item)
                }
            });
            let toggle = Switch::new(SharedString::from(format!("pin-{tool:?}")))
                .small()
                .checked(on)
                .color(r.feedback_success)
                .disabled(!self.unlocked)
                .accessibility_label(name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.emit(ChatAction::SetToolbar(next.clone()), cx)
                }));
            let up = moved(pinned, tool, index.saturating_sub(1));
            let down = moved(pinned, tool, index + 1);
            let grip = div()
                .id(SharedString::from(format!("grip-{tool:?}")))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if !this.unlocked {
                        return;
                    }
                    let next = match event.keystroke.key.as_str() {
                        "up" => &up,
                        "down" => &down,
                        _ => return,
                    };
                    cx.stop_propagation();
                    this.emit(ChatAction::SetToolbar(next.clone()), cx);
                }))
                .child(
                    Button::new(SharedString::from(format!("move-{tool:?}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::GripVertical)
                        .accessibility_label(format!("调整{name}顺序"))
                        .tooltip("拖动排序；上下方向键调整")
                        .disabled(!self.unlocked),
                )
                .when(self.unlocked, |d| {
                    d.on_drag(
                        DragTool {
                            panel: cx.entity_id(),
                            tool,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        },
                    )
                });
            let saved = pinned.to_vec();
            menu = menu.child(
                row()
                    .id(SharedString::from(format!("tool-row-{tool:?}")))
                    .flex_nowrap()
                    .h(TOOL_ROW_HEIGHT)
                    .px(space::S6)
                    .gap(space::S8)
                    .rounded(radius::MD)
                    .hover(|s| s.bg(r.control_bg_hovered))
                    .drag_over::<DragTool>(move |style, _, _, _| style.bg(r.control_bg_hovered))
                    .on_drop(cx.listener(move |this, drag: &DragTool, _, cx| {
                        if this.unlocked && drag.panel == cx.entity_id() {
                            this.emit(ChatAction::SetToolbar(moved(&saved, drag.tool, index)), cx);
                        }
                    }))
                    .child(grip)
                    .child(Icon::new(icon(tool)).small().text_color(r.text_muted))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(toggle),
            );
        }
        menu.child(
            col()
                .mt(space::S4)
                .pt(space::S4)
                .border_t_1()
                .border_color(r.border_muted)
                .child(
                    Button::new("reset-toolbar")
                        .small()
                        .ghost()
                        .w_full()
                        .icon(IconName::RotateCcw)
                        .child(div().flex_1().text_left().child("恢复默认"))
                        .accessibility_label("恢复默认")
                        .disabled(!self.unlocked || pinned == DEFAULT_TOOLS)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.emit(ChatAction::SetToolbar(DEFAULT_TOOLS.to_vec()), cx)
                        })),
                ),
        )
    }
}

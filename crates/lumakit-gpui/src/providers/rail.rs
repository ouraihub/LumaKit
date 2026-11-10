//! Provider list geometry and accessible selection. Filtering and persistence remain host-owned.
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{
    Disableable,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    AnyElement, App, Div, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window, px,
};
use lumakit_design::{
    Roles,
    components::{label, settings},
    metrics::{radius, space, text},
};
use std::rc::Rc;

/// Search/filter controls, provider rows and create action in a single shared column.
pub fn rail(
    search: AnyElement,
    filter: AnyElement,
    rows: impl IntoIterator<Item = AnyElement>,
    add: AnyElement,
) -> Div {
    v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(space::S4)
        .child(
            h_flex()
                .min_w(px(0.))
                .gap(space::S6)
                .child(search)
                .child(filter),
        )
        .children(rows)
        .child(add)
}

type Select = Rc<dyn Fn(&mut Window, &mut App)>;

/// Provider row with native keyboard selection and a separate, non-nested menu action.
#[derive(IntoElement)]
pub struct ProviderRow {
    id: ElementId,
    name: SharedString,
    selected: bool,
    enabled: bool,
    keyed: bool,
    disabled: bool,
    menu: AnyElement,
    roles: Roles,
    on_select: Option<Select>,
}

impl ProviderRow {
    /// Makes selection unavailable during a host operation.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Selects the captured provider identity through pointer or keyboard activation.
    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

/// A service provider row with shared status, avatar, selected state and trailing menu.
pub fn provider_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: bool,
    enabled: bool,
    keyed: bool,
    menu: AnyElement,
    r: &Roles,
) -> ProviderRow {
    ProviderRow {
        id: id.into(),
        name: name.into(),
        selected,
        enabled,
        keyed,
        menu,
        roles: *r,
        on_select: None,
        disabled: false,
    }
}

impl RenderOnce for ProviderRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let r = self.roles;
        let select = self.on_select;
        let control = Button::new(self.id)
            .ghost()
            .disabled(self.disabled)
            .accessibility_label(self.name.clone())
            .flex_1()
            .min_w(px(0.))
            .child(settings::avatar(&self.name, &r))
            .child(label(text::UI, self.name).flex_1().min_w(px(0.)).truncate())
            .children(self.enabled.then(|| {
                settings::dot(if self.keyed {
                    r.feedback_success
                } else {
                    r.feedback_warning
                })
            }))
            .on_click(move |_, window, cx| {
                if let Some(handler) = &select {
                    handler(window, cx);
                }
            });
        let row = h_flex()
            .w_full()
            .min_w(px(0.))
            .h(settings::LIST_ROW)
            .rounded(radius::MD)
            .gap(space::S4)
            .child(control)
            .child(self.menu);
        if self.selected {
            row.bg(r.control_bg_selected)
        } else {
            row
        }
    }
}

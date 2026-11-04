//! The 外观 settings page: theme cards, 跟随系统, accent swatches and density. Stateless — the
//! host owns the saved [`Preferences`] and persists what `on_change` hands it.

use std::rc::Rc;

use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div,
    assets::IconName,
    base::{h_flex, v_flex},
    component::{Icon, button::{Button, ButtonVariants as _}, switch::Switch},
    prelude::FluentBuilder as _,
    px,
};

use super::{ACCENTS, Accent, Density, Preferences, theme_name};
use crate::components::{Segment, label, settings as ui};
use crate::metrics::{radius, size, space, text};
use crate::theme::on_accent;
use crate::{LumaTheme, ThemeCatalog};

/// Theme card and accent swatch sizes.
const CARD_W: gpui_kit::Pixels = px(112.);
const CARD_PREVIEW_H: gpui_kit::Pixels = px(42.);
const SWATCH: gpui_kit::Pixels = px(40.);
const SWATCH_DOT: gpui_kit::Pixels = px(24.);

type ChangeHandler = Rc<dyn Fn(Preferences, &mut Window, &mut App)>;

/// The appearance page for `saved`. `on_change` receives the next preferences only when the
/// user changes something; the host saves them and, on success, applies them.
#[derive(IntoElement)]
pub struct AppearancePage {
    catalog: &'static ThemeCatalog,
    saved: Preferences,
    error: Option<SharedString>,
    on_change: ChangeHandler,
}

impl AppearancePage {
    pub fn new(catalog: &'static ThemeCatalog, saved: Preferences, on_change: impl Fn(Preferences, &mut Window, &mut App) + 'static) -> Self {
        Self { catalog, saved, error: None, on_change: Rc::new(on_change) }
    }

    /// A save failure to show under the page.
    pub fn error(mut self, error: Option<impl Into<SharedString>>) -> Self {
        self.error = error.map(Into::into);
        self
    }
}

impl RenderOnce for AppearancePage {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let r = LumaTheme::global(cx).roles;
        let shown = LumaTheme::global(cx).appearance;
        let Self { catalog, saved, error, on_change } = self;
        // Fires only for a real change, so re-picking the current choice never saves.
        let change = {
            let saved = saved.clone();
            Rc::new(move |next: Preferences, window: &mut Window, cx: &mut App| if next != saved { on_change(next, window, cx) })
        };

        let mut themes = h_flex().flex_wrap().gap(space::S8).py(space::S12);
        for (i, theme) in catalog.list().iter().enumerate() {
            let selected = saved.is_selected(theme);
            let name = theme_name(theme).to_owned();
            let next = saved.pick_theme(theme);
            let change = change.clone();
            themes = themes.child(Button::new(("theme", i)).ghost().p_0().h_auto().w(CARD_W).rounded(radius::MD)
                .border_1().border_color(if selected { r.primary } else { r.border_muted })
                .accessibility_label(name.clone()).toggled(selected)
                .child(v_flex().w_full()
                    .child(h_flex().h(CARD_PREVIEW_H).w_full().px(space::S8).gap(space::S4).rounded_t(radius::MD)
                        .bg(theme.palette.background).border_b_1().border_color(theme.palette.border)
                        .child(div().size(space::S12).rounded_full().bg(theme.palette.accent))
                        .child(div().h(space::S4).flex_1().rounded(radius::XS).bg(theme.palette.muted)))
                    .child(h_flex().w_full().px(space::S8).py(space::S6).gap(space::S4)
                        .child(label(text::METADATA, name).flex_1().min_w(px(0.)).text_color(if selected { r.text_default } else { r.text_muted }))
                        .when(selected, |d| d.child(Icon::new(IconName::Check).size(size::ICON_SMALL).text_color(r.primary)))))
                .on_click(move |_, window, cx| change(next.clone(), window, cx)));
        }

        let follow = {
            let (saved, change) = (saved.clone(), change.clone());
            Switch::new("theme-follow-system").checked(saved.follow_system).accessibility_label("跟随系统")
                .on_click(move |checked: &bool, window, cx| change(saved.set_follow(catalog, *checked, window.appearance()), window, cx))
        };
        let follow_note = if saved.follow_system { "浅色与深色各选一个主题，随系统外观切换。" } else { "开启后按系统浅色 / 深色外观切换主题。" };

        let mut accents = h_flex().flex_wrap().gap(space::S8).py(space::S12);
        for (i, (value, name)) in ACCENTS.into_iter().enumerate() {
            let selected = saved.accent == value;
            let fill = value.preset().map_or(r.primary, |preset| preset.color(shown));
            let swatch = div().size(SWATCH_DOT).rounded_full().flex().items_center().justify_center().bg(fill)
                .when(value == Accent::Theme, |d| d.border_1().border_color(r.border_default))
                .when(selected, |d| d.child(Icon::new(IconName::Check).size(size::ICON_SMALL).text_color(on_accent(fill))));
            let next = Preferences { accent: value, ..saved.clone() };
            let change = change.clone();
            accents = accents.child(Button::new(("accent", i)).ghost().p_0().size(SWATCH).rounded_full()
                .border_2().border_color(if selected { r.text_default.into() } else { gpui_kit::transparent_black() })
                .accessibility_label(name).tooltip(name).toggled(selected).child(swatch)
                .on_click(move |_, window, cx| change(next.clone(), window, cx)));
        }

        let density = {
            let (saved, change) = (saved.clone(), change.clone());
            Segment::new("density", ["舒适", "紧凑"], usize::from(saved.density == Density::Compact)).accessibility_label("界面密度")
                .on_select(move |index: &usize, window, cx| {
                    let density = if *index == 1 { Density::Compact } else { Density::Comfortable };
                    change(Preferences { density, ..saved.clone() }, window, cx);
                })
        };

        v_flex().id("appearance-settings").w_full().max_w(size::CONTENT_MAX).gap(space::S24)
            .child(ui::page_header("外观", Some("选择后立即保存，应用于整个工作区。".into()), &r))
            .child(ui::group("主题", None, &r).child(themes).child(ui::row("跟随系统", Some(follow_note.into()), follow, &r)))
            .child(ui::group("强调色", Some("主题默认使用所选主题自带的强调色。".into()), &r).child(accents))
            .child(ui::group("密度", None, &r).child(ui::row("界面密度", Some("紧凑同时缩小全部字号。".into()), density, &r)))
            .children(error.map(|error| ui::notice(error, ui::Tone::Error, &r)))
    }
}

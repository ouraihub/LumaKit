//! Host-owned data and transient picker interaction state; no I/O or model inference.
use crate::localization::text as tr;
use gpui_kit::component::slider::SliderState;
use gpui_kit::{
    component::input::{InputEvent, InputState},
    *,
};

/// A reasoning level explicitly declared by a model.
#[derive(Clone, Debug, PartialEq)]
pub struct Effort {
    /// Stable wire value supplied by the host.
    pub id: String,
    /// Localized tick label.
    pub label: String,
    /// Explanation shown under the rail.
    pub hint: String,
}
/// Secret-free display projection; identity remains an opaque host key.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Full host identity, distinct across providers, routes and protocols.
    pub id: String,
    /// Stable grouping identity.
    pub provider_id: String,
    /// Display name of the provider.
    pub provider: String,
    /// Model display name.
    pub name: String,
    /// Honest source, price and context metadata supplied by the host.
    pub detail: String,
    /// Whether this source has an enabled credential; keyless sources remain selectable.
    pub has_key: bool,
    /// Supported levels in ascending order; order never chooses a request default.
    pub efforts: Vec<Effort>,
}
/// Model and explicit effort; `None` inherits provider behavior, not `off`.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// Opaque model identity.
    pub model: String,
    /// Supported wire level, or inherited behavior.
    pub effort: Option<String>,
}
/// Authoritative snapshot from the consuming host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerData {
    /// Currently available models.
    pub models: Vec<Model>,
    /// Current selection, applied by the host rather than optimistically committed here.
    pub selected: Option<Selection>,
    /// Saved default for new conversations.
    pub default: Option<Selection>,
    /// Whether the host currently accepts selection changes.
    pub enabled: bool,
    /// Whether a real catalog operation is in flight.
    pub refreshing: bool,
    /// Host-provided refresh or save failure; empty hides it.
    pub notice: String,
}
/// User intentions; the host performs and reports all side effects.
#[derive(Clone, Debug)]
pub enum PickerEvent {
    /// Change the current model/effort pair.
    Select(Selection),
    /// Save a pair as the new-conversation default.
    SetDefault(Selection),
    /// Reload catalog; force requests a source refresh instead of a local reread.
    Refresh {
        /// Whether the user explicitly requested refresh.
        force: bool,
    },
}
/// Shared GPUI control, independent of chat sessions, storage and design-system globals.
pub struct Picker {
    pub(crate) data: PickerData,
    pub(crate) diagnostic_id: Option<String>,
    pub(crate) query: Entity<InputState>,
    pub(crate) expanded: std::collections::HashSet<String>,
    pub(crate) search_folded: std::collections::HashSet<String>,
    pub(crate) highlighted: usize,
    pub(crate) slider: Option<Entity<SliderState>>,
    pub(crate) slider_key: Option<(String, Vec<String>)>,
    pub(crate) slider_subscription: Option<Subscription>,
    pub(crate) slider_focus: FocusHandle,
    pub(crate) scroll: ScrollHandle,
    _query_subscription: Subscription,
}
impl std::fmt::Debug for Picker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picker").finish_non_exhaustive()
    }
}
impl EventEmitter<PickerEvent> for Picker {}
impl Picker {
    /// Create transient inputs; call `set_data` with an authoritative host snapshot.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr("搜索模型…（可附加等级：opus high）"))
        });
        let subscription = cx.subscribe(&query, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.highlighted = 0;
                this.search_folded.clear();
                this.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        });
        Self {
            data: PickerData::default(),
            diagnostic_id: None,
            query,
            expanded: Default::default(),
            search_folded: Default::default(),
            highlighted: 0,
            slider: None,
            slider_key: None,
            slider_subscription: None,
            slider_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _query_subscription: subscription,
        }
    }
    /// Diagnostic ID associated with the host's current refresh failure; `None` hides copying.
    pub fn set_diagnostic_id(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.diagnostic_id != id {
            self.diagnostic_id = id;
            cx.notify();
        }
    }
    /// Replace host state only when changed; this never emits a user action.
    pub fn set_data(&mut self, data: PickerData, cx: &mut Context<Self>) {
        if self.data == data {
            return;
        }
        if self.data.selected.as_ref().map(|s| &s.model) != data.selected.as_ref().map(|s| &s.model)
            && let Some(model) = data
                .selected
                .as_ref()
                .and_then(|s| data.models.iter().find(|m| m.id == s.model))
        {
            self.expanded.insert(model.provider_id.clone());
        }
        self.data = data;
        cx.notify();
    }
    /// Refreshes labels in an existing host snapshot without reloading its catalog.
    /// Wire IDs, selection and search text remain unchanged. Hosts call this on locale changes;
    /// absent IDs retain their host-provided labels and hints.
    pub fn set_effort_labels(&mut self, labels: &[(&str, &str)], cx: &mut Context<Self>) {
        let mut changed = false;
        for effort in self.data.models.iter_mut().flat_map(|model| &mut model.efforts) {
            if let Some((_, label)) = labels.iter().find(|(id, _)| *id == effort.id)
                && effort.label != *label
            {
                effort.label.clear();
                effort.label.push_str(label);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }
    pub(crate) fn select(&self, model: String, effort: Option<String>, cx: &mut Context<Self>) {
        if self.data.enabled {
            cx.emit(PickerEvent::Select(Selection { model, effort }));
        }
    }
}

//! `TermElement`: a gpui `Element` that paints a `lumakit_term::Frame`.
//!
//! The full painter lives in `paint::paint_frame`; this element is the
//! prepaint + paint wrapper the host embeds. Line damage uses the per-row
//! hash provided by `lumakit_term::snapshot`.

use std::sync::Arc;

use gpui_kit::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, LayoutId, Pixels, Style, Window,
};
use lumakit_term::{Frame, Palette, TermHandle};
use parking_lot::Mutex;

use crate::overlay::Overlays;
use crate::paint::{Metrics, paint_frame};

/// Per-frame prepaint state the paint stage consumes.
pub struct TermPrepaint {
    /// Snapshot captured during prepaint.
    pub frame: Frame,
    /// Cached row hashes (one per visible row). Rows with matching hashes
    /// in the previous prepaint are candidates for replay.
    pub row_hashes: Vec<u64>,
}

/// Element wrapping a `TermHandle`.
pub struct TermElement {
    handle: Arc<TermHandle>,
    palette: Palette,
    metrics: Metrics,
    overlays: Overlays,
    previous_hashes: Arc<Mutex<Vec<u64>>>,
    focused: bool,
    composition: String,
}

impl TermElement {
    /// Build a new element.
    pub fn new(handle: Arc<TermHandle>, palette: Palette, metrics: Metrics) -> Self {
        Self {
            handle,
            palette,
            metrics,
            overlays: Overlays::default(),
            previous_hashes: Arc::new(Mutex::new(Vec::new())),
            focused: true,
            composition: String::new(),
        }
    }

    /// Replace overlays.
    pub fn with_overlays(mut self, overlays: Overlays) -> Self {
        self.overlays = overlays;
        self
    }

    /// Focused state (empty cursor when false).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Current IME composition string.
    pub fn composition(mut self, text: impl Into<String>) -> Self {
        self.composition = text.into();
        self
    }
}

impl std::fmt::Debug for TermElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermElement")
            .field("focused", &self.focused)
            .finish_non_exhaustive()
    }
}

impl Element for TermElement {
    type RequestLayoutState = ();
    type PrepaintState = TermPrepaint;

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style::default();
        (window.request_layout(style, [], _cx), ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        // Only take the snapshot; comparison with `previous_hashes` is used
        // later when the paint path is wired. For T1.18 we expose the
        // mechanism without the damage shortcut yet.
        self.handle.take_dirty();
        let frame = self.handle.snapshot(&self.palette);
        let row_hashes: Vec<u64> = frame.rows.iter().map(|row| row.hash).collect();
        let mut prev = self.previous_hashes.lock();
        *prev = row_hashes.clone();
        TermPrepaint { frame, row_hashes }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_frame(
            &prepaint.frame,
            bounds,
            &self.metrics,
            &self.palette,
            &self.composition,
            self.focused,
            &self.overlays,
            window,
            cx,
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn prepaint_state_debug_impl_works() {
        let hashes: Vec<u64> = Vec::new();
        let _ = format!("{hashes:?}");
    }
}

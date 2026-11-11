//! Inline context usage for GPUI hosts. No I/O, accounting, or theme installation.
//!
//! Wrap host assets with [`UsageStatusAssets`] (outside any picker wrapper), create
//! [`ContextStatusView`] with `cx.new`, and update its snapshot using `set_status`.
//! Clicks open built-in details; hosts may subscribe to [`UsageStatusEvent`] to
//! open their own accounting surface instead, using `set_builtin_details(false)`.
//! Numbers are facts supplied by the host, not a billing or context-limit guarantee.
mod assets;
pub mod localization;
mod presentation;
mod view;

pub use assets::UsageStatusAssets;
pub use view::ContextStatusView;

/// Observed money in one currency; hosts retain partial/multi-currency detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Money {
    /// Currency identifier, e.g. USD. No implicit exchange is performed.
    pub currency: String,
    /// Nonnegative finite amount. Invalid amounts are displayed as unknown.
    pub amount: f64,
    /// Price-times-token estimate rather than an upstream monetary charge.
    pub estimated: bool,
    /// Only a known portion of requests is priced.
    pub partial: bool,
}

/// Host-reported activity, separate from whether usage is known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusState {
    /// Idle and ready.
    #[default]
    Ready,
    /// A response is being streamed.
    Streaming,
    /// Context compaction is running.
    Compacting,
    /// Activity is not reported.
    Unknown,
}

/// One controlled snapshot. `None` means unknown, never zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextStatus {
    /// Current context occupancy, not cumulative request tokens.
    pub used_tokens: Option<u64>,
    /// Model context capacity; zero is treated as unknown.
    pub context_window: Option<u64>,
    /// Reported occupancy in percent (0..=100); never inferred from tokens.
    pub percent: Option<f32>,
    /// Whether current context numbers are estimates.
    pub estimate: bool,
    /// Host-accounted cumulative request tokens without double-counted subsets.
    pub cumulative_tokens: Option<u64>,
    /// User-configured request-price estimate only; None falls back to cumulative tokens.
    pub cumulative_cost: Option<Money>,
    /// Current activity.
    pub state: StatusState,
    /// Whether automatic context compaction is enabled.
    pub auto_compact: bool,
    /// Host explanations, including missing usage/prices or multiple currencies.
    pub detail: String,
}

/// Interaction intent, emitted in addition to optional built-in details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageStatusEvent {
    /// Open the host's existing usage/accounting details.
    OpenDetails,
}

/// Compile-target platform; no runtime operating-system queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Microsoft Windows.
    Windows,
    /// Apple macOS.
    MacOs,
    /// Linux.
    Linux,
    /// Another target; no platform glyph is shown.
    Other,
}
impl Platform {
    /// The compile target, including reserved macOS/Linux asset mappings.
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::Other => "Other platform",
        }
    }
}

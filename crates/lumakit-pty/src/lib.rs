//! Cross-platform PTY runtime, shell detection and encoding decoder shared
//! by AutoTerm (T1.14 pipeline) and ThinkRail (after T1.23). Ported from
//! autorail's `crates/terminal/`, with its "replay + offset" history replaced
//! by a bounded ring buffer so the host owns capacity.
//!
//! No GUI, no `tracing_appender`, no product-specific session paths.

#![warn(missing_docs)]

mod decode;
mod ring;
mod runtime;
mod shells;

pub use decode::{DecodeError, Decoder};
pub use ring::RingBuffer;
pub use runtime::{PtyRuntime, RuntimeError, RuntimeEvent, SpawnOptions, TermSize};
pub use shells::{Shell, ShellCatalog, ShellKind, discover};

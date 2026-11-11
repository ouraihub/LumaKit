//! Host-injected UI translation, independent of product catalogs and protocol identities.
use std::{borrow::Cow, cell::Cell};

/// Translates built-in Chinese UI source labels; return a borrowed label when possible.
/// Model names, provider identities, wire effort IDs and host-owned text are never passed here.
pub type Translator = fn(&'static str) -> Cow<'static, str>;

thread_local! {
    static TRANSLATOR: Cell<Translator> = Cell::new(Cow::Borrowed);
}

/// Installs the host translator on the GPUI UI thread. Defaults to Chinese for existing hosts.
/// Call again when changing language, then refresh host windows; lookups occur during rendering,
/// including already-open popups. The callback must not re-enter GPUI or perform I/O.
pub fn set_translator(translator: Translator) {
    TRANSLATOR.with(|current| current.set(translator));
}

pub(crate) fn text(source: &'static str) -> Cow<'static, str> {
    TRANSLATOR.with(|current| current.get()(source))
}

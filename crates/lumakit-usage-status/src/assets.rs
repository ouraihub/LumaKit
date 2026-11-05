//! Apple/Linux paths: Simple Icons develop/icons/{apple,linux}.svg, fetched
//! 2026-11-05 from https://github.com/simple-icons/simple-icons (CC0; license
//! included at crate root). Brand marks remain their owners' trademarks.
//! Windows geometry is original component artwork; wand uses RiMagicLine from
//! @remixicon/react 4.9.0 (LICENSE-REMIX). Colors follow the host theme.
//! macOS/Linux resources are reserved, not live-verified.
use crate::Platform;
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, SharedString};
use std::borrow::Cow;

const PREFIX: &str = "lumakit-usage-status/";
const ICONS: [(&str, &[u8]); 4] = [
    ("windows", br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="white" d="M2 5.2 10.2 4v7.2H2zm9.5-1.4L22 2.3v8.9H11.5zM2 12.6h8.2v7.2L2 18.7zm9.5 0H22v8.9L11.5 20z"/></svg>"#),
    ("apple", include_bytes!("../assets/apple.svg")),
    ("linux", include_bytes!("../assets/linux.svg")),
    ("wand", include_bytes!("../assets/magic.svg")),
];

/// Composes embedded usage icons with the original host assets as fallback.
/// Install as `UsageStatusAssets(PickerAssets(host_assets))` when using both.
#[derive(Clone, Copy, Debug)]
pub struct UsageStatusAssets<A>(pub A);
impl<A: AssetSource> AssetSource for UsageStatusAssets<A> {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix(PREFIX)
            .and_then(|p| p.strip_suffix(".svg"))
            && let Some((_, bytes)) = ICONS.iter().find(|(id, _)| *id == name)
        {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        self.0.load(path)
    }
    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = if path.starts_with(PREFIX) {
            Vec::new()
        } else {
            self.0.list(path)?
        };
        paths.extend(
            ICONS
                .iter()
                .map(|(name, _)| format!("{PREFIX}{name}.svg"))
                .filter(|name| name.starts_with(path))
                .map(SharedString::from),
        );
        Ok(paths)
    }
}

pub(crate) fn icon(name: &str) -> Icon {
    Icon::default().path(format!("{PREFIX}{name}.svg"))
}
pub(crate) fn platform_icon(platform: Platform) -> Option<Icon> {
    match platform {
        Platform::Windows => Some(icon("windows")),
        Platform::MacOs => Some(icon("apple")),
        Platform::Linux => Some(icon("linux")),
        Platform::Other => None,
    }
}

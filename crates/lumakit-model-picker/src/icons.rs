//! Exact ThinkRail picker shapes from @remixicon/react 4.9.0 (Remix Icon License v1.0).
//! Package SHA-1 e4469985dc88af987f03c2309e36b04028cb4600; SVG path data unmodified.
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, SharedString};
use std::borrow::Cow;

const ICONS: [(&str, &[u8]); 10] = [
    ("RiZzzLine", include_bytes!("../assets/RiZzzLine.svg")),
    (
        "RiContrastDrop2Line",
        include_bytes!("../assets/RiContrastDrop2Line.svg"),
    ),
    ("RiLeafLine", include_bytes!("../assets/RiLeafLine.svg")),
    (
        "RiFlashlightLine",
        include_bytes!("../assets/RiFlashlightLine.svg"),
    ),
    (
        "RiSparkling2Line",
        include_bytes!("../assets/RiSparkling2Line.svg"),
    ),
    (
        "RiSparklingLine",
        include_bytes!("../assets/RiSparklingLine.svg"),
    ),
    ("RiFireLine", include_bytes!("../assets/RiFireLine.svg")),
    (
        "RiPushpin2Line",
        include_bytes!("../assets/RiPushpin2Line.svg"),
    ),
    (
        "RiRefreshLine",
        include_bytes!("../assets/RiRefreshLine.svg"),
    ),
    ("RiKey2Line", include_bytes!("../assets/RiKey2Line.svg")),
];
const PREFIX: &str = "lumakit-picker/";

/// Adds the shared picker's original icons without replacing the host's own assets.
#[derive(Debug, Clone, Copy)]
pub struct PickerAssets<A>(pub A);
impl<A: AssetSource> AssetSource for PickerAssets<A> {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix(PREFIX)
            .and_then(|path| path.strip_suffix(".svg"))
            && let Some((_, bytes)) = ICONS.iter().find(|(id, _)| *id == name)
        {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        self.0.load(path)
    }
    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        if path.starts_with(PREFIX) {
            return Ok(ICONS
                .iter()
                .map(|(id, _)| SharedString::from(format!("{PREFIX}{id}.svg")))
                .collect());
        }
        self.0.list(path)
    }
}
pub(crate) fn icon(name: &str) -> Icon {
    Icon::default().path(format!("{PREFIX}{name}.svg"))
}
pub(crate) fn glyph(id: Option<&str>) -> Icon {
    icon(match id {
        Some("off") => "RiZzzLine",
        Some("minimal") => "RiLeafLine",
        Some("low") => "RiFlashlightLine",
        Some("medium") => "RiContrastDrop2Line",
        Some("high") => "RiSparkling2Line",
        Some("xhigh") => "RiSparklingLine",
        Some("max") => "RiFireLine",
        _ => "RiContrastDrop2Line",
    })
}

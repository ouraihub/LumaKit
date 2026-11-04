//! The asset source the app registers: the component library's default icons plus the Lucide
//! icons LumaKit components and hosts use (selected at compile time, nothing downloaded).

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString, assets::Assets};

gpui_kit::assets::icon_assets!(
    Extra,
    [
        Folder,
        FolderOpen,
        FolderGit2,
        GitBranch,
        GitCompare,
        GitCommitHorizontal,
        Plus,
        X,
        ChevronRight,
        ChevronDown,
        ChevronUp,
        Settings,
        SquareTerminal,
        RotateCcwClock,
        File,
        FileText,
        Files,
        FileDiff,
        FilePlus,
        ScrollText,
        FileMinus,
        MessageSquare,
        MessageCirclePlus,
        List,
        ListTree,
        ListChecks,
        SendHorizontal,
        ArrowUp,
        Square,
        Circle,
        CircleCheck,
        CircleX,
        CircleQuestionMark,
        Check,
        Ban,
        LoaderCircle,
        TriangleAlert,
        CircleAlert,
        Copy,
        PanelLeft,
        Brain,
        Wrench,
        Clock,
        Undo2,
        Pencil,
        Trash,
        Eye,
        Search,
        Play,
        Pause,
        Minus,
        Bot,
        User,
        Ellipsis,
        RefreshCw,
        SquarePen,
        Columns2,
        Rows2,
        ShieldCheck,
        ShieldAlert,
        Plug,
        KeyRound,
        Star,
        TextWrap,
        GitPullRequest,
        Settings2,
        SlidersHorizontal,
        ChevronsUpDown,
        ChevronsDownUp,
        EyeOff,
        ExternalLink,
        Sparkles,
        Palette,
        Monitor,
        ArrowRight,
        RotateCcw,
        Info,
        // LumaKit chat and LumaDesk settings.
        AtSign,
        Gauge,
        Image,
        MessageSquarePlus,
        GripVertical,
        Paperclip
    ]
);

#[derive(Debug, Clone, Copy, Default)]
pub struct UiAssets;

impl AssetSource for UiAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        match Extra.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut all = Extra.list(path)?;
        all.extend(Assets.list(path)?);
        all.sort();
        all.dedup();
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    #[test]
    fn icons_outside_the_kit_bundle_resolve_and_kit_icons_still_do() {
        // These render as empty boxes with the Kit-only source, which reports a missing asset as an error.
        for icon in [IconName::Paperclip, IconName::Image, IconName::KeyRound, IconName::SlidersHorizontal] {
            assert!(!Assets.load(&icon.path()).is_ok_and(|bytes| bytes.is_some()), "{icon:?} is now in the Kit bundle; drop it from Extra");
            assert!(UiAssets.load(&icon.path()).is_ok_and(|bytes| bytes.is_some()), "{icon:?} must resolve");
        }
        for icon in [IconName::Globe, IconName::Menu, IconName::LayoutDashboard] {
            assert!(UiAssets.load(&icon.path()).is_ok_and(|bytes| bytes.is_some()), "{icon:?} must still come from the Kit bundle");
        }
    }
}

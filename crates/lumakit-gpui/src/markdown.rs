use std::{collections::HashMap, ops::Range};
use gpui_kit::{ImageSource, Resource, SharedString};
use gpui_kit::component::text::TextView;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub(crate) fn web_link(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false; };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest.is_empty() && !url.chars().any(char::is_control)
}

const IMAGE_PLACEHOLDER: &str = "〔图片未加载〕";

/// Backslash-escapes every ASCII punctuation character, so the text cannot form Markdown or HTML.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() { if c.is_ascii_punctuation() { out.push('\\'); } out.push(c); }
    out
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES | Options::ENABLE_DEFINITION_LIST
}
// Rewrites the untrusted source in place: raw HTML is dropped, every image form becomes a
// placeholder, and a link to anything but http(s) becomes its escaped visible text.
// Kit renders Markdown, not HTML: its HTML import gives nodes no source span, so paragraphs
// with inline code share one element id and all but the last paint as blank bars (0.9.0).
// The view also blocks every image load, in case Kit's parser disagrees with this one.
fn sanitized_markdown(source: &str) -> String {
    let options = options();
    let mut out = String::with_capacity(source.len());
    let (mut cursor, mut skip_until) = (0, 0);
    // An unsafe link's source range and visible text, replaced as a whole at its end.
    let mut unsafe_link: Option<(Range<usize>, String)> = None;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        if range.start < skip_until { continue; }
        let image = matches!(event, Event::Start(Tag::Image { .. }));
        if image { skip_until = range.end; }
        if let Some((link, text)) = &mut unsafe_link {
            match event {
                Event::End(TagEnd::Link) => {
                    out.push_str(&source[cursor..link.start]);
                    out.push_str(&escaped(text));
                    cursor = link.end;
                    unsafe_link = None;
                }
                Event::Text(t) | Event::Code(t) => text.push_str(&t),
                Event::SoftBreak | Event::HardBreak => text.push(' '),
                _ if image => text.push_str(IMAGE_PLACEHOLDER),
                _ => {}
            }
            continue;
        }
        let replacement = match event {
            _ if image => IMAGE_PLACEHOLDER,
            Event::Html(_) | Event::InlineHtml(_) => "",
            Event::Start(Tag::Link { dest_url, .. }) if !web_link(&dest_url) => { unsafe_link = Some((range, String::new())); continue; }
            _ => continue,
        };
        out.push_str(&source[cursor..range.start]);
        out.push_str(replacement);
        cursor = range.end;
    }
    out.push_str(&source[cursor..]);
    out
}

struct Entry {
    source: SharedString,
    markdown: SharedString,
    bytes: usize,
    used: u64,
}
/// Sanitized Markdown per answer. Kit keeps the parsed document in element state keyed by
/// the view id, which is freed with the element.
#[derive(Default)]
pub(crate) struct MarkdownCache {
    entries: HashMap<String, Entry>,
    clock: u64,
}
impl MarkdownCache {
    pub fn clear(&mut self) { self.entries.clear(); }
    pub fn view(&mut self, id: &str, source: &str) -> (TextView, SharedString) {
        self.clock = self.clock.wrapping_add(1);
        if self.entries.get(id).is_none_or(|entry| entry.source.as_ref() != source) {
            let markdown = sanitized_markdown(source);
            let bytes = source.len() + markdown.len();
            self.entries.remove(id);
            // ponytail: keep one oversized answer readable even if it alone exceeds
            // 4 MiB; chunk rendering only if the 1 MiB source cap grows.
            while !self.entries.is_empty() && (self.entries.len() >= 64
                || self.entries.values().map(|e| e.bytes).sum::<usize>() + bytes > 4 * 1024 * 1024) {
                let oldest = self.entries.iter().min_by_key(|(_, e)| e.used).map(|(key, _)| key.clone()).unwrap();
                self.entries.remove(&oldest);
            }
            self.entries.insert(id.to_owned(), Entry { source: source.to_owned().into(), markdown: markdown.into(), bytes, used: self.clock });
        }
        let entry = self.entries.get_mut(id).unwrap();
        entry.used = self.clock;
        (TextView::markdown(SharedString::from(format!("answer:{id}")), entry.markdown.clone()).selectable(true).scrollable(false)
            // No asset has this name: every image fails without touching the network or disk.
            .image_source(|_| ImageSource::Resource(Resource::Embedded("".into())))
            .on_link_click(|url, _, _, cx| { if web_link(url) { cx.open_url(url); } }), entry.source.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_markdown_cannot_load_resources_or_open_local_links() {
        let source = "# 标题\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\n![remote](https://example.com/x) ![local](file:///secret) ![ref][img] [![nested](https://e.com/i)](https://example.com)\n\n[img]: https://example.com/ref\n\n<img src='https://example.com/raw'><script>bad()</script>\n\n<div>\n块级 HTML\n</div>\n\n[unsafe](javascript:alert) [local](file:///secret) [relative](secret.txt) [**加粗** `代码`](file:///x) [safe](HTTPS://example.com) <file:///auto>\n\n- 列表 `code`\n- `<img src=x>` 不是 HTML";
        let safe = sanitized_markdown(source);
        // Re-parse as Kit will: no image, no raw HTML, and only web links survive.
        let mut links = Vec::new();
        for event in Parser::new_ext(&safe, options()) {
            match event {
                Event::Start(Tag::Image { .. }) | Event::Html(_) | Event::InlineHtml(_) => panic!("unsafe event survived: {event:?}\n{safe}"),
                Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url.to_string()),
                _ => {}
            }
        }
        assert!(links.iter().all(|url| web_link(url)), "{links:?}");
        assert!(links.contains(&"HTTPS://example.com".to_owned()));
        assert_eq!(safe.matches(IMAGE_PLACEHOLDER).count(), 4);
        assert!(safe.contains("```rust\nlet x = 1;\n```") && safe.contains("| 1 | 2 |"));
        assert!(safe.contains("- 列表 `code`") && safe.contains("`<img src=x>` 不是 HTML"), "code spans stay verbatim:\n{safe}");
        assert!(safe.contains("加粗 代码") && !safe.contains("javascript:") && !safe.contains("file:///secret"));
        assert_eq!(sanitized_markdown(&safe), safe, "sanitizing is idempotent");
        assert!(!web_link("https://example.com\n") && !web_link("data:text/html,x"));
    }
}

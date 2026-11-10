//! Compiler source locations, never developer-machine absolute paths.

pub(crate) fn source_path(path: &str) -> &str {
    let file = path.rsplit(['/', '\\']).next().unwrap_or_default();
    for (start, _) in path.char_indices() {
        if start != 0 && !matches!(path.as_bytes()[start - 1], b'/' | b'\\') {
            continue;
        }
        let relative = &path[start..];
        if [
            "crates/",
            "crates\\",
            "src/",
            "src\\",
            "tests/",
            "tests\\",
            "examples/",
            "examples\\",
        ]
        .iter()
        .any(|root| relative.starts_with(root))
            && relative
                .split(['/', '\\'])
                .all(|part| !matches!(part, "" | "." | ".."))
        {
            return relative;
        }
    }
    file
}

pub(crate) fn backtrace() -> String {
    use std::fmt::Write as _;
    let raw = std::backtrace::Backtrace::force_capture().to_string();
    let mut safe = String::with_capacity(raw.len());
    for line in raw.lines() {
        if !safe.is_empty() {
            safe.push('\n');
        }
        match line.trim().strip_prefix("at ") {
            Some(path) => {
                let _ = write!(safe, "at {}", source_path(path));
            }
            None => safe.push_str(line),
        }
    }
    safe
}

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests fail loudly on missing log lines"
)]

use std::{
    io,
    sync::{Arc, Mutex},
    thread,
};

use tracing_subscriber::{EnvFilter, Registry, fmt::MakeWriter, layer::SubscriberExt};

use super::*;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

/// 与 `LogHandle::init` 相同的层组合，输出写入内存；返回按行切分的日志。
fn capture(level: &str, body: impl FnOnce()) -> Vec<String> {
    let buffer = Buffer::default();
    let subscriber = Registry::default()
        .with(EnvFilter::new(level))
        .with(SpanFields)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .event_format(GrepFormat::utc())
                .with_writer(buffer.clone()),
        );
    tracing::subscriber::with_default(subscriber, body);
    let bytes = buffer.0.lock().unwrap().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// 取 `Key:` 段的值；段之间以未转义的 `|` 分隔。
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (_time, rest) = line.split_once(' ')?;
    let (_level, body) = rest.trim_start().split_once(' ')?;
    body.trim_start()
        .split('|')
        .find_map(|segment| segment.strip_prefix(key)?.strip_prefix(':'))
}

fn find<'a>(lines: &'a [String], me: &str, ev: &str) -> &'a str {
    lines
        .iter()
        .find(|line| field(line, "ME") == Some(me) && field(line, "EV") == Some(ev))
        .unwrap_or_else(|| panic!("missing {me} {ev} in {lines:#?}"))
}

#[test]
fn trace_ids_are_32_hex_and_distinct() {
    let lines = capture("info", || {
        let (first, first_guard) = TraceContext::root("test.first", "ui");
        let (second, second_guard) = TraceContext::root("test.second", "ui");
        assert_eq!(first.trace_id().len(), 32);
        assert!(
            first
                .trace_id()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_ne!(first.trace_id(), second.trace_id());
        first_guard.finish("SUCCESS");
        second_guard.finish("SUCCESS");
    });
    assert_eq!(lines.len(), 4);
}

#[test]
fn chain_records_entry_exit_parent_and_duration() {
    let mut trace_id = String::new();
    let lines = capture("debug", || {
        let (trace, root) = TraceContext::root("chat.send", "ui");
        trace_id = trace.trace_id().to_owned();
        let _entered = root.span().clone().entered();
        let service = operation(&trace, "chat.request", "service");
        {
            let _service = service.span().clone().entered();
            operation(&trace, "provider.http", "external").finish("200");
        }
        service.finish("SUCCESS");
        root.finish("SUCCESS");
    });
    assert_eq!(lines.len(), 6, "{lines:#?}");
    assert!(
        lines
            .iter()
            .all(|line| field(line, "TraceId") == Some(&trace_id))
    );

    let root_entry = find(&lines, "chat.send", "ENTRY");
    assert!(root_entry.contains(" INFO "));
    assert_eq!(field(root_entry, "Layer"), Some("ui"));
    assert_eq!(field(root_entry, "ParentSpanId"), Some("-"));
    assert_eq!(field(root_entry, "RT"), None);

    let service_entry = find(&lines, "chat.request", "ENTRY");
    assert!(service_entry.contains(" DEBUG "));
    assert_eq!(
        field(service_entry, "ParentSpanId"),
        field(root_entry, "SpanId")
    );
    let http_exit = find(&lines, "provider.http", "EXIT");
    assert_eq!(
        field(http_exit, "ParentSpanId"),
        field(service_entry, "SpanId")
    );
    assert_eq!(field(http_exit, "Result"), Some("200"));

    let root_exit = find(&lines, "chat.send", "EXIT");
    assert_eq!(field(root_exit, "Result"), Some("SUCCESS"));
    assert!(field(root_exit, "RT").unwrap().parse::<u64>().is_ok());
}

#[test]
fn lifecycle_lines_include_operation_call_source_location() {
    let lines = capture("debug", || {
        let (trace, root) = TraceContext::root("source.root", "ui");
        let _entered = root.span().clone().entered();
        operation(&trace, "source.child", "service").finish("SUCCESS");
        root.finish("SUCCESS");
    });
    for line in lines.iter().filter(|line| field(line, "EV").is_some()) {
        assert!(field(line, "SourceFile").is_some(), "{line}");
        assert!(field(line, "SourceLine").is_some(), "{line}");
    }
    assert!(lines.iter().any(|line| line.contains("source.root")));
    assert!(lines.iter().any(|line| line.contains("source.child")));
}

#[test]
fn terminal_events_use_their_levels_and_fields() {
    let lines = capture("debug", || {
        let (trace, root) = TraceContext::root("job.run", "ui");
        let _entered = root.span().clone().entered();
        operation(&trace, "db.read", "repository").fail("DATABASE_TIMEOUT");
        operation(&trace, "net.fetch", "external").timeout(30_000);
        operation(&trace, "user.wait", "service").cancel("user_cancelled");
        drop(operation(&trace, "early.return", "service"));
        root.fail("DATABASE_TIMEOUT");
    });

    let child_error = find(&lines, "db.read", "ERROR");
    assert!(child_error.contains(" WARN "));
    assert_eq!(field(child_error, "Result"), Some("FAILED"));
    assert_eq!(field(child_error, "Error"), Some("DATABASE_TIMEOUT"));

    let timeout = find(&lines, "net.fetch", "TIMEOUT");
    assert!(timeout.contains(" WARN "));
    assert_eq!(field(timeout, "Limit"), Some("30000"));
    assert!(field(timeout, "RT").is_some());

    let cancel = find(&lines, "user.wait", "CANCEL");
    assert!(cancel.contains(" DEBUG "));
    assert_eq!(field(cancel, "Reason"), Some("user_cancelled"));

    let unfinished = find(&lines, "early.return", "UNFINISHED");
    assert!(unfinished.contains(" WARN "));

    assert!(find(&lines, "job.run", "ERROR").contains(" ERROR "));
    // 每个操作恰好一个终态：ENTRY 与终态各 5 条。
    assert_eq!(lines.len(), 10, "{lines:#?}");
}

#[test]
fn info_level_keeps_entry_points_and_hides_inner_lifecycle() {
    let lines = capture("info", || {
        let (trace, root) = TraceContext::root("settings.refresh", "ui");
        let _entered = root.span().clone().entered();
        operation(&trace, "catalog.fetch", "external").finish("SUCCESS");
        tracing::info!(count = 3, "catalog merged");
        root.finish("SUCCESS");
    });
    assert_eq!(lines.len(), 3, "{lines:#?}");
    let note = &lines[1];
    assert_eq!(field(note, "ME"), Some("settings.refresh"));
    assert_eq!(field(note, "count"), Some("3"));
    assert_eq!(field(note, "Msg"), Some("catalog merged"));
    assert!(field(note, "Target").is_some());
}

#[test]
fn worker_thread_keeps_trace_and_parent() {
    let lines = capture("debug", || {
        let (trace, root) = TraceContext::root("ai.request", "ui");
        let span = root.span().clone();
        let worker_trace = trace.clone();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let _entered = span.entered();
                operation(&worker_trace, "ai.stream", "external").finish("SUCCESS");
            });
        })
        .join()
        .unwrap();
        root.finish("SUCCESS");
    });
    let root_entry = find(&lines, "ai.request", "ENTRY");
    let worker = find(&lines, "ai.stream", "EXIT");
    assert_eq!(field(worker, "TraceId"), field(root_entry, "TraceId"));
    assert_eq!(field(worker, "ParentSpanId"), field(root_entry, "SpanId"));
}

#[test]
fn new_root_inside_another_trace_starts_a_separate_chain() {
    let lines = capture("info", || {
        let (_outer, outer) = TraceContext::root("outer", "ui");
        let _entered = outer.span().clone().entered();
        let (_inner, inner) = TraceContext::root("inner", "worker");
        inner.finish("SUCCESS");
        outer.finish("SUCCESS");
    });
    let outer = find(&lines, "outer", "ENTRY");
    let inner = find(&lines, "inner", "ENTRY");
    assert_ne!(field(inner, "TraceId"), field(outer, "TraceId"));
    assert_eq!(field(inner, "ParentSpanId"), Some("-"));
}

#[test]
fn values_cannot_break_the_line_or_segments() {
    let lines = capture("info", || {
        tracing::info!(note = "a|b\nc", "x|y");
    });
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with("note:a\\|b\\nc|Msg:x\\|y"), "{lines:#?}");
}

#[test]
fn business_event_field_never_masquerades_as_lifecycle() {
    let lines = capture("info", || {
        let (_trace, root) = TraceContext::root("ai.request", "service");
        root.span()
            .in_scope(|| tracing::warn!(event = "ai.http.rejected", status = 429, "rejected"));
        root.finish("SUCCESS");
    });
    let note = &lines[1];
    assert_eq!(field(note, "EV"), None, "{lines:#?}");
    assert_eq!(field(note, "event"), Some("ai.http.rejected"));
    assert_eq!(parse_line(note).event, None);
}

#[test]
fn parser_reads_back_what_the_formatter_writes() {
    let mut trace_id = String::new();
    let lines = capture("info", || {
        let (trace, root) = TraceContext::root("chat.send", "ui");
        trace_id = trace.trace_id().to_owned();
        root.span()
            .in_scope(|| tracing::warn!(note = "a|b", "slow|reply\nretrying"));
        root.fail("AI_HTTP_429");
        tracing::info!("outside");
    });
    let [entry, note, exit, outside] = lines.as_slice() else {
        panic!("{lines:#?}");
    };
    let entry = parse_line(entry);
    assert_eq!(entry.level, "INFO");
    assert_eq!(entry.trace_id.as_deref(), Some(trace_id.as_str()));
    assert_eq!(entry.layer.as_deref(), Some("ui"));
    assert_eq!(entry.operation.as_deref(), Some("chat.send"));
    assert_eq!(entry.event.as_deref(), Some("ENTRY"));
    assert_eq!(entry.duration_ms, None);

    let note = parse_line(note);
    assert_eq!(note.level, "WARN");
    assert_eq!(note.message, "slow|reply\nretrying");
    assert!(note.target.is_some());
    assert_eq!(note.event, None);

    let exit = parse_line(exit);
    assert_eq!(exit.level, "ERROR");
    assert_eq!(exit.event.as_deref(), Some("ERROR"));
    assert!(exit.duration_ms.is_some());

    let outside = parse_line(outside);
    assert_eq!(outside.trace_id, None);
    assert_eq!(outside.message, "outside");

    let garbage = parse_line("thread 'main' panicked");
    assert_eq!(garbage.time, "");
    assert_eq!(garbage.message, "thread 'main' panicked");
    assert_eq!(garbage.trace_id, None);
}

#[test]
fn redaction_never_returns_secrets_or_query_data() {
    assert_eq!(redact_secret("secret-value"), "***");
    assert_eq!(redact_secret(""), "<empty>");
    assert_eq!(
        redact_url("https://user:password@example.com/path?token=secret#fragment"),
        "https://example.com/path"
    );
    assert_eq!(bounded_text("你好世界", 3), "你好…");
    assert_eq!(diagnostic_hash("same"), diagnostic_hash("same"));
    assert_ne!(diagnostic_hash("same"), "same");
}

#[test]
fn source_locations_never_expose_absolute_build_paths() {
    let lines = capture("info", || {
        let (trace, seed) = TraceContext::root("source.seed", "worker");
        seed.finish("SUCCESS");
        let root = crate::operation::start(
            &trace,
            "source.absolute",
            "worker",
            true,
            "C:\\PRIVATE_BUILD\\checkout\\crates\\demo\\src\\lib.rs",
            42,
        );
        root.span().in_scope(|| {
            tracing::info!(
                log.file = "C:\\PRIVATE_BUILD\\checkout\\src\\handler.rs",
                "source.event"
            );
            tracing::info!(log.file = ?"/PRIVATE_BUILD/checkout/src/debug.rs", "source.debug");
        });
        root.finish("SUCCESS");
    });
    assert!(
        lines.iter().all(|line| !line.contains("PRIVATE_BUILD")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("SourceFile:crates\\demo\\src\\lib.rs")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("log.file:src\\handler.rs")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("log.file:src/debug.rs")),
        "{lines:#?}"
    );
}

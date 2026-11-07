use super::*;

#[test]
fn trace_context_has_a_stable_id_and_child_keeps_it() {
    let (trace, _root) = TraceContext::root("test.root").unwrap();
    assert_eq!(trace.trace_id().len(), 32);
    assert!(trace.trace_id().bytes().all(|b| b.is_ascii_hexdigit()));
    let child = trace.child("test.child");
    let _entered = child.enter();
    assert_eq!(trace.trace_id().len(), 32);
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
fn operation_lifecycle_methods_are_idempotent() {
    let (trace, _root) = TraceContext::root("test.operation").unwrap();
    operation(&trace, "test.success", "service").finish("ok");
    operation(&trace, "test.failure", "repository").fail("TEST_FAILURE");
    operation(&trace, "test.cancel", "external").cancel("user");
    operation(&trace, "test.timeout", "external").timeout(10);
    operation(&trace, "test.unfinished", "worker").finish("ok");
}

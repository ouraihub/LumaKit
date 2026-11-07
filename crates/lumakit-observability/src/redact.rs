use sha2::{Digest, Sha256};

/// Hides a secret while retaining whether a value was present.
pub fn redact_secret(value: &str) -> String {
    if value.is_empty() {
        return "<empty>".into();
    }
    "***".into()
}

/// Removes userinfo and query/fragment data from a URL-like value.
pub fn redact_url(value: &str) -> String {
    let without_fragment = value.split_once('#').map_or(value, |(value, _)| value);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(value, _)| value);
    if let Some((scheme, rest)) = without_query.split_once("://") {
        let host = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
        return format!("{scheme}://{host}");
    }
    without_query.to_owned()
}

/// Truncates by Unicode scalar values and appends an explicit marker.
pub fn bounded_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", value.chars().take(keep).collect::<String>())
}

/// Returns a stable non-secret diagnostic hash.
pub fn diagnostic_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

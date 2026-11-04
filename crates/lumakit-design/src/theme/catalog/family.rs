//! A theme's family, by id (original `familyKey`): the id without a trailing `light` / `dark`
//! segment and a leading `high-contrast` prefix. `minimal-dark` and `minimal-light` share
//! `minimal`; `dark`, `light` and `high-contrast-*` have the empty family.

/// `id.replace(/(^|-)(light|dark)$/, "").replace(/^high-contrast-?/, "")`.
pub(super) fn key(id: &str) -> &str {
    let id = ["light", "dark"]
        .iter()
        .find_map(|suffix| {
            let rest = id.strip_suffix(suffix)?;
            if rest.is_empty() {
                Some(rest)
            } else {
                rest.strip_suffix('-')
            }
        })
        .unwrap_or(id);
    id.strip_prefix("high-contrast")
        .map_or(id, |rest| rest.strip_prefix('-').unwrap_or(rest))
}

//! Pure selection and search rules shared by pointer and keyboard paths.
use crate::{Model, Selection};
const KNOWN: [&str; 8] = [
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
];

pub(crate) fn selected_effort<'a>(
    model: &'a Model,
    selection: Option<&Selection>,
) -> Option<&'a crate::Effort> {
    let selection = selection.filter(|s| s.model == model.id)?;
    model
        .efforts
        .iter()
        .find(|e| Some(e.id.as_str()) == selection.effort.as_deref())
}
pub(crate) fn search(query: &str) -> (&str, Option<&str>) {
    let query = query.trim();
    if let Some((base, suffix)) = query.rsplit_once(char::is_whitespace)
        && !base.trim().is_empty()
        && KNOWN.iter().any(|id| id.eq_ignore_ascii_case(suffix))
    {
        return (base.trim(), Some(suffix));
    }
    (query, None)
}
pub(crate) fn score(model: &Model, query: &str) -> Option<usize> {
    let (query, level) = search(query);
    if level.is_some_and(|level| {
        !model
            .efforts
            .iter()
            .any(|e| e.id.eq_ignore_ascii_case(level))
    }) {
        return None;
    }
    let text = format!("{} {} {}", model.name, model.provider, model.detail).to_lowercase();
    let query = query.to_lowercase();
    if let Some(index) = text.find(&query) {
        return Some(index);
    }
    if query.split_whitespace().all(|part| text.contains(part)) {
        return Some(500);
    }
    let mut chars = text.char_indices();
    let mut rank = 1000;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        rank += chars.find(|(_, c)| *c == wanted)?.0;
    }
    Some(rank)
}
pub(crate) fn selection(model: &Model, current: Option<&Selection>, query: &str) -> Selection {
    let typed = search(query)
        .1
        .and_then(|id| model.efforts.iter().find(|e| e.id.eq_ignore_ascii_case(id)));
    let effort = typed.map(|e| e.id.clone()).or_else(|| {
        current
            .and_then(|s| s.effort.as_ref())
            .filter(|id| model.efforts.iter().any(|e| &e.id == *id))
            .cloned()
    });
    Selection {
        model: model.id.clone(),
        effort,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Model {
        Model {
            id: "p:opus".into(),
            provider_id: "p".into(),
            provider: "Provider".into(),
            name: "Opus".into(),
            detail: "source.json".into(),
            has_key: false,
            efforts: ["off", "low", "high"]
                .into_iter()
                .map(|id| crate::Effort {
                    id: id.into(),
                    label: id.into(),
                    hint: String::new(),
                })
                .collect(),
        }
    }
    #[test]
    fn suffix_search_rejects_unsupported_levels_and_handles_multiword_queries() {
        let model = model();
        assert!(score(&model, "opus HIGH").is_some());
        assert_eq!(score(&model, "opus max"), None);
        assert!(score(&model, "provider opus").is_some());
        assert_eq!(score(&model, "opus mystery"), None);
    }
    #[test]
    fn switching_preserves_supported_level_but_never_selects_first_level() {
        let model = model();
        assert_eq!(selection(&model, None, "").effort, None);
        let old = |id: &str| Selection {
            model: "other".into(),
            effort: Some(id.into()),
        };
        assert_eq!(
            selection(&model, Some(&old("high")), "").effort.as_deref(),
            Some("high")
        );
        assert_eq!(selection(&model, Some(&old("max")), "").effort, None);
        assert_eq!(
            selection(&model, Some(&old("high")), "opus off")
                .effort
                .as_deref(),
            Some("off")
        );
    }
}

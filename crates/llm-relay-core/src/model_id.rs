const CONTEXT_SUFFIX: &str = "[1m]";

pub fn has_context_suffix(id: &str) -> bool {
    id.get(id.len().saturating_sub(CONTEXT_SUFFIX.len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(CONTEXT_SUFFIX))
}

pub fn without_context_suffix(id: &str) -> &str {
    if has_context_suffix(id) {
        &id[..id.len() - CONTEXT_SUFFIX.len()]
    } else {
        id
    }
}

pub fn split_context_suffix(id: &str) -> (&str, bool) {
    (without_context_suffix(id), has_context_suffix(id))
}

pub fn normalize_claude_main_model(id: &str) -> String {
    format!("{}{}", without_context_suffix(id), CONTEXT_SUFFIX)
}

pub fn is_claude_family_model(id: &str) -> bool {
    let id = without_context_suffix(id);
    id.rsplit('/')
        .next()
        .unwrap_or(id)
        .get(.."claude".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_claude_family_ids() {
        assert!(is_claude_family_model("claude-opus-5[1m]"));
        assert!(is_claude_family_model("vendor/Claude-sonnet-5"));
        assert!(!is_claude_family_model("gpt-5.6-sol[1m]"));
    }

    #[test]
    fn main_context_suffix_is_idempotent() {
        assert_eq!(
            normalize_claude_main_model("gpt-5.6-sol"),
            "gpt-5.6-sol[1m]"
        );
        assert_eq!(
            normalize_claude_main_model("gpt-5.6-sol[1m]"),
            "gpt-5.6-sol[1m]"
        );
        assert_eq!(
            normalize_claude_main_model("gpt-5.6-sol[1M]"),
            "gpt-5.6-sol[1m]"
        );
    }
}

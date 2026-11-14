//! Attribute helpers shared by the loader and the extractor. Pure.

/// `#[cfg(test)]` (but not `#[cfg(not(test))]`).
pub fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let tokens = match &a.meta {
            syn::Meta::List(l) => l.tokens.to_string(),
            _ => return false,
        };
        tokens
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|t| t == "test")
            && !tokens.contains("not")
    })
}

/// `#[test]`, `#[tokio::test]`, `#[rstest]`-style test functions.
pub fn is_test_fn(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "test" || s.ident == "rstest")
    })
}

/// Skip code that only exists in test builds.
pub fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    is_cfg_test(attrs) || is_test_fn(attrs)
}

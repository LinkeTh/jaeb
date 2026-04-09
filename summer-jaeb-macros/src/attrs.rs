// SPDX-License-Identifier: MIT
//! Parsing of `#[event_listener(...)]` attribute arguments and state parameters.

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, Token, Type};

// ── Attribute arguments ──────────────────────────────────────────────────────

/// Parsed attributes from `#[event_listener(retries = 3, retry_strategy = "fixed", retry_base_ms = 100, dead_letter = true)]`.
///
/// Retry strategy is specified via `retry_strategy = "..."` combined with
/// `retry_base_ms` and optionally `retry_max_ms`.
#[cfg_attr(test, derive(Debug))]
pub(crate) struct ListenerAttrs {
    pub retries: Option<usize>,
    pub retry_strategy: Option<String>,
    pub retry_base_ms: Option<u64>,
    pub retry_max_ms: Option<u64>,
    pub dead_letter: Option<bool>,
    /// Human-readable listener name. Defaults to the function name when unset.
    /// Set to `""` to opt out of automatic naming.
    pub name: Option<String>,
}

impl Parse for ListenerAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut retries = None;
        let mut retry_strategy = None;
        let mut retry_base_ms = None;
        let mut retry_max_ms = None;
        let mut dead_letter = None;
        let mut name = None;

        let pairs = Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated(input)?;
        for pair in &pairs {
            let key = pair
                .path
                .get_ident()
                .ok_or_else(|| syn::Error::new_spanned(&pair.path, "expected identifier"))?
                .to_string();

            match key.as_str() {
                "retries" => {
                    if retries.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `retries`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retries = Some(lit.base10_parse()?);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                    }
                }
                "retry_strategy" => {
                    if retry_strategy.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `retry_strategy`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) = &pair.value {
                        let val = lit.value();
                        match val.as_str() {
                            "fixed" | "exponential" | "exponential_jitter" => {
                                retry_strategy = Some(val);
                            }
                            _ => {
                                return Err(syn::Error::new_spanned(
                                    &pair.value,
                                    "expected one of: \"fixed\", \"exponential\", \"exponential_jitter\"",
                                ));
                            }
                        }
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected string literal"));
                    }
                }
                "retry_base_ms" => {
                    if retry_base_ms.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `retry_base_ms`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retry_base_ms = Some(lit.base10_parse()?);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                    }
                }
                "retry_max_ms" => {
                    if retry_max_ms.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `retry_max_ms`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retry_max_ms = Some(lit.base10_parse()?);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                    }
                }
                "dead_letter" => {
                    if dead_letter.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `dead_letter`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(lit), ..
                    }) = &pair.value
                    {
                        dead_letter = Some(lit.value);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected bool"));
                    }
                }
                "name" => {
                    if name.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `name`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) = &pair.value {
                        name = Some(lit.value());
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected string literal"));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &pair.path,
                        format!(
                            "unknown attribute `{key}`, expected one of: retries, \
                             retry_strategy, retry_base_ms, retry_max_ms, dead_letter, name"
                        ),
                    ));
                }
            }
        }

        Ok(ListenerAttrs {
            retries,
            retry_strategy,
            retry_base_ms,
            retry_max_ms,
            dead_letter,
            name,
        })
    }
}

impl ListenerAttrs {
    /// Returns `true` if any failure-policy attribute was specified.
    pub(crate) fn has_failure_policy(&self) -> bool {
        self.retries.is_some()
            || self.retry_strategy.is_some()
            || self.retry_base_ms.is_some()
            || self.retry_max_ms.is_some()
            || self.dead_letter.is_some()
    }

    /// Returns `true` if any retry-related attribute was specified
    /// (excluding `retries` itself and `dead_letter`).
    pub(crate) fn has_retry_strategy_attrs(&self) -> bool {
        self.retry_strategy.is_some() || self.retry_base_ms.is_some() || self.retry_max_ms.is_some()
    }
}

// ── Parsed state parameter ───────────────────────────────────────────────────

/// A `Component(name): Component<Type>` parameter extracted from the function signature.
pub(crate) struct StateParam {
    /// The binding name (e.g. `db` from `Component(db): Component<DbPool>`)
    pub name: Ident,
    /// The inner type (e.g. `DbPool`)
    pub inner_ty: Type,
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_str;

    /// Helper: parse a `#[event_listener(...)]` attribute body into `ListenerAttrs`.
    fn parse_attrs(input: &str) -> syn::Result<ListenerAttrs> {
        parse_str::<ListenerAttrs>(input)
    }

    #[test]
    fn parse_empty_attrs() {
        let attrs = parse_attrs("").unwrap();
        assert!(attrs.retries.is_none());
        assert!(attrs.retry_strategy.is_none());
        assert!(attrs.retry_base_ms.is_none());
        assert!(attrs.retry_max_ms.is_none());
        assert!(attrs.dead_letter.is_none());
        assert!(attrs.name.is_none());
    }

    #[test]
    fn parse_name_attr() {
        let attrs = parse_attrs(r#"name = "custom""#).unwrap();
        assert_eq!(attrs.name.as_deref(), Some("custom"));
    }

    #[test]
    fn parse_name_empty_string() {
        let attrs = parse_attrs(r#"name = """#).unwrap();
        assert_eq!(attrs.name.as_deref(), Some(""));
    }

    #[test]
    fn parse_name_with_retry_attrs() {
        let attrs = parse_attrs(r#"name = "x", retries = 3, dead_letter = true"#).unwrap();
        assert_eq!(attrs.name.as_deref(), Some("x"));
        assert_eq!(attrs.retries, Some(3));
        assert_eq!(attrs.dead_letter, Some(true));
    }

    #[test]
    fn parse_reject_duplicate_name() {
        let err = parse_attrs(r#"name = "a", name = "b""#).unwrap_err();
        assert!(err.to_string().contains("duplicate attribute `name`"));
    }

    #[test]
    fn parse_reject_non_string_name() {
        let err = parse_attrs("name = 42").unwrap_err();
        assert!(err.to_string().contains("expected string literal"));
    }

    #[test]
    fn has_failure_policy_excludes_name() {
        let attrs = parse_attrs(r#"name = "x""#).unwrap();
        assert!(!attrs.has_failure_policy());
        assert!(!attrs.has_retry_strategy_attrs());
    }

    #[test]
    fn parse_all_retry_strategies() {
        for strategy in &["fixed", "exponential", "exponential_jitter"] {
            let input = format!(r#"retry_strategy = "{strategy}""#);
            let attrs = parse_attrs(&input).unwrap();
            assert_eq!(attrs.retry_strategy.as_deref(), Some(*strategy));
        }
    }

    #[test]
    fn parse_reject_unknown_attr() {
        let err = parse_attrs("unknown = 1").unwrap_err();
        assert!(err.to_string().contains("unknown attribute `unknown`"));
    }

    #[test]
    fn parse_reject_unknown_retry_strategy() {
        let err = parse_attrs(r#"retry_strategy = "unknown""#).unwrap_err();
        assert!(err.to_string().contains("expected one of"));
    }

    #[test]
    fn parse_full_retry_config() {
        let attrs = parse_attrs(r#"retries = 5, retry_strategy = "fixed", retry_base_ms = 200, dead_letter = false"#).unwrap();
        assert_eq!(attrs.retries, Some(5));
        assert_eq!(attrs.dead_letter, Some(false));
        assert_eq!(attrs.retry_base_ms, Some(200));
        assert_eq!(attrs.retry_strategy.as_deref(), Some("fixed"));
        assert!(attrs.has_failure_policy());
        assert!(attrs.has_retry_strategy_attrs());
    }

    #[test]
    fn parse_exponential_config() {
        let attrs = parse_attrs(r#"retries = 3, retry_strategy = "exponential", retry_base_ms = 50, retry_max_ms = 5000"#).unwrap();
        assert_eq!(attrs.retries, Some(3));
        assert_eq!(attrs.retry_strategy.as_deref(), Some("exponential"));
        assert_eq!(attrs.retry_base_ms, Some(50));
        assert_eq!(attrs.retry_max_ms, Some(5000));
    }

    #[test]
    fn parse_reject_duplicate_retries() {
        let err = parse_attrs("retries = 1, retries = 2").unwrap_err();
        assert!(err.to_string().contains("duplicate attribute `retries`"));
    }
}

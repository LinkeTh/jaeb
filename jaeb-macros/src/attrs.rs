//! Parsing of `#[handler(...)]` attribute arguments.

use syn::Token;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;

/// Parsed attributes from:
/// `#[handler(retries = 3, retry_strategy = "fixed", retry_base_ms = 100, dead_letter = true)]`.
///
/// Retry strategy is specified via `retry_strategy = "..."` combined with
/// `retry_base_ms` and optionally `retry_max_ms`.
#[cfg_attr(test, derive(Debug))]
pub(crate) struct HandlerAttrs {
    pub retries: Option<usize>,
    pub retry_strategy: Option<String>,
    pub retry_base_ms: Option<u64>,
    pub retry_max_ms: Option<u64>,
    pub dead_letter: Option<bool>,
    pub priority: Option<i32>,
    /// Human-readable listener name. Defaults to the function name when unset.
    /// Set to `""` to opt out of automatic naming.
    pub name: Option<String>,
}

impl Parse for HandlerAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut retries = None;
        let mut retry_strategy = None;
        let mut retry_base_ms = None;
        let mut retry_max_ms = None;
        let mut dead_letter = None;
        let mut priority = None;
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
                "priority" => {
                    if priority.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `priority`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        priority = Some(lit.base10_parse()?);
                    } else if let syn::Expr::Unary(syn::ExprUnary {
                        op: syn::UnOp::Neg(_), expr, ..
                    }) = &pair.value
                    {
                        if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = expr.as_ref() {
                            let n: i32 = lit.base10_parse()?;
                            priority = Some(-n);
                        } else {
                            return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
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
                            "unknown attribute `{key}`, expected one of: retries, retry_strategy, retry_base_ms, retry_max_ms, dead_letter, priority, name"
                        ),
                    ));
                }
            }
        }

        Ok(HandlerAttrs {
            retries,
            retry_strategy,
            retry_base_ms,
            retry_max_ms,
            dead_letter,
            priority,
            name,
        })
    }
}

impl HandlerAttrs {
    /// Returns `true` if any subscription-policy attribute was specified.
    pub(crate) fn has_subscription_policy(&self) -> bool {
        self.retries.is_some()
            || self.retry_strategy.is_some()
            || self.retry_base_ms.is_some()
            || self.retry_max_ms.is_some()
            || self.dead_letter.is_some()
            || self.priority.is_some()
    }

    /// Returns `true` if any retry-related attribute was specified
    /// (excluding `retries` itself and `dead_letter`).
    pub(crate) fn has_retry_strategy_attrs(&self) -> bool {
        self.retry_strategy.is_some() || self.retry_base_ms.is_some() || self.retry_max_ms.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_str;

    fn parse_attrs(input: &str) -> syn::Result<HandlerAttrs> {
        parse_str::<HandlerAttrs>(input)
    }

    #[test]
    fn parse_empty_attrs() {
        let attrs = parse_attrs("").expect("should parse");
        assert!(attrs.retries.is_none());
        assert!(attrs.retry_strategy.is_none());
        assert!(attrs.retry_base_ms.is_none());
        assert!(attrs.retry_max_ms.is_none());
        assert!(attrs.dead_letter.is_none());
        assert!(attrs.priority.is_none());
        assert!(attrs.name.is_none());
    }

    #[test]
    fn parse_name_attr() {
        let attrs = parse_attrs(r#"name = "custom""#).expect("should parse");
        assert_eq!(attrs.name.as_deref(), Some("custom"));
    }

    #[test]
    fn parse_name_empty_string() {
        let attrs = parse_attrs(r#"name = """#).expect("should parse");
        assert_eq!(attrs.name.as_deref(), Some(""));
    }

    #[test]
    fn parse_name_with_retry_attrs() {
        let attrs = parse_attrs(r#"name = "x", retries = 3, dead_letter = true"#).expect("should parse");
        assert_eq!(attrs.name.as_deref(), Some("x"));
        assert_eq!(attrs.retries, Some(3));
        assert_eq!(attrs.dead_letter, Some(true));
    }

    #[test]
    fn parse_priority_positive() {
        let attrs = parse_attrs("priority = 10").expect("should parse");
        assert_eq!(attrs.priority, Some(10));
    }

    #[test]
    fn parse_priority_negative() {
        let attrs = parse_attrs("priority = -3").expect("should parse");
        assert_eq!(attrs.priority, Some(-3));
    }

    #[test]
    fn parse_reject_duplicate_name() {
        let err = parse_attrs(r#"name = "a", name = "b""#).expect_err("should fail");
        assert!(err.to_string().contains("duplicate attribute `name`"));
    }

    #[test]
    fn parse_reject_non_string_name() {
        let err = parse_attrs("name = 42").expect_err("should fail");
        assert!(err.to_string().contains("expected string literal"));
    }

    #[test]
    fn has_subscription_policy_excludes_name() {
        let attrs = parse_attrs(r#"name = "x""#).expect("should parse");
        assert!(!attrs.has_subscription_policy());
        assert!(!attrs.has_retry_strategy_attrs());
    }

    #[test]
    fn parse_all_retry_strategies() {
        for strategy in &["fixed", "exponential", "exponential_jitter"] {
            let input = format!(r#"retry_strategy = "{strategy}""#);
            let attrs = parse_attrs(&input).expect("should parse");
            assert_eq!(attrs.retry_strategy.as_deref(), Some(*strategy));
        }
    }

    #[test]
    fn parse_reject_unknown_attr() {
        let err = parse_attrs("unknown = 1").expect_err("should fail");
        assert!(err.to_string().contains("unknown attribute `unknown`"));
    }

    #[test]
    fn parse_reject_unknown_retry_strategy() {
        let err = parse_attrs(r#"retry_strategy = "unknown""#).expect_err("should fail");
        assert!(err.to_string().contains("expected one of"));
    }

    #[test]
    fn parse_full_retry_config() {
        let attrs = parse_attrs(r#"retries = 5, retry_strategy = "fixed", retry_base_ms = 200, dead_letter = false"#).expect("should parse");
        assert_eq!(attrs.retries, Some(5));
        assert_eq!(attrs.dead_letter, Some(false));
        assert_eq!(attrs.retry_base_ms, Some(200));
        assert_eq!(attrs.retry_strategy.as_deref(), Some("fixed"));
        assert!(attrs.has_subscription_policy());
        assert!(attrs.has_retry_strategy_attrs());
    }

    #[test]
    fn parse_exponential_config() {
        let attrs = parse_attrs(r#"retries = 3, retry_strategy = "exponential", retry_base_ms = 50, retry_max_ms = 5000"#).expect("should parse");
        assert_eq!(attrs.retries, Some(3));
        assert_eq!(attrs.retry_strategy.as_deref(), Some("exponential"));
        assert_eq!(attrs.retry_base_ms, Some(50));
        assert_eq!(attrs.retry_max_ms, Some(5000));
    }

    #[test]
    fn parse_reject_duplicate_retries() {
        let err = parse_attrs("retries = 1, retries = 2").expect_err("should fail");
        assert!(err.to_string().contains("duplicate attribute `retries`"));
    }

    #[test]
    fn parse_reject_non_int_priority() {
        let err = parse_attrs(r#"priority = "high""#).expect_err("should fail");
        assert!(err.to_string().contains("expected integer"));
    }
}

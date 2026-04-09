// SPDX-License-Identifier: MIT
//! Parsing of `#[event_listener(...)]` attribute arguments and state parameters.

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, Token, Type};

// ── Attribute arguments ──────────────────────────────────────────────────────

/// Parsed attributes from `#[event_listener(retries = 3, retry_delay_ms = 100, dead_letter = true)]`.
pub(crate) struct ListenerAttrs {
    pub retries: Option<usize>,
    pub retry_delay_ms: Option<u64>,
    pub dead_letter: Option<bool>,
}

impl Parse for ListenerAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut retries = None;
        let mut retry_delay_ms = None;
        let mut dead_letter = None;

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
                "retry_delay_ms" => {
                    if retry_delay_ms.is_some() {
                        return Err(syn::Error::new_spanned(&pair.path, "duplicate attribute `retry_delay_ms`"));
                    }
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retry_delay_ms = Some(lit.base10_parse()?);
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
                _ => {
                    return Err(syn::Error::new_spanned(
                        &pair.path,
                        format!("unknown attribute `{key}`, expected one of: retries, retry_delay_ms, dead_letter"),
                    ));
                }
            }
        }

        Ok(ListenerAttrs {
            retries,
            retry_delay_ms,
            dead_letter,
        })
    }
}

impl ListenerAttrs {
    /// Returns `true` if any failure-policy attribute was specified.
    pub(crate) fn has_failure_policy(&self) -> bool {
        self.retries.is_some() || self.retry_delay_ms.is_some() || self.dead_letter.is_some()
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

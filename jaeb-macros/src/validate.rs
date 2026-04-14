//! Validation and extraction helpers for handler function signatures.

use syn::punctuated::Punctuated;
use syn::{FnArg, Ident, Pat, PatType, Token, Type};

/// Extract `T` from `&T`.
pub(crate) fn extract_ref_type(ty: &Type) -> Option<&Type> {
    if let Type::Reference(syn::TypeReference { elem, mutability, .. }) = ty
        && mutability.is_none()
    {
        return Some(elem.as_ref());
    }
    None
}

/// Check if the type's last path segment is `DeadLetter`.
pub(crate) fn is_dead_letter_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty
        && let Some(seg) = type_path.path.segments.last()
    {
        return seg.ident == "DeadLetter";
    }
    false
}

/// Check if the type's last path segment is `HandlerResult`.
pub(crate) fn is_handler_result_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty
        && let Some(seg) = type_path.path.segments.last()
    {
        return seg.ident == "HandlerResult";
    }
    false
}

/// Check if a type is `&EventBus` (an immutable reference to `EventBus`).
/// Used by the handler macro to detect the optional bus parameter.
pub(crate) fn is_event_bus_ref_type(ty: &Type) -> bool {
    if let Type::Reference(syn::TypeReference { elem, mutability, .. }) = ty
        && mutability.is_none()
        && let Type::Path(type_path) = elem.as_ref()
        && let Some(seg) = type_path.path.segments.last()
    {
        return seg.ident == "EventBus";
    }
    false
}

// ── Dep<T> parameters ────────────────────────────────────────────────────────

/// A `Dep<T>` parameter extracted from a handler function signature.
///
/// Supports two syntax forms:
/// - `Dep(name): Dep<T>` — destructured; `name` binds directly to the inner `T`.
/// - `name: Dep<T>` — plain identifier; also binds to the inner `T`.
pub(crate) struct DepParam {
    /// The binding name (e.g. `db` from `Dep(db): Dep<DbPool>`).
    pub name: Ident,
    /// The inner dependency type (e.g. `DbPool`).
    pub inner_ty: Type,
}

/// Parse `Dep<T>` parameters from parameter position 1 onwards.
///
/// Returns `(dep_params, has_bus)`. `has_bus` is `true` if any parameter
/// at position 1+ has type `&EventBus`. A compile error is returned if a
/// parameter at position 1+ is neither `Dep<T>` nor `&EventBus`.
pub(crate) fn parse_dep_params(inputs: &Punctuated<FnArg, Token![,]>) -> syn::Result<(Vec<DepParam>, bool)> {
    let mut params = Vec::new();
    let mut has_bus = false;

    for arg in inputs.iter().skip(1) {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new_spanned(arg, "unexpected `self` parameter in handler"));
        };

        if is_event_bus_ref_type(&pat_ty.ty) {
            has_bus = true;
            continue;
        }

        let (name, inner_ty) = parse_dep_param(pat_ty)?;
        params.push(DepParam { name, inner_ty });
    }

    Ok((params, has_bus))
}

fn parse_dep_param(pat_ty: &PatType) -> syn::Result<(Ident, Type)> {
    // Verify the type annotation is `Dep<T>` and extract `T`.
    let inner_ty = extract_dep_inner_type(&pat_ty.ty).ok_or_else(|| {
        syn::Error::new_spanned(
            &pat_ty.ty,
            "handler dependency parameter type must be `Dep<T>`; \
             add `use jaeb::Dep` and annotate as `Dep(name): Dep<MyType>` or `name: Dep<MyType>`",
        )
    })?;

    // Extract the binding name from the pattern.
    let name = extract_dep_binding_name(&pat_ty.pat).ok_or_else(|| {
        syn::Error::new_spanned(
            &pat_ty.pat,
            "handler dependency parameter must use `Dep(name): Dep<T>` \
             (destructured) or `name: Dep<T>` (plain identifier) syntax",
        )
    })?;

    Ok((name, inner_ty))
}

/// Extract `T` from the type `Dep<T>`.
fn extract_dep_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty
        && let Some(seg) = type_path.path.segments.last()
        && seg.ident == "Dep"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && args.args.len() == 1
        && let syn::GenericArgument::Type(inner) = &args.args[0]
    {
        return Some(inner.clone());
    }
    None
}

/// Extract the binding name from a `Dep(name)` tuple-struct pattern or a
/// plain `name` identifier pattern.
fn extract_dep_binding_name(pat: &Pat) -> Option<Ident> {
    match pat {
        // `Dep(name)` form — TupleStruct pattern with one element where the struct path is `Dep`
        Pat::TupleStruct(pts) if pts.elems.len() == 1 && pts.path.segments.last().is_some_and(|s| s.ident == "Dep") => {
            if let Pat::Ident(pi) = &pts.elems[0] {
                Some(pi.ident.clone())
            } else {
                None
            }
        }
        // `name` form — plain identifier
        Pat::Ident(pi) => Some(pi.ident.clone()),
        _ => None,
    }
}

//! Validation and extraction helpers for event listener function signatures.

use syn::punctuated::Punctuated;
use syn::{FnArg, Ident, Pat, PatType, Token, Type};

use crate::attrs::StateParam;

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

/// Check if a type is `&EventBus`.
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

/// Parse `Component(name): Component<Type>` parameters from position 1 onwards.
///
/// Returns `(state_params, has_bus)`. `has_bus` is `true` if any parameter
/// at position 1+ has type `&EventBus`.
pub(crate) fn parse_state_params(inputs: &Punctuated<FnArg, Token![,]>) -> syn::Result<(Vec<StateParam>, bool)> {
    let mut params = Vec::new();
    let mut has_bus = false;

    for arg in inputs.iter().skip(1) {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new_spanned(arg, "unexpected `self` parameter in event listener"));
        };

        if is_event_bus_ref_type(&pat_ty.ty) {
            has_bus = true;
            continue;
        }

        let (name, inner_ty) = parse_component_param(pat_ty)?;
        params.push(StateParam { name, inner_ty });
    }

    Ok((params, has_bus))
}

/// Parse a single `Component(name): Component<Type>` parameter.
fn parse_component_param(pat_ty: &PatType) -> syn::Result<(Ident, Type)> {
    // Extract the binding name from the pattern `Component(name)`
    let name = extract_tuple_struct_binding(&pat_ty.pat)
        .ok_or_else(|| syn::Error::new_spanned(&pat_ty.pat, "state parameter must use `Component(name): Component<Type>` syntax"))?;

    // Extract the inner type from `Component<Type>`
    let inner_ty = extract_component_inner_type(&pat_ty.ty)
        .ok_or_else(|| syn::Error::new_spanned(&pat_ty.ty, "state parameter type must be `Component<Type>`"))?;

    Ok((name, inner_ty))
}

/// Extract `name` from pattern `Component(name)`.
fn extract_tuple_struct_binding(pat: &Pat) -> Option<Ident> {
    if let Pat::TupleStruct(pts) = pat
        && pts.elems.len() == 1
        && let Pat::Ident(pi) = &pts.elems[0]
    {
        return Some(pi.ident.clone());
    }
    None
}

/// Extract `T` from `Component<T>`.
fn extract_component_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty
        && let Some(seg) = type_path.path.segments.last()
        && seg.ident == "Component"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && args.args.len() == 1
        && let syn::GenericArgument::Type(inner) = &args.args[0]
    {
        return Some(inner.clone());
    }
    None
}

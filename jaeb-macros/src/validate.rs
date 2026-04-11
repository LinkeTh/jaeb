//! Validation and extraction helpers for handler function signatures.

use syn::Type;

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

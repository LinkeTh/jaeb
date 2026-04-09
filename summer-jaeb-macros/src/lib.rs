// SPDX-License-Identifier: MIT
//! Proc macros for `summer-jaeb` event listener auto-registration.
//!
//! Provides the `#[event_listener]` attribute macro that generates:
//! - A handler struct implementing `EventHandler<E>` (async) or `SyncEventHandler<E>` (sync)
//! - A registrar struct implementing `TypedListenerRegistrar`
//! - An `inventory::submit!` call for auto-collection by `SummerJaeb` plugin
//!
//! This crate is not intended for direct use — import `event_listener` from `summer_jaeb` instead.

mod attrs;
mod codegen;
mod validate;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{FnArg, ItemFn, ReturnType, parse_macro_input};

use attrs::ListenerAttrs;
use codegen::{gen_async_handler_impl, gen_stateful_structs, gen_stateless_structs, gen_subscribe_call, gen_sync_handler_impl};
use validate::{extract_ref_type, is_dead_letter_type, is_handler_result_type, parse_state_params};

/// Marks a free function as an event listener that is auto-registered by the `SummerJaeb` plugin.
///
/// # Async listener (implements `EventHandler<E>`)
///
/// ```rust,ignore
/// #[event_listener]
/// async fn on_order_placed(event: &OrderPlacedEvent) -> HandlerResult {
///     info!(order_id = event.order_id, "order placed");
///     Ok(())
/// }
/// ```
///
/// # Sync listener (implements `SyncEventHandler<E>`)
///
/// ```rust,ignore
/// #[event_listener]
/// fn on_order_shipped(event: &OrderShippedEvent) -> HandlerResult {
///     info!(order_id = event.order_id, "order shipped");
///     Ok(())
/// }
/// ```
///
/// # With state injection
///
/// ```rust,ignore
/// #[event_listener]
/// async fn on_order_placed(event: &OrderPlacedEvent, Component(db): Component<DbPool>) -> HandlerResult {
///     db.save(&event).await?;
///     Ok(())
/// }
/// ```
///
/// # Failure policy attributes
///
/// ```rust,ignore
/// #[event_listener(retries = 3, retry_delay_ms = 100, dead_letter = true)]
/// async fn flaky_handler(event: &SomeEvent) -> HandlerResult {
///     // ...
///     Ok(())
/// }
/// ```
///
/// # Dead letter listener
///
/// When the event type is `DeadLetter`, `subscribe_dead_letters()` is used automatically
/// to prevent infinite recursion.
///
/// ```rust,ignore
/// #[event_listener]
/// fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
///     eprintln!("dead letter: {:?}", event);
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn event_listener(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as ListenerAttrs);
    let func = parse_macro_input!(item as ItemFn);

    match expand_event_listener(attrs, func) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_event_listener(attrs: ListenerAttrs, func: ItemFn) -> syn::Result<TokenStream2> {
    // Validate: must be a free function (no self parameter)
    if func.sig.inputs.first().is_some_and(|arg| matches!(arg, FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[event_listener] can only be applied to free functions, not methods",
        ));
    }

    // Validate: must have at least one parameter (the event)
    if func.sig.inputs.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[event_listener] function must have at least one parameter (the event)",
        ));
    }

    // Validate: must have an explicit return type
    if matches!(func.sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[event_listener] function must have an explicit return type (-> HandlerResult)",
        ));
    }

    // Validate: return type should be HandlerResult
    if let ReturnType::Type(_, ref ty) = func.sig.output
        && !is_handler_result_type(ty)
    {
        return Err(syn::Error::new_spanned(ty, "#[event_listener] function must return `HandlerResult`"));
    }

    let is_async = func.sig.asyncness.is_some();
    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();

    // Parse the first parameter as the event: `event: &EventType`
    let first_param = func.sig.inputs.first().expect("already validated non-empty");

    let FnArg::Typed(event_pat_ty) = first_param else {
        return Err(syn::Error::new_spanned(first_param, "first parameter must be the event (not `self`)"));
    };

    // Extract the event type from `&EventType`
    let event_ty = extract_ref_type(&event_pat_ty.ty)
        .ok_or_else(|| syn::Error::new_spanned(&event_pat_ty.ty, "event parameter must be a reference: `event: &EventType`"))?;

    // Check if this is a DeadLetter listener (last path segment is "DeadLetter")
    let is_dead_letter = is_dead_letter_type(event_ty);

    // DeadLetter listeners must be synchronous (subscribe_dead_letters requires SyncEventHandler)
    if is_dead_letter && is_async {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "DeadLetter listeners must be synchronous — remove `async` (subscribe_dead_letters requires SyncEventHandler)",
        ));
    }

    // Failure policy attributes are not supported on DeadLetter listeners
    // (subscribe_dead_letters hard-codes its own policy with dead_letter: false)
    if is_dead_letter && attrs.has_failure_policy() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "failure policy attributes (retries, retry_delay_ms, dead_letter) are not supported on DeadLetter listeners",
        ));
    }

    // Warn about likely mistakes in failure policy configuration
    if attrs.retry_delay_ms.is_some() && attrs.retries.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retry_delay_ms` has no effect without `retries`; add `retries = N` or remove `retry_delay_ms`",
        ));
    }

    // Retry attributes are only supported on async handlers — sync handlers
    // execute exactly once.
    if !is_async && (attrs.retries.is_some() || attrs.retry_delay_ms.is_some()) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retries` and `retry_delay_ms` are only supported on async handlers — sync handlers execute exactly once (failures produce dead letters when enabled)",
        ));
    }

    // Parse remaining parameters as Component state
    let state_params = parse_state_params(&func.sig.inputs)?;
    let has_state = !state_params.is_empty();

    // Build the generated code
    let handler_trait_impl = if is_async {
        gen_async_handler_impl(event_ty, fn_name, &state_params)
    } else {
        gen_sync_handler_impl(event_ty, fn_name, &state_params)
    };

    let subscribe_call = gen_subscribe_call(event_ty, is_dead_letter, &attrs, &fn_name_str, has_state);

    let (handler_struct, registrar_struct, inventory_submit) = if has_state {
        gen_stateful_structs(&state_params, &fn_name_str, &subscribe_call)
    } else {
        gen_stateless_structs(&fn_name_str, &subscribe_call, &handler_trait_impl)
    };

    // For stateful, handler_trait_impl is separate from registrar
    let handler_impl_in_const = if has_state {
        handler_trait_impl
    } else {
        // Already included in gen_stateless_structs
        quote! {}
    };

    Ok(quote! {
        #[allow(dead_code)]
        #func

        const _: () = {
            #handler_struct
            #handler_impl_in_const
            #registrar_struct
            #inventory_submit
        };
    })
}

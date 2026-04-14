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
/// // Fixed delay
/// #[event_listener(retries = 3, retry_strategy = "fixed", retry_base_ms = 100, dead_letter = true)]
/// async fn flaky_handler(event: &SomeEvent) -> HandlerResult {
///     // ...
///     Ok(())
/// }
///
/// // Exponential back-off
/// #[event_listener(retries = 5, retry_strategy = "exponential", retry_base_ms = 50, retry_max_ms = 5000)]
/// async fn backoff_handler(event: &SomeEvent) -> HandlerResult {
///     // ...
///     Ok(())
/// }
///
/// // Exponential back-off with jitter
/// #[event_listener(retries = 5, retry_strategy = "exponential_jitter", retry_base_ms = 50, retry_max_ms = 5000)]
/// async fn jitter_handler(event: &SomeEvent) -> HandlerResult {
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
///
/// # Listener naming
///
/// By default the function name is used as the listener name (visible in traces,
/// dead-letter records, and `BusStats`). Override with `name = "..."` or opt
/// out with `name = ""`:
///
/// ```rust,ignore
/// #[event_listener(name = "order-processor")]
/// async fn handle_order(event: &OrderEvent) -> HandlerResult {
///     Ok(())
/// }
///
/// // Opt out — name() returns None
/// #[event_listener(name = "")]
/// async fn anonymous(event: &OrderEvent) -> HandlerResult {
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

    // Subscription policy attributes are not supported on DeadLetter listeners
    // (subscribe_dead_letters hard-codes its own policy with dead_letter: false)
    if is_dead_letter && attrs.has_subscription_policy() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "subscription policy attributes (retries, retry_strategy, \
             retry_base_ms, retry_max_ms, dead_letter, priority) are not supported on DeadLetter listeners",
        ));
    }

    // Parse remaining parameters as Component state
    let (state_params, has_bus) = parse_state_params(&func.sig.inputs)?;
    let has_state = !state_params.is_empty();

    if is_dead_letter && has_bus {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "DeadLetter listeners cannot declare a `bus: &EventBus` parameter",
        ));
    }

    // `retry_base_ms` / `retry_max_ms` are only valid with explicit `retry_strategy`.
    if (attrs.retry_base_ms.is_some() || attrs.retry_max_ms.is_some()) && attrs.retry_strategy.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retry_base_ms` and `retry_max_ms` require `retry_strategy`; \
             add `retry_strategy = \"exponential\"` (or `\"exponential_jitter\"` / `\"fixed\"`)",
        ));
    }

    // Exponential strategies require both `retry_base_ms` and `retry_max_ms`.
    if let Some(ref strategy) = attrs.retry_strategy {
        match strategy.as_str() {
            "exponential" | "exponential_jitter" => {
                if attrs.retry_base_ms.is_none() || attrs.retry_max_ms.is_none() {
                    return Err(syn::Error::new_spanned(
                        &func.sig,
                        format!(
                            "`retry_strategy = \"{strategy}\"` requires both \
                             `retry_base_ms` and `retry_max_ms`"
                        ),
                    ));
                }
            }
            "fixed" => {
                if attrs.retry_base_ms.is_none() {
                    return Err(syn::Error::new_spanned(
                        &func.sig,
                        "`retry_strategy = \"fixed\"` requires `retry_base_ms`",
                    ));
                }
            }
            _ => {} // unreachable, validated in parsing
        }
    }

    // Any retry strategy attributes without `retries` are useless.
    if attrs.has_retry_strategy_attrs() && attrs.retries.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "retry strategy attributes have no effect without `retries`; \
             add `retries = N` or remove retry-related attributes",
        ));
    }

    // Retry attributes are only supported on async handlers — sync handlers
    // execute exactly once.
    if !is_async && (attrs.retries.is_some() || attrs.has_retry_strategy_attrs()) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retries` and retry strategy attributes are only supported on async handlers \
             — sync handlers execute exactly once (failures produce dead letters when enabled)",
        ));
    }

    if is_async && attrs.priority.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`priority` attribute is only supported on sync handlers",
        ));
    }

    // ── Listener name resolution ─────────────────────────────────────────
    //
    //  - Default (no `name` attr): use the function name
    //  - `name = "custom"`:        use the custom name
    //  - `name = ""`:              opt out of naming (None)
    let listener_name: Option<&str> = match &attrs.name {
        Some(n) if n.is_empty() => None,
        Some(n) => Some(n.as_str()),
        None => Some(fn_name_str.as_str()),
    };

    // Build the generated code
    let handler_trait_impl = if is_async {
        gen_async_handler_impl(event_ty, fn_name, &state_params, listener_name, has_bus)
    } else {
        gen_sync_handler_impl(event_ty, fn_name, &state_params, listener_name, has_bus)
    };

    let subscribe_call = gen_subscribe_call(event_ty, is_dead_letter, &attrs, &fn_name_str, has_state, is_async);

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

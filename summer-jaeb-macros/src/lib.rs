// SPDX-License-Identifier: MIT
//! Proc macros for `summer-jaeb` event listener auto-registration.
//!
//! Provides the `#[event_listener]` attribute macro that generates:
//! - A handler struct implementing `EventHandler<E>` (async) or `SyncEventHandler<E>` (sync)
//! - A registrar struct implementing `TypedListenerRegistrar`
//! - An `inventory::submit!` call for auto-collection by `SummerJaeb` plugin
//!
//! This crate is not intended for direct use — import `event_listener` from `summer_jaeb` instead.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{FnArg, Ident, ItemFn, Pat, PatType, ReturnType, Token, Type, parse_macro_input};

// ── Attribute arguments ──────────────────────────────────────────────────────

/// Parsed attributes from `#[event_listener(retries = 3, retry_delay_ms = 100, dead_letter = true)]`.
struct ListenerAttrs {
    retries: Option<usize>,
    retry_delay_ms: Option<u64>,
    dead_letter: Option<bool>,
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
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retries = Some(lit.base10_parse()?);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                    }
                }
                "retry_delay_ms" => {
                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = &pair.value {
                        retry_delay_ms = Some(lit.base10_parse()?);
                    } else {
                        return Err(syn::Error::new_spanned(&pair.value, "expected integer"));
                    }
                }
                "dead_letter" => {
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

// ── Parsed state parameter ───────────────────────────────────────────────────

/// A `Component(name): Component<Type>` parameter extracted from the function signature.
struct StateParam {
    /// The binding name (e.g. `db` from `Component(db): Component<DbPool>`)
    name: Ident,
    /// The inner type (e.g. `DbPool`)
    inner_ty: Type,
}

// ── Main macro ───────────────────────────────────────────────────────────────

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
    if is_dead_letter && has_failure_policy(&attrs) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "failure policy attributes (retries, retry_delay_ms, dead_letter) are not supported on DeadLetter listeners",
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

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract `T` from `&T`.
fn extract_ref_type(ty: &Type) -> Option<&Type> {
    if let Type::Reference(syn::TypeReference { elem, mutability, .. }) = ty {
        if mutability.is_none() {
            return Some(elem.as_ref());
        }
    }
    None
}

/// Check if the type's last path segment is `DeadLetter`.
fn is_dead_letter_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == "DeadLetter";
        }
    }
    false
}

/// Parse `Component(name): Component<Type>` parameters from position 1 onwards.
fn parse_state_params(inputs: &Punctuated<FnArg, Token![,]>) -> syn::Result<Vec<StateParam>> {
    let mut params = Vec::new();

    for arg in inputs.iter().skip(1) {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new_spanned(arg, "unexpected `self` parameter in event listener"));
        };

        let (name, inner_ty) = parse_component_param(pat_ty)?;
        params.push(StateParam { name, inner_ty });
    }

    Ok(params)
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
    if let Pat::TupleStruct(pts) = pat {
        if pts.elems.len() == 1 {
            if let Pat::Ident(pi) = &pts.elems[0] {
                return Some(pi.ident.clone());
            }
        }
    }
    None
}

/// Extract `T` from `Component<T>`.
fn extract_component_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            if seg.ident == "Component" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if args.args.len() == 1 {
                        if let syn::GenericArgument::Type(inner) = &args.args[0] {
                            return Some(inner.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

// ── Code generation ──────────────────────────────────────────────────────────

/// Generate `impl EventHandler<E> for Handler { ... }` (async).
fn gen_async_handler_impl(event_ty: &Type, fn_name: &Ident, state_params: &[StateParam]) -> TokenStream2 {
    let call_args = gen_handler_call_args(state_params);

    quote! {
        impl ::jaeb::EventHandler<#event_ty> for Handler {
            async fn handle(&self, event: &#event_ty) -> ::jaeb::HandlerResult {
                #fn_name(event, #call_args).await
            }
        }
    }
}

/// Generate `impl SyncEventHandler<E> for Handler { ... }` (sync).
fn gen_sync_handler_impl(event_ty: &Type, fn_name: &Ident, state_params: &[StateParam]) -> TokenStream2 {
    let call_args = gen_handler_call_args(state_params);

    quote! {
        impl ::jaeb::SyncEventHandler<#event_ty> for Handler {
            fn handle(&self, event: &#event_ty) -> ::jaeb::HandlerResult {
                #fn_name(event, #call_args)
            }
        }
    }
}

/// Generate the argument expressions for calling the original function from inside the handler.
/// For state params, generates `Component(self.field_name.clone())`.
fn gen_handler_call_args(state_params: &[StateParam]) -> TokenStream2 {
    let args: Vec<TokenStream2> = state_params
        .iter()
        .map(|sp| {
            let name = &sp.name;
            quote! { ::summer::extractor::Component(self.#name.clone()) }
        })
        .collect();

    quote! { #(#args),* }
}

/// Generate the subscribe call for the registrar's `register` method.
fn gen_subscribe_call(event_ty: &Type, is_dead_letter: bool, attrs: &ListenerAttrs, fn_name_str: &str, has_state: bool) -> TokenStream2 {
    let handler_expr = if has_state {
        // State resolved at register-time, construct Handler with fields
        quote! { handler }
    } else {
        quote! { Handler }
    };

    let subscribe_msg = format!("summer-jaeb: failed to subscribe listener '{fn_name_str}'");

    if is_dead_letter {
        // DeadLetter listeners always use subscribe_dead_letters (sync only)
        quote! {
            let _sub = bus.subscribe_dead_letters(#handler_expr)
                .await
                .expect(#subscribe_msg);
        }
    } else if has_failure_policy(attrs) {
        let policy = gen_failure_policy(attrs);
        quote! {
            let _sub = bus.subscribe_with_policy::<#event_ty, _, _>(#handler_expr, #policy)
                .await
                .expect(#subscribe_msg);
        }
    } else {
        quote! {
            let _sub = bus.subscribe::<#event_ty, _, _>(#handler_expr)
                .await
                .expect(#subscribe_msg);
        }
    }
}

fn has_failure_policy(attrs: &ListenerAttrs) -> bool {
    attrs.retries.is_some() || attrs.retry_delay_ms.is_some() || attrs.dead_letter.is_some()
}

fn gen_failure_policy(attrs: &ListenerAttrs) -> TokenStream2 {
    let mut chain = quote! { ::jaeb::FailurePolicy::default() };

    if let Some(r) = attrs.retries {
        chain = quote! { #chain.with_max_retries(#r) };
    }
    if let Some(ms) = attrs.retry_delay_ms {
        chain = quote! { #chain.with_retry_delay(::core::time::Duration::from_millis(#ms)) };
    }
    if let Some(dl) = attrs.dead_letter {
        chain = quote! { #chain.with_dead_letter(#dl) };
    }

    chain
}

/// Generate structs for stateless listeners (no Component params).
/// Single `Handler` struct implements both the handler trait and `TypedListenerRegistrar`.
fn gen_stateless_structs(
    _fn_name_str: &str,
    subscribe_call: &TokenStream2,
    handler_trait_impl: &TokenStream2,
) -> (TokenStream2, TokenStream2, TokenStream2) {
    let handler_struct = quote! {
        struct Handler;

        #handler_trait_impl
    };

    let registrar_struct = quote! {
        impl ::summer_jaeb::TypedListenerRegistrar for Handler {
            fn register<'a>(
                &self,
                bus: &'a ::jaeb::EventBus,
                _app: &'a ::summer::app::AppBuilder,
            ) -> ::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = ()> + Send + 'a>> {
                ::std::boxed::Box::pin(async move {
                    #subscribe_call
                })
            }
        }
    };

    let inventory_submit = quote! {
        ::summer_jaeb::_private::inventory::submit! {
            &Handler as &dyn ::summer_jaeb::TypedListenerRegistrar
        }
    };

    // handler_struct already contains the trait impl, so handler_impl_in_const will be empty
    (handler_struct, registrar_struct, inventory_submit)
}

/// Generate structs for stateful listeners (with Component params).
/// Separate `Handler` (with state fields) and `Registrar` (stateless, submitted to inventory).
fn gen_stateful_structs(state_params: &[StateParam], fn_name_str: &str, subscribe_call: &TokenStream2) -> (TokenStream2, TokenStream2, TokenStream2) {
    let field_defs: Vec<TokenStream2> = state_params
        .iter()
        .map(|sp| {
            let name = &sp.name;
            let ty = &sp.inner_ty;
            quote! { #name: #ty }
        })
        .collect();

    let field_resolutions: Vec<TokenStream2> = state_params
        .iter()
        .map(|sp| {
            let name = &sp.name;
            let ty = &sp.inner_ty;
            let err_msg = format!("summer-jaeb: missing component {} for listener '{}'", quote! { #ty }, fn_name_str);
            quote! {
                let #name: #ty = ::summer::plugin::ComponentRegistry::get_component(app)
                    .expect(#err_msg);
            }
        })
        .collect();

    let field_names: Vec<&Ident> = state_params.iter().map(|sp| &sp.name).collect();

    let handler_struct = quote! {
        struct Handler {
            #(#field_defs,)*
        }
    };

    let registrar_struct = quote! {
        struct Registrar;

        impl ::summer_jaeb::TypedListenerRegistrar for Registrar {
            fn register<'a>(
                &self,
                bus: &'a ::jaeb::EventBus,
                app: &'a ::summer::app::AppBuilder,
            ) -> ::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = ()> + Send + 'a>> {
                ::std::boxed::Box::pin(async move {
                    #(#field_resolutions)*
                    let handler = Handler { #(#field_names,)* };
                    #subscribe_call
                })
            }
        }
    };

    let inventory_submit = quote! {
        ::summer_jaeb::_private::inventory::submit! {
            &Registrar as &dyn ::summer_jaeb::TypedListenerRegistrar
        }
    };

    (handler_struct, registrar_struct, inventory_submit)
}

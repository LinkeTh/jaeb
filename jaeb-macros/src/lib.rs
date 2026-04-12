//! Proc macros for `jaeb` handler generation and registration.

mod attrs;
mod codegen;
mod validate;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, ReturnType, parse_macro_input};

use attrs::HandlerAttrs;
use codegen::{
    gen_async_handler_impl, gen_async_handler_impl_resolved, gen_dead_letter_descriptor_impl, gen_dead_letter_descriptor_impl_with_deps,
    gen_handler_descriptor_impl, gen_handler_descriptor_impl_with_deps, gen_resolved_struct, gen_sync_handler_impl, gen_sync_handler_impl_resolved,
    handler_struct_ident, resolved_struct_ident,
};
use validate::{extract_ref_type, is_dead_letter_type, is_handler_result_type, parse_dep_params};

/// Marks a free function as a jaeb event handler and generates a companion
/// handler struct that implements [`HandlerDescriptor`](::jaeb::HandlerDescriptor).
///
/// The macro emits:
/// - A `#[doc(hidden)]` struct named `<FunctionNamePascalCase>Handler`.
/// - A same-name `const` matching the original function name, so you can pass
///   the handler to the builder using the function name directly.
///
/// # Async handler
///
/// ```rust,ignore
/// #[handler]
/// async fn on_order(event: &OrderPlaced) -> HandlerResult {
///     Ok(())
/// }
///
/// let bus = EventBus::builder()
///     .handler(on_order)   // pass the function name directly
///     .build()
///     .await?;
/// ```
///
/// # Sync handler
///
/// ```rust,ignore
/// #[handler]
/// fn audit(event: &OrderPlaced) -> HandlerResult {
///     Ok(())
/// }
///
/// let bus = EventBus::builder()
///     .handler(audit)
///     .build()
///     .await?;
/// ```
///
/// # Policy attributes
///
/// ```rust,ignore
/// #[handler(retries = 3, retry_strategy = "fixed", retry_base_ms = 100, dead_letter = true, priority = 10)]
/// async fn flaky(event: &OrderPlaced) -> HandlerResult {
///     Ok(())
/// }
///
/// let bus = EventBus::builder()
///     .handler(flaky)
///     .build()
///     .await?;
/// ```
///
/// # Dependency injection with `Dep<T>`
///
/// Handlers can declare [`Dep<T>`](::jaeb::Dep) parameters (position 1 and
/// onward) to receive dependencies resolved from the [`Deps`](::jaeb::Deps)
/// container at build time. Each `T` is cloned per invocation, so use
/// `Arc<T>` for non-`Clone` types.
///
/// Two syntax forms are supported:
/// - `Dep(name): Dep<T>` — destructured; `name` binds directly to the inner `T`.
/// - `name: Dep<T>` — plain identifier; `name` is the whole `Dep<T>` wrapper
///   (access inner value via `name.0`).
///
/// ```rust,ignore
/// #[handler]
/// async fn process_order(event: &OrderPlaced, Dep(db): Dep<Arc<DbPool>>) -> HandlerResult {
///     db.save(event).await?;
///     Ok(())
/// }
///
/// let bus = EventBus::builder()
///     .handler(process_order)
///     .deps(Deps::new().insert(Arc::new(db_pool)))
///     .build()
///     .await?;
/// ```
///
/// A missing dependency causes `build()` to return
/// [`EventBusError::MissingDependency`](::jaeb::EventBusError::MissingDependency).
///
/// # Dead-letter handlers
///
/// Handlers for [`DeadLetter`](::jaeb::DeadLetter) events **must** use
/// [`#[dead_letter_handler]`](dead_letter_handler) instead. Using `#[handler]`
/// on a function that takes `&DeadLetter` is a compile-time error.
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as HandlerAttrs);
    let func = parse_macro_input!(item as ItemFn);

    match expand_handler(attrs, func) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Marks a free function as a jaeb dead-letter handler and generates a
/// companion handler struct that implements
/// [`DeadLetterDescriptor`](::jaeb::DeadLetterDescriptor).
///
/// The function **must** be synchronous and take `&DeadLetter` as its first
/// parameter. Subscription-policy attributes (`retries`, `retry_strategy`,
/// etc.) are not supported.
///
/// The macro emits:
/// - A `#[doc(hidden)]` struct named `<FunctionNamePascalCase>Handler`.
/// - A same-name `const` matching the original function name, so you can pass
///   the handler to the builder using the function name directly.
///
/// ```rust,ignore
/// #[dead_letter_handler]
/// fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
///     eprintln!("dead letter: {:?}", event.event_name);
///     Ok(())
/// }
///
/// let bus = EventBus::builder()
///     .dead_letter(on_dead_letter)   // pass the function name directly
///     .build()
///     .await?;
/// ```
///
/// # Dependency injection with `Dep<T>`
///
/// Like `#[handler]`, dead-letter handlers can declare [`Dep<T>`](::jaeb::Dep)
/// parameters to receive build-time dependencies:
///
/// ```rust,ignore
/// #[dead_letter_handler]
/// fn log_dead_letter(event: &DeadLetter, Dep(log): Dep<Arc<AuditLog>>) -> HandlerResult {
///     log.record(event);
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn dead_letter_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as HandlerAttrs);
    let func = parse_macro_input!(item as ItemFn);

    match expand_dead_letter_handler(attrs, func) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_handler(attrs: HandlerAttrs, func: ItemFn) -> syn::Result<TokenStream2> {
    if func.sig.inputs.first().is_some_and(|arg| matches!(arg, FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[handler] can only be applied to free functions, not methods",
        ));
    }

    if func.sig.inputs.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[handler] function must have at least one parameter (the event)",
        ));
    }

    if matches!(func.sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[handler] function must have an explicit return type (-> HandlerResult)",
        ));
    }

    if let ReturnType::Type(_, ref ty) = func.sig.output
        && !is_handler_result_type(ty)
    {
        return Err(syn::Error::new_spanned(ty, "#[handler] function must return `HandlerResult`"));
    }

    let is_async = func.sig.asyncness.is_some();
    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();

    let first_param = func.sig.inputs.first().expect("validated non-empty");
    let FnArg::Typed(event_pat_ty) = first_param else {
        return Err(syn::Error::new_spanned(first_param, "first parameter must be the event (not `self`)"));
    };

    let event_ty = extract_ref_type(&event_pat_ty.ty)
        .ok_or_else(|| syn::Error::new_spanned(&event_pat_ty.ty, "event parameter must be a reference: `event: &EventType`"))?;

    if is_dead_letter_type(event_ty) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "use `#[dead_letter_handler]` for DeadLetter handlers — `#[handler]` cannot be used with `&DeadLetter`",
        ));
    }

    // Parse optional Dep<T> parameters (position 1+).
    let dep_params = parse_dep_params(&func.sig.inputs)?;
    let has_deps = !dep_params.is_empty();

    if (attrs.retry_base_ms.is_some() || attrs.retry_max_ms.is_some()) && attrs.retry_strategy.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retry_base_ms` and `retry_max_ms` require `retry_strategy`; add `retry_strategy = \"exponential\"` (or `\"exponential_jitter\"` / `\"fixed\")",
        ));
    }

    if let Some(ref strategy) = attrs.retry_strategy {
        match strategy.as_str() {
            "exponential" | "exponential_jitter" => {
                if attrs.retry_base_ms.is_none() || attrs.retry_max_ms.is_none() {
                    return Err(syn::Error::new_spanned(
                        &func.sig,
                        format!("`retry_strategy = \"{strategy}\"` requires both `retry_base_ms` and `retry_max_ms`"),
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
            _ => {}
        }
    }

    if attrs.has_retry_strategy_attrs() && attrs.retries.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "retry strategy attributes have no effect without `retries`; add `retries = N` or remove retry-related attributes",
        ));
    }

    if !is_async && (attrs.retries.is_some() || attrs.has_retry_strategy_attrs()) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`retries` and retry strategy attributes are only supported on async handlers — sync handlers execute exactly once (failures produce dead letters when enabled)",
        ));
    }

    let handler_name: Option<&str> = match &attrs.name {
        Some(n) if n.is_empty() => None,
        Some(n) => Some(n.as_str()),
        None => Some(fn_name_str.as_str()),
    };

    let handler_ident = handler_struct_ident(fn_name);
    // Preserve the original function's visibility so the const and struct match it.
    let vis = &func.vis;

    // Rename the inner function to avoid a name clash between the emitted
    // function and the same-name const we generate below.  The inner function
    // is forced to private visibility because it is an implementation detail;
    // the public surface is the const and the struct.
    let inner_fn_ident = format_ident!("__jaeb_{}", fn_name);
    let mut inner_func = func.clone();
    inner_func.sig.ident = inner_fn_ident.clone();
    inner_func.vis = syn::Visibility::Inherited;

    if has_deps {
        let resolved_ident = resolved_struct_ident(fn_name);
        let resolved_struct = gen_resolved_struct(&resolved_ident, &dep_params);
        let handler_impl = if is_async {
            gen_async_handler_impl_resolved(&resolved_ident, event_ty, &inner_fn_ident, &dep_params, handler_name)
        } else {
            gen_sync_handler_impl_resolved(&resolved_ident, event_ty, &inner_fn_ident, &dep_params, handler_name)
        };
        let descriptor_impl = gen_handler_descriptor_impl_with_deps(&handler_ident, &resolved_ident, event_ty, &attrs, is_async, &dep_params);

        Ok(quote! {
            #[allow(dead_code)]
            #inner_func

            #resolved_struct

            #handler_impl

            #[doc(hidden)]
            #vis struct #handler_ident;

            #descriptor_impl

            #[allow(non_upper_case_globals)]
            #vis const #fn_name: #handler_ident = #handler_ident;
        })
    } else {
        let handler_impl = if is_async {
            gen_async_handler_impl(&handler_ident, event_ty, &inner_fn_ident, handler_name)
        } else {
            gen_sync_handler_impl(&handler_ident, event_ty, &inner_fn_ident, handler_name)
        };
        let descriptor_impl = gen_handler_descriptor_impl(&handler_ident, event_ty, &attrs, is_async);

        Ok(quote! {
            #[allow(dead_code)]
            #inner_func

            #[doc(hidden)]
            #vis struct #handler_ident;

            #handler_impl

            #descriptor_impl

            #[allow(non_upper_case_globals)]
            #vis const #fn_name: #handler_ident = #handler_ident;
        })
    }
}

fn expand_dead_letter_handler(attrs: HandlerAttrs, func: ItemFn) -> syn::Result<TokenStream2> {
    if func.sig.inputs.first().is_some_and(|arg| matches!(arg, FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[dead_letter_handler] can only be applied to free functions, not methods",
        ));
    }

    if func.sig.inputs.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[dead_letter_handler] function must have one parameter: `event: &DeadLetter`",
        ));
    }

    if matches!(func.sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[dead_letter_handler] function must have an explicit return type (-> HandlerResult)",
        ));
    }

    if let ReturnType::Type(_, ref ty) = func.sig.output
        && !is_handler_result_type(ty)
    {
        return Err(syn::Error::new_spanned(ty, "#[dead_letter_handler] function must return `HandlerResult`"));
    }

    if func.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[dead_letter_handler] must be synchronous — remove `async` (dead-letter handlers implement SyncEventHandler)",
        ));
    }

    if attrs.has_subscription_policy() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "subscription policy attributes (retries, retry_strategy, retry_base_ms, retry_max_ms, dead_letter, priority) are not supported on dead-letter handlers",
        ));
    }

    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();

    let first_param = func.sig.inputs.first().expect("validated non-empty");
    let FnArg::Typed(event_pat_ty) = first_param else {
        return Err(syn::Error::new_spanned(first_param, "first parameter must be the event (not `self`)"));
    };

    let event_ty = extract_ref_type(&event_pat_ty.ty)
        .ok_or_else(|| syn::Error::new_spanned(&event_pat_ty.ty, "event parameter must be a reference: `event: &DeadLetter`"))?;

    if !is_dead_letter_type(event_ty) {
        return Err(syn::Error::new_spanned(
            &event_pat_ty.ty,
            "#[dead_letter_handler] first parameter must be `&DeadLetter`",
        ));
    }

    // Parse optional Dep<T> parameters (position 1+).
    let dep_params = parse_dep_params(&func.sig.inputs)?;
    let has_deps = !dep_params.is_empty();

    let handler_name: Option<&str> = match &attrs.name {
        Some(n) if n.is_empty() => None,
        Some(n) => Some(n.as_str()),
        None => Some(fn_name_str.as_str()),
    };

    let handler_ident = handler_struct_ident(fn_name);
    // Preserve the original function's visibility so the const and struct match it.
    let vis = &func.vis;

    // Rename the inner function to avoid a name clash between the emitted
    // function and the same-name const we generate below.  The inner function
    // is forced to private visibility because it is an implementation detail;
    // the public surface is the const and the struct.
    let inner_fn_ident = format_ident!("__jaeb_{}", fn_name);
    let mut inner_func = func.clone();
    inner_func.sig.ident = inner_fn_ident.clone();
    inner_func.vis = syn::Visibility::Inherited;

    if has_deps {
        let resolved_ident = resolved_struct_ident(fn_name);
        let resolved_struct = gen_resolved_struct(&resolved_ident, &dep_params);
        let sync_impl = gen_sync_handler_impl_resolved(&resolved_ident, event_ty, &inner_fn_ident, &dep_params, handler_name);
        let descriptor_impl = gen_dead_letter_descriptor_impl_with_deps(&handler_ident, &resolved_ident, &dep_params);

        Ok(quote! {
            #[allow(dead_code)]
            #inner_func

            #resolved_struct

            #sync_impl

            #[doc(hidden)]
            #vis struct #handler_ident;

            #descriptor_impl

            #[allow(non_upper_case_globals)]
            #vis const #fn_name: #handler_ident = #handler_ident;
        })
    } else {
        let sync_impl = gen_sync_handler_impl(&handler_ident, event_ty, &inner_fn_ident, handler_name);
        let descriptor_impl = gen_dead_letter_descriptor_impl(&handler_ident);

        Ok(quote! {
            #[allow(dead_code)]
            #inner_func

            #[doc(hidden)]
            #vis struct #handler_ident;

            #sync_impl

            #descriptor_impl

            #[allow(non_upper_case_globals)]
            #vis const #fn_name: #handler_ident = #handler_ident;
        })
    }
}

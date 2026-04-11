//! Proc macros for `jaeb` handler generation and registration.

mod attrs;
mod codegen;
mod validate;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, FnArg, ItemFn, Path, ReturnType, Token, parse_macro_input};

use attrs::HandlerAttrs;
use codegen::{gen_async_handler_impl, gen_inventory_submit, gen_register_impl, gen_registrar_impl, gen_sync_handler_impl, handler_struct_ident};
use validate::{extract_ref_type, is_dead_letter_type, is_handler_result_type};

/// Marks a free function as a JAEB handler and generates a companion handler struct.
///
/// The generated struct is named `<FunctionNamePascalCase>Handler` and provides:
/// - `impl EventHandler<E>` for async functions
/// - `impl SyncEventHandler<E>` for sync functions
/// - `async fn register(&EventBus)` using the selected subscription strategy
///
/// # Async
///
/// ```rust,ignore
/// #[handler]
/// async fn on_order(event: &OrderPlaced) -> HandlerResult {
///     Ok(())
/// }
///
/// // Generates `OnOrderHandler`
/// OnOrderHandler::register(&bus).await?;
/// ```
///
/// # Sync
///
/// ```rust,ignore
/// #[handler]
/// fn audit(event: &OrderPlaced) -> HandlerResult {
///     Ok(())
/// }
///
/// // Generates `AuditHandler`
/// AuditHandler::register(&bus).await?;
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
/// // register() uses subscribe_with_policy with the generated policy.
/// FlakyHandler::register(&bus).await?;
/// ```
///
/// # Dead letter listener
///
/// When the event type is `DeadLetter`, `register()` uses `subscribe_dead_letters`.
///
/// ```rust,ignore
/// #[handler]
/// fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as HandlerAttrs);
    let func = parse_macro_input!(item as ItemFn);

    match expand_handler(attrs, func) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Registers multiple `#[handler]` functions in one call.
///
/// Usage:
///
/// ```rust,ignore
/// register_handlers!(bus, on_order, audit_log, on_dead_letter)?;
/// register_handlers!(bus)?; // auto-discover all #[handler] functions
/// ```
///
/// Expands to sequential `Handler::register(&bus).await?` calls and returns
/// `Result<(), EventBusError>`.
#[proc_macro]
pub fn register_handlers(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as RegisterHandlersInput);
    expand_register_handlers(input).into()
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

    if func.sig.inputs.len() > 1 {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "#[handler] only supports one parameter: `event: &EventType`",
        ));
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

    let is_dead_letter = is_dead_letter_type(event_ty);

    if is_dead_letter && is_async {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "DeadLetter listeners must be synchronous — remove `async` (subscribe_dead_letters requires SyncEventHandler)",
        ));
    }

    if is_dead_letter && attrs.has_subscription_policy() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "subscription policy attributes (retries, retry_strategy, retry_base_ms, retry_max_ms, dead_letter, priority) are not supported on DeadLetter listeners",
        ));
    }

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
    let handler_impl = if is_async {
        gen_async_handler_impl(&handler_ident, event_ty, fn_name, handler_name)
    } else {
        gen_sync_handler_impl(&handler_ident, event_ty, fn_name, handler_name)
    };
    let register_impl = gen_register_impl(&handler_ident, event_ty, &attrs, is_dead_letter, is_async);
    let registrar_impl = gen_registrar_impl(&handler_ident);
    let inventory_submit = gen_inventory_submit(&handler_ident);

    Ok(quote! {
        #[allow(dead_code)]
        #func

        pub struct #handler_ident;

        #handler_impl

        #register_impl

        #registrar_impl

        #inventory_submit
    })
}

struct RegisterHandlersInput {
    bus_expr: Expr,
    handlers: Punctuated<Path, Token![,]>,
}

impl Parse for RegisterHandlersInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let bus_expr: Expr = input.parse()?;

        if input.is_empty() {
            return Ok(Self {
                bus_expr,
                handlers: Punctuated::new(),
            });
        }

        input.parse::<Token![,]>()?;
        let handlers = Punctuated::<Path, Token![,]>::parse_terminated(input)?;

        Ok(Self { bus_expr, handlers })
    }
}

fn expand_register_handlers(input: RegisterHandlersInput) -> TokenStream2 {
    let RegisterHandlersInput { bus_expr, handlers } = input;

    if handlers.is_empty() {
        return quote! {
            {
                let __jaeb_bus_ref = &(#bus_expr);
                ::jaeb::macros_support::register_all(__jaeb_bus_ref).await?;
                ::core::result::Result::<(), ::jaeb::EventBusError>::Ok(())
            }
        };
    }

    let register_calls = handlers.iter().map(|fn_path| {
        let mut struct_path = fn_path.clone();
        if let Some(last_seg) = struct_path.segments.last_mut() {
            let handler_ident = handler_struct_ident(&last_seg.ident);
            last_seg.ident = handler_ident;
        }

        quote! {
            let _ = #struct_path::register(__jaeb_bus_ref).await?;
        }
    });

    quote! {
        {
            let __jaeb_bus_ref = &(#bus_expr);
            #(#register_calls)*
            ::core::result::Result::<(), ::jaeb::EventBusError>::Ok(())
        }
    }
}

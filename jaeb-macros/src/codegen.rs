//! Code generation for handler structs, trait impls, and descriptor impls.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attrs::HandlerAttrs;
use crate::validate::DepParam;

// ── Naming helpers ────────────────────────────────────────────────────────────

pub(crate) fn handler_struct_ident(fn_name: &Ident) -> Ident {
    format_ident!("{}Handler", to_pascal_case(&fn_name.to_string()))
}

/// Generate the private `__<PascalCase>Resolved` struct ident used when a
/// handler has `Dep<T>` parameters.
pub(crate) fn resolved_struct_ident(fn_name: &Ident) -> Ident {
    format_ident!("__{}Resolved", to_pascal_case(&fn_name.to_string()))
}

// ── Handler trait impls (no deps — handler struct IS the handler) ─────────────

pub(crate) fn gen_async_handler_impl(
    handler_ident: &Ident,
    event_ty: &Type,
    fn_name: &Ident,
    handler_name: Option<&str>,
    has_bus: bool,
) -> TokenStream2 {
    let name_method = gen_name_method(handler_name);
    let bus_param = if has_bus {
        quote! { bus }
    } else {
        quote! { _bus }
    };
    let bus_call_arg = if has_bus {
        quote! { , bus }
    } else {
        quote! {}
    };

    quote! {
        impl ::jaeb::EventHandler<#event_ty> for #handler_ident {
            async fn handle(&self, event: &#event_ty, #bus_param: &::jaeb::EventBus) -> ::jaeb::HandlerResult {
                #fn_name(event #bus_call_arg).await
            }

            #name_method
        }
    }
}

pub(crate) fn gen_sync_handler_impl(
    handler_ident: &Ident,
    event_ty: &Type,
    fn_name: &Ident,
    handler_name: Option<&str>,
    has_bus: bool,
) -> TokenStream2 {
    let name_method = gen_name_method(handler_name);
    let bus_param = if has_bus {
        quote! { bus }
    } else {
        quote! { _bus }
    };
    let bus_call_arg = if has_bus {
        quote! { , bus }
    } else {
        quote! {}
    };

    quote! {
        impl ::jaeb::SyncEventHandler<#event_ty> for #handler_ident {
            fn handle(&self, event: &#event_ty, #bus_param: &::jaeb::EventBus) -> ::jaeb::HandlerResult {
                #fn_name(event #bus_call_arg)
            }

            #name_method
        }
    }
}

// ── Resolved struct + handler trait impls (with deps) ────────────────────────

/// Generate the private `__FooResolved` struct that holds resolved `Dep<T>` field
/// values and implements the handler trait.  Used when the handler function has
/// one or more `Dep<T>` parameters.
pub(crate) fn gen_resolved_struct(resolved_ident: &Ident, dep_params: &[DepParam]) -> TokenStream2 {
    let fields: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let name = &dp.name;
            let ty = &dp.inner_ty;
            quote! { #name: #ty }
        })
        .collect();

    // Emit a static assertion that each dep type implements Clone.  This
    // produces a diagnostic pointing at the generated code rather than at
    // the impl — not perfect, but better than a confusing "method `clone`
    // not found" error from inside the handler impl.
    let clone_assertions: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let ty = &dp.inner_ty;
            quote! {
                const _: fn() = || {
                    fn _assert_clone<T: ::core::clone::Clone>() {}
                    _assert_clone::<#ty>();
                };
            }
        })
        .collect();

    quote! {
        #[doc(hidden)]
        struct #resolved_ident {
            #(#fields,)*
        }

        #(#clone_assertions)*
    }
}

/// `EventHandler<E>` impl on the resolved struct — passes cloned dep values as
/// `::jaeb::Dep(...)` to the original inner function.
pub(crate) fn gen_async_handler_impl_resolved(
    resolved_ident: &Ident,
    event_ty: &Type,
    inner_fn_ident: &Ident,
    dep_params: &[DepParam],
    handler_name: Option<&str>,
    has_bus: bool,
) -> TokenStream2 {
    let call_args: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let name = &dp.name;
            quote! { ::jaeb::Dep(self.#name.clone()) }
        })
        .collect();
    let name_method = gen_name_method(handler_name);
    let bus_param = if has_bus {
        quote! { bus }
    } else {
        quote! { _bus }
    };
    let bus_call_arg = if has_bus {
        quote! { , bus }
    } else {
        quote! {}
    };

    quote! {
        impl ::jaeb::EventHandler<#event_ty> for #resolved_ident {
            async fn handle(&self, event: &#event_ty, #bus_param: &::jaeb::EventBus) -> ::jaeb::HandlerResult {
                #inner_fn_ident(event, #(#call_args),* #bus_call_arg).await
            }

            #name_method
        }
    }
}

/// `SyncEventHandler<E>` impl on the resolved struct.
pub(crate) fn gen_sync_handler_impl_resolved(
    resolved_ident: &Ident,
    event_ty: &Type,
    inner_fn_ident: &Ident,
    dep_params: &[DepParam],
    handler_name: Option<&str>,
    has_bus: bool,
) -> TokenStream2 {
    let call_args: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let name = &dp.name;
            quote! { ::jaeb::Dep(self.#name.clone()) }
        })
        .collect();
    let name_method = gen_name_method(handler_name);
    let bus_param = if has_bus {
        quote! { bus }
    } else {
        quote! { _bus }
    };
    let bus_call_arg = if has_bus {
        quote! { , bus }
    } else {
        quote! {}
    };

    quote! {
        impl ::jaeb::SyncEventHandler<#event_ty> for #resolved_ident {
            fn handle(&self, event: &#event_ty, #bus_param: &::jaeb::EventBus) -> ::jaeb::HandlerResult {
                #inner_fn_ident(event, #(#call_args),* #bus_call_arg)
            }

            #name_method
        }
    }
}

// ── HandlerDescriptor / DeadLetterDescriptor impls ───────────────────────────

/// `impl HandlerDescriptor` for a handler **without** `Dep<T>` parameters.
/// The descriptor subscribes itself (the unit struct) directly.
pub(crate) fn gen_handler_descriptor_impl(handler_ident: &Ident, event_ty: &Type, attrs: &HandlerAttrs, is_async: bool) -> TokenStream2 {
    let subscribe_call = if attrs.has_subscription_policy() {
        let policy = gen_subscription_policy(attrs, is_async);
        quote! {
            bus.subscribe_with_policy::<#event_ty, _, _>(#handler_ident, #policy).await
        }
    } else {
        quote! {
            bus.subscribe::<#event_ty, _, _>(#handler_ident).await
        }
    };

    quote! {
        impl ::jaeb::HandlerDescriptor for #handler_ident {
            fn register<'a>(
                &'a self,
                bus: &'a ::jaeb::EventBus,
                _deps: &'a ::jaeb::Deps,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move { #subscribe_call })
            }
        }
    }
}

/// `impl HandlerDescriptor` for a handler **with** `Dep<T>` parameters.
/// The descriptor resolves each dependency from `Deps` at build time,
/// constructs the `__FooResolved` struct, and subscribes it.
pub(crate) fn gen_handler_descriptor_impl_with_deps(
    handler_ident: &Ident,
    resolved_ident: &Ident,
    event_ty: &Type,
    attrs: &HandlerAttrs,
    is_async: bool,
    dep_params: &[DepParam],
) -> TokenStream2 {
    let dep_resolutions: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let name = &dp.name;
            let ty = &dp.inner_ty;
            quote! {
                let #name = deps.get_required::<#ty>()?.clone();
            }
        })
        .collect();

    let field_names: Vec<&Ident> = dep_params.iter().map(|dp| &dp.name).collect();

    let subscribe_call = if attrs.has_subscription_policy() {
        let policy = gen_subscription_policy(attrs, is_async);
        quote! {
            bus.subscribe_with_policy::<#event_ty, _, _>(handler, #policy).await
        }
    } else {
        quote! {
            bus.subscribe::<#event_ty, _, _>(handler).await
        }
    };

    quote! {
        impl ::jaeb::HandlerDescriptor for #handler_ident {
            fn register<'a>(
                &'a self,
                bus: &'a ::jaeb::EventBus,
                deps: &'a ::jaeb::Deps,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    #(#dep_resolutions)*
                    let handler = #resolved_ident { #(#field_names,)* };
                    #subscribe_call
                })
            }
        }
    }
}

/// `impl DeadLetterDescriptor` for a dead-letter handler **without** deps.
pub(crate) fn gen_dead_letter_descriptor_impl(handler_ident: &Ident) -> TokenStream2 {
    quote! {
        impl ::jaeb::DeadLetterDescriptor for #handler_ident {
            fn register_dead_letter<'a>(
                &'a self,
                bus: &'a ::jaeb::EventBus,
                _deps: &'a ::jaeb::Deps,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move { bus.subscribe_dead_letters(#handler_ident).await })
            }
        }
    }
}

/// `impl DeadLetterDescriptor` for a dead-letter handler **with** `Dep<T>` parameters.
pub(crate) fn gen_dead_letter_descriptor_impl_with_deps(handler_ident: &Ident, resolved_ident: &Ident, dep_params: &[DepParam]) -> TokenStream2 {
    let dep_resolutions: Vec<TokenStream2> = dep_params
        .iter()
        .map(|dp| {
            let name = &dp.name;
            let ty = &dp.inner_ty;
            quote! {
                let #name = deps.get_required::<#ty>()?.clone();
            }
        })
        .collect();

    let field_names: Vec<&Ident> = dep_params.iter().map(|dp| &dp.name).collect();

    quote! {
        impl ::jaeb::DeadLetterDescriptor for #handler_ident {
            fn register_dead_letter<'a>(
                &'a self,
                bus: &'a ::jaeb::EventBus,
                deps: &'a ::jaeb::Deps,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    #(#dep_resolutions)*
                    let handler = #resolved_ident { #(#field_names,)* };
                    bus.subscribe_dead_letters(handler).await
                })
            }
        }
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn gen_name_method(handler_name: Option<&str>) -> TokenStream2 {
    match handler_name {
        Some(n) => quote! {
            fn name(&self) -> Option<&'static str> {
                Some(#n)
            }
        },
        None => quote! {},
    }
}

fn gen_subscription_policy(attrs: &HandlerAttrs, is_async: bool) -> TokenStream2 {
    let mut chain = if is_async {
        quote! { ::jaeb::AsyncSubscriptionPolicy::default() }
    } else {
        quote! { ::jaeb::SyncSubscriptionPolicy::default() }
    };

    if let Some(r) = attrs.retries {
        chain = quote! { #chain.with_max_retries(#r) };
    }

    if let Some(ref strategy) = attrs.retry_strategy {
        match strategy.as_str() {
            "fixed" => {
                let base_ms = attrs.retry_base_ms.expect("retry_base_ms validated by attrs parser");
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::Fixed(
                        ::core::time::Duration::from_millis(#base_ms),
                    ))
                };
            }
            "exponential" => {
                let base_ms = attrs.retry_base_ms.expect("retry_base_ms validated by attrs parser");
                let max_ms = attrs.retry_max_ms.expect("retry_max_ms validated by attrs parser");
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::Exponential {
                        base: ::core::time::Duration::from_millis(#base_ms),
                        max: ::core::time::Duration::from_millis(#max_ms),
                    })
                };
            }
            "exponential_jitter" => {
                let base_ms = attrs.retry_base_ms.expect("retry_base_ms validated by attrs parser");
                let max_ms = attrs.retry_max_ms.expect("retry_max_ms validated by attrs parser");
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::ExponentialWithJitter {
                        base: ::core::time::Duration::from_millis(#base_ms),
                        max: ::core::time::Duration::from_millis(#max_ms),
                    })
                };
            }
            _ => unreachable!("retry strategy validated by attrs parser"),
        }
    }

    if let Some(dl) = attrs.dead_letter {
        chain = quote! { #chain.with_dead_letter(#dl) };
    }

    if let Some(priority) = attrs.priority {
        chain = quote! { #chain.with_priority(#priority) };
    }

    chain
}

fn to_pascal_case(input: &str) -> String {
    let mut out = String::new();
    let mut upper = true;

    for ch in input.chars() {
        if ch == '_' {
            upper = true;
            continue;
        }

        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }

    out
}

//! Code generation for handler structs, trait impls, and registration methods.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attrs::HandlerAttrs;

pub(crate) fn handler_struct_ident(fn_name: &Ident) -> Ident {
    format_ident!("{}Handler", to_pascal_case(&fn_name.to_string()))
}

pub(crate) fn gen_async_handler_impl(handler_ident: &Ident, event_ty: &Type, fn_name: &Ident, listener_name: Option<&str>) -> TokenStream2 {
    let name_method = gen_name_method(listener_name);

    quote! {
        impl ::jaeb::EventHandler<#event_ty> for #handler_ident {
            async fn handle(&self, event: &#event_ty) -> ::jaeb::HandlerResult {
                #fn_name(event).await
            }

            #name_method
        }
    }
}

pub(crate) fn gen_sync_handler_impl(handler_ident: &Ident, event_ty: &Type, fn_name: &Ident, listener_name: Option<&str>) -> TokenStream2 {
    let name_method = gen_name_method(listener_name);

    quote! {
        impl ::jaeb::SyncEventHandler<#event_ty> for #handler_ident {
            fn handle(&self, event: &#event_ty) -> ::jaeb::HandlerResult {
                #fn_name(event)
            }

            #name_method
        }
    }
}

pub(crate) fn gen_register_impl(handler_ident: &Ident, event_ty: &Type, attrs: &HandlerAttrs, is_dead_letter: bool, is_async: bool) -> TokenStream2 {
    let register_call = if is_dead_letter {
        quote! {
            bus.subscribe_dead_letters(#handler_ident).await
        }
    } else if attrs.has_subscription_policy() {
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
        impl #handler_ident {
            pub async fn register(bus: &::jaeb::EventBus) -> ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError> {
                #register_call
            }
        }
    }
}

pub(crate) fn gen_registrar_impl(handler_ident: &Ident) -> TokenStream2 {
    quote! {
        impl ::jaeb::macros_support::HandlerRegistrar for #handler_ident {
            fn register<'a>(
                &self,
                bus: &'a ::jaeb::EventBus,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<::jaeb::Subscription, ::jaeb::EventBusError>,
                        > + Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(#handler_ident::register(bus))
            }
        }
    }
}

pub(crate) fn gen_inventory_submit(handler_ident: &Ident) -> TokenStream2 {
    quote! {
        ::jaeb::macros_support::_private::inventory::submit! {
            &#handler_ident as &dyn ::jaeb::macros_support::HandlerRegistrar
        }
    }
}

fn gen_name_method(listener_name: Option<&str>) -> TokenStream2 {
    match listener_name {
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
        quote! { ::jaeb::SubscriptionPolicy::default() }
    } else {
        quote! { ::jaeb::SyncSubscriptionPolicy::default() }
    };

    if let Some(r) = attrs.retries {
        chain = quote! { #chain.with_max_retries(#r) };
    }

    if let Some(ref strategy) = attrs.retry_strategy {
        let base_ms = attrs.retry_base_ms.unwrap_or(0);
        let max_ms = attrs.retry_max_ms.unwrap_or(0);

        match strategy.as_str() {
            "fixed" => {
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::Fixed(
                        ::core::time::Duration::from_millis(#base_ms),
                    ))
                };
            }
            "exponential" => {
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::Exponential {
                        base: ::core::time::Duration::from_millis(#base_ms),
                        max: ::core::time::Duration::from_millis(#max_ms),
                    })
                };
            }
            "exponential_jitter" => {
                chain = quote! {
                    #chain.with_retry_strategy(::jaeb::RetryStrategy::ExponentialWithJitter {
                        base: ::core::time::Duration::from_millis(#base_ms),
                        max: ::core::time::Duration::from_millis(#max_ms),
                    })
                };
            }
            _ => {}
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

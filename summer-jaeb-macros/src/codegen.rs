// SPDX-License-Identifier: MIT
//! Code generation for handler trait impls, subscribe calls, and inventory registration.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Ident, Type};

use crate::attrs::{ListenerAttrs, StateParam};

// ── Handler trait implementations ────────────────────────────────────────────

/// Generate `impl EventHandler<E> for Handler { ... }` (async).
pub(crate) fn gen_async_handler_impl(event_ty: &Type, fn_name: &Ident, state_params: &[StateParam]) -> TokenStream2 {
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
pub(crate) fn gen_sync_handler_impl(event_ty: &Type, fn_name: &Ident, state_params: &[StateParam]) -> TokenStream2 {
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

// ── Subscribe call generation ────────────────────────────────────────────────

/// Generate the subscribe call for the registrar's `register` method.
pub(crate) fn gen_subscribe_call(event_ty: &Type, is_dead_letter: bool, attrs: &ListenerAttrs, fn_name_str: &str, has_state: bool) -> TokenStream2 {
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
    } else if attrs.has_failure_policy() {
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

// ── Struct generation ────────────────────────────────────────────────────────

/// Generate structs for stateless listeners (no Component params).
/// Single `Handler` struct implements both the handler trait and `TypedListenerRegistrar`.
pub(crate) fn gen_stateless_structs(
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
pub(crate) fn gen_stateful_structs(
    state_params: &[StateParam],
    fn_name_str: &str,
    subscribe_call: &TokenStream2,
) -> (TokenStream2, TokenStream2, TokenStream2) {
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

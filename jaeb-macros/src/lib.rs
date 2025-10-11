use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, PatType, Type, parse_macro_input};

#[proc_macro_attribute]
pub fn event_listener(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    // Validate signature: exactly 1 argument
    let args = &input_fn.sig.inputs;
    if args.len() != 1 {
        return syn::Error::new_spanned(&input_fn.sig.inputs, "#[event_listener] functions must take exactly one argument (event)")
            .to_compile_error()
            .into();
    }

    let is_async = input_fn.sig.asyncness.is_some();

    // Extract arg pattern and type
    let (arg_pat, arg_ty, is_ref): (Box<syn::Pat>, Box<Type>, bool) = match args.first().unwrap() {
        FnArg::Typed(PatType { pat, ty, .. }) => {
            let is_ref = matches!(**ty, Type::Reference(_));
            (pat.clone(), ty.clone(), is_ref)
        }
        other => {
            return syn::Error::new_spanned(other, "unsupported argument kind").to_compile_error().into();
        }
    };

    // Determine event type (remove reference if present)
    let event_ty: Box<Type> = match *arg_ty {
        Type::Reference(ref tr) => tr.elem.clone(),
        ref t => Box::new(t.clone()),
    };

    // Generate a unique registrar name based on function ident
    let fn_ident = &input_fn.sig.ident;
    let registrar_ident = format_ident!("__jaeb_register_{}", fn_ident);

    // Build registration body depending on async/sync and ref-ness
    let register_body = if is_async {
        // async listener: require owned param; we will call subscribe_async
        if is_ref {
            // If user supplies &E for async, this would require Clone to move into async closure; disallow for now to keep semantics clear.
            syn::Error::new_spanned(
                &input_fn.sig.inputs,
                "async #[event_listener] must take the event by value (e.g., e: MyEvent)",
            )
            .to_compile_error()
        } else {
            quote! {
                // subscribe as async listener
                bus.subscribe_async(|#arg_pat: #event_ty| async move {
                    #fn_ident(#arg_pat).await;
                }).await;
            }
        }
    } else {
        // sync listener: must accept &E
        if !is_ref {
            return syn::Error::new_spanned(
                &input_fn.sig.inputs,
                "sync #[event_listener] must take the event by reference (e.g., e: &MyEvent)",
            )
            .to_compile_error()
            .into();
        }
        quote! {
            bus.subscribe_sync::<#event_ty, _>(|#arg_pat| {
                #fn_ident(#arg_pat);
            }).await;
        }
    };

    // Registrar function type: fn(&EventBus) -> Pin<Box<dyn Future<Output=()> + Send>>
    let expanded = quote! {
        #input_fn

        #[doc(hidden)]
        fn #registrar_ident(bus: &::jaeb::EventBus) -> ::jaeb::RegistrarFuture<'_> {
            Box::pin(async move {
                #register_body
            })
        }

        // Submit to global inventory in jaeb crate
        ::jaeb::_private::inventory::submit!(::jaeb::ListenerRegistrar { func: #registrar_ident });
    };

    expanded.into()
}

#[proc_macro]
pub fn bootstrap_listeners(input: TokenStream) -> TokenStream {
    // Expect a single expression representing the bus argument
    // e.g., bootstrap_listeners!(bus) or bootstrap_listeners!(&bus)
    let stream = proc_macro2::TokenStream::from(input);
    let expanded = quote! {
        ::jaeb::bootstrap_listeners(#stream).await
    };
    TokenStream::from(expanded)
}

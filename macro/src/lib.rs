use proc_macro2::Span;
use quote::{quote, ToTokens};
use syn::*;

use diplomat_core::ast;

mod enum_convert;
mod transparent_convert;

fn cfgs_to_stream(attrs: &[Attribute]) -> proc_macro2::TokenStream {
    attrs
        .iter()
        .fold(quote!(), |prev, attr| quote!(#prev #attr))
}

fn gen_params_at_boundary(ty: &ast::TypeName, name: &ast::Ident, mut on_expanded_params: impl FnMut(Type, Ident)) {
    // recursively calling `gen_params_at_boundary` stackoverflows when using `ast::TypeName::Function`. using the aux function as a workaround.
    fn gen_params_at_boundary_aux(ty: &ast::TypeName, name: &ast::Ident, mut on_expanded_closure_params: impl FnMut(Type, Ident)) {
        match ty {
            ast::TypeName::StrReference(
                .., ast::StringEncoding::UnvalidatedUtf8 | ast::StringEncoding::UnvalidatedUtf16 | ast::StringEncoding::Utf8)
            | ast::TypeName::PrimitiveSlice(..)
            | ast::TypeName::StrSlice(..) => {
                let data_type = match ty {
                    ast::TypeName::PrimitiveSlice(.., prim) =>
                        ast::TypeName::Primitive(*prim).to_syn().to_token_stream(),
                    ast::TypeName::StrReference(_, ast::StringEncoding::UnvalidatedUtf8 | ast::StringEncoding::Utf8) =>
                        quote! { u8 },
                    ast::TypeName::StrReference(_, ast::StringEncoding::UnvalidatedUtf16) =>
                        quote! { u16 },
                    ast::TypeName::StrSlice(ast::StringEncoding::UnvalidatedUtf8 | ast::StringEncoding::Utf8) =>
                        // TODO: this is not an ABI-stable type!
                        quote! { &[u8] },
                    ast::TypeName::StrSlice(ast::StringEncoding::UnvalidatedUtf16) =>
                        quote! { &[u16] },
                    _ => unreachable!()
                };
                on_expanded_closure_params(
                    parse2(match ty {
                        ast::TypeName::PrimitiveSlice(Some((_, ast::Mutability::Mutable)) | None, _)
                        | ast::TypeName::StrReference(None, ..) =>
                            quote! { *mut #data_type },
                        _ =>
                            quote! { *const #data_type }
                    })
                    .unwrap(),
                    Ident::new(&format!("{}_diplomat_data", name), Span::call_site())
                );

                on_expanded_closure_params(
                    parse2(quote! { usize })
                        .unwrap(),
                    Ident::new(&format!("{}_diplomat_len", name), Span::call_site())
                );
            }
            o => on_expanded_closure_params(
                o.to_syn(),
                Ident::new(name.as_str(), Span::call_site())
            )
        }
    }
    match ty {
        ast::TypeName::Function(inputs, output) => {
            let expanded_inputs = inputs.iter().enumerate().map(|(i, (input_ty, input_name))| {
                let mut exp_inputs: Vec<Type> = vec![];
                if i == 0 {
                    exp_inputs.push(parse_quote! { *mut std::ffi::c_void });
                }
                gen_params_at_boundary_aux(input_ty, input_name.as_ref().unwrap_or(name), |exp_ty, _|
                    exp_inputs.push(exp_ty)
                );
                exp_inputs
            }).flatten().collect::<Vec<_>>();
            let expanded_output = output.to_syn();
            on_expanded_params(
                parse2(quote! { extern "C" fn(#(#expanded_inputs),*) -> #expanded_output }).unwrap(),
                Ident::new(name.as_str(), Span::call_site())
            );
            on_expanded_params(
                parse2(quote! { * mut std::ffi::c_void }).unwrap(),
                Ident::new("_ctx", Span::call_site())

            );
        }
        o =>
            gen_params_at_boundary_aux(o, name, on_expanded_params)
    }
}

fn gen_params_invocation(param: &ast::Param, expanded_params: &mut Vec<Expr>) {
    match &param.ty {
        ast::TypeName::Function(inputs, output) => {
            let closure = Expr::Closure(ExprClosure {
                attrs: vec![],
                lifetimes: None,
                constness: None,
                movability: None,
                asyncness: None,
                capture: Some(Token![move](Span::call_site())),
                or1_token: Token![|](Span::call_site()),
                inputs: {
                    let mut puncs = punctuated::Punctuated::default();
                    inputs.iter().enumerate().for_each(|(index, (_, input_name))| {
                        puncs.push(Pat::Ident(PatIdent {
                            attrs: vec![],
                            by_ref: None,
                            mutability: None,
                            ident: input_name.as_ref().map(|n| Ident::new(n.as_str(), Span::call_site()))
                                .unwrap_or_else(|| Ident::new(&format!("_{}", index), Span::call_site())).into(),
                            subpat: None,
                        }));
                    });
                    puncs
                },
                or2_token: Token![|](Span::call_site()),
                output: match &**output {
                    ast::TypeName::Unit => syn::ReturnType::Default,
                    o => syn::ReturnType::Type(
                        Token![->](Span::call_site()),
                        Box::new(o.to_syn())
                    )
                },
                body: Box::new(Expr::Call(ExprCall {
                    attrs: vec![],
                    func: Box::new(Expr::Path(ExprPath {
                        attrs: vec![],
                        qself: None,
                        path: Ident::new(param.name.as_str(), Span::call_site()).into(),
                    })),
                    paren_token: token::Paren(Span::call_site()),
                    args: {
                        let mut args = punctuated::Punctuated::default();

                        // all lifted closures take an extra argument which captures the original callback
                        args.push(Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: Ident::new("_ctx", Span::call_site()).into()
                        }));

                        inputs.iter().enumerate().for_each(|(index, (input_ty, input_name))| {
                            let path = input_name.as_ref().map(|n| Ident::new(n.as_str(), Span::call_site()))
                                .unwrap_or_else(|| Ident::new(&format!("_{}", index), Span::call_site()));


                            match input_ty {
                                ast::TypeName::StrReference(
                                    .., ast::StringEncoding::UnvalidatedUtf8 | ast::StringEncoding::UnvalidatedUtf16 | ast::StringEncoding::Utf8
                                ) | ast::TypeName::PrimitiveSlice(..) | ast::TypeName::StrSlice(..) => {
                                    args.push(Expr::MethodCall(ExprMethodCall {
                                        attrs: vec![],
                                        receiver: Box::new(Expr::Path(ExprPath {
                                            attrs: vec![],
                                            qself: None,
                                            path: path.clone().into(),
                                        })),
                                        dot_token: Token![.](Span::call_site()),
                                        method: Ident::new("as_ptr", Span::call_site()),
                                        turbofish: None,
                                        paren_token: token::Paren(Span::call_site()),
                                        args: Default::default(),
                                    }));
                                    args.push(Expr::MethodCall(ExprMethodCall {
                                        attrs: vec![],
                                        receiver: Box::new(Expr::Path(ExprPath {
                                            attrs: vec![],
                                            qself: None,
                                            path: path.clone().into(),
                                        })),
                                        dot_token: Token![.](Span::call_site()),
                                        method: Ident::new("len", Span::call_site()),
                                        turbofish: None,
                                        paren_token: token::Paren(Span::call_site()),
                                        args: Default::default(),
                                    }));
                                }
                                _ => {
                                    args.push(Expr::Path(ExprPath {
                                        attrs: vec![],
                                        qself: None,
                                        path: path.into(),
                                    }));
                                }
                            }
                        });
                        args
                    }
                }))
            });
            expanded_params.push(closure);
        }
        ast::TypeName::StrReference(..)
        | ast::TypeName::PrimitiveSlice(..)
        | ast::TypeName::StrSlice(..) => {
            let data_ident =
                Ident::new(&format!("{}_diplomat_data", param.name), Span::call_site());
            let len_ident = Ident::new(&format!("{}_diplomat_len", param.name), Span::call_site());

            let tokens = if let ast::TypeName::PrimitiveSlice(lm, _) = &param.ty {
                match lm {
                    Some((_, ast::Mutability::Mutable)) => quote! {
                        if #len_ident == 0 {
                            &mut []
                        } else {
                            unsafe { core::slice::from_raw_parts_mut(#data_ident, #len_ident) }
                        }
                    },
                    Some((_, ast::Mutability::Immutable)) => quote! {
                        if #len_ident == 0 {
                            &[]
                        } else {
                            unsafe { core::slice::from_raw_parts(#data_ident, #len_ident) }
                        }
                    },
                    None => quote! {
                        if #len_ident == 0 {
                            Default::default()
                        } else {
                            unsafe { alloc::boxed::Box::from_raw(core::ptr::slice_from_raw_parts_mut(#data_ident, #len_ident)) }
                        }
                    },
                }
            } else if let ast::TypeName::StrReference(Some(_), encoding) = &param.ty {
                let encode = match encoding {
                    ast::StringEncoding::Utf8 => quote! {
                        // The FFI guarantees this, by either validating, or communicating this requirement to the user.
                        unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(#data_ident, #len_ident)) }
                    },
                    _ => quote! {
                        unsafe { core::slice::from_raw_parts(#data_ident, #len_ident) }
                    },
                };
                quote! {
                    if #len_ident == 0 {
                        Default::default()
                    } else {
                        #encode
                    }
                }
            } else if let ast::TypeName::StrReference(None, encoding) = &param.ty {
                let encode = match encoding {
                    ast::StringEncoding::Utf8 => quote! {
                        unsafe { core::str::from_boxed_utf8_unchecked(alloc::boxed::Box::from_raw(core::ptr::slice_from_raw_parts_mut(#data_ident, #len_ident))) }
                    },
                    _ => quote! {
                        unsafe { alloc::boxed::Box::from_raw(core::ptr::slice_from_raw_parts_mut(#data_ident, #len_ident)) }
                    },
                };
                quote! {
                    if #len_ident == 0 {
                        Default::default()
                    } else {
                        #encode
                    }
                }
            } else if let ast::TypeName::StrSlice(_) = &param.ty {
                quote! {
                    if #len_ident == 0 {
                        &[]
                    } else {
                        unsafe { core::slice::from_raw_parts(#data_ident, #len_ident) }
                    }
                }
            } else {
                unreachable!();
            };
            expanded_params.push(parse2(tokens).unwrap());
        }
        ast::TypeName::Result(_, _, _) => {
            let param = &param.name;
            expanded_params.push(parse2(quote!(#param.into())).unwrap());
        }
        _ => {
            expanded_params.push(Expr::Path(ExprPath {
                attrs: vec![],
                qself: None,
                path: Ident::new(param.name.as_str(), Span::call_site()).into(),
            }));
        }
    }
}

fn gen_custom_type_this_ident() -> Pat {
    Pat::Ident(PatIdent {
        attrs: vec![],
        by_ref: None,
        mutability: None,
        ident: Ident::new("this", Span::call_site()),
        subpat: None,
    })
}

fn gen_custom_type_all_params(m: &ast::Method) -> Vec<FnArg> {
    let this_ident = Pat::Ident(PatIdent {
        attrs: vec![],
        by_ref: None,
        mutability: None,
        ident: Ident::new("this", Span::call_site()),
        subpat: None,
    });

    let mut all_params = vec![];
    m.params.iter().for_each(|p| {
        gen_params_at_boundary(&p.ty, &p.name, |ty, ident| all_params.push(FnArg::Typed(PatType {
            attrs: vec![],
            pat: Box::new(Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident,
                subpat: None,
            })),
            colon_token: syn::token::Colon(Span::call_site()),
            ty: Box::new(ty),
        })));
    });

    if let Some(self_param) = &m.self_param {
        all_params.insert(
            0,
            FnArg::Typed(PatType {
                attrs: vec![],
                pat: Box::new(this_ident),
                colon_token: syn::token::Colon(Span::call_site()),
                ty: Box::new(self_param.to_typename().to_syn()),
            }),
        );
    }

    all_params
}

fn gen_custom_type_field(m: &ast::Method) -> proc_macro2::TokenStream {
    let all_params = gen_custom_type_all_params(m);

    let mut all_params_invocation = vec![];
    m.params.iter().for_each(|p| {
        gen_params_invocation(p, &mut all_params_invocation);
    });

    let lifetimes = {
        let lifetime_env = &m.lifetime_env;
        if lifetime_env.is_empty() { quote! {} } else { quote! { for<#lifetime_env> } }
    };

    let return_tokens = if let Some(return_type) = &m.return_type {
        if let ast::TypeName::Result(ok, err, true) = return_type {
            let ok = ok.to_syn();
            let err = err.to_syn();
            quote! { -> diplomat_runtime::DiplomatResult<#ok, #err> }
        } else if let ast::TypeName::Ordering = return_type {
            let return_type_syn = return_type.to_syn();
            quote! { -> #return_type_syn }
        } else if let ast::TypeName::Option(ty) = return_type {
            match ty.as_ref() {
                // pass by reference, Option becomes null
                ast::TypeName::Box(..) | ast::TypeName::Reference(..) => {
                    let return_type_syn = return_type.to_syn();
                    quote! { -> #return_type_syn }
                }
                // anything else goes through DiplomatResult
                _ => {
                    let ty = ty.to_syn();
                    quote! { -> diplomat_runtime::DiplomatResult<#ty, ()> }
                }
            }
        } else {
            let return_type_syn = return_type.to_syn();
            quote! { -> #return_type_syn }
        }
    } else {
        quote! {}
    };

    syn::parse_quote! {
        #lifetimes extern "C" fn(#(#all_params),*) #return_tokens
    }
}

fn gen_custom_type_method(strct: &ast::CustomType, m: &ast::Method) -> Item {
    let self_ident = Ident::new(strct.name().as_str(), Span::call_site());
    let method_ident = Ident::new(m.name.as_str(), Span::call_site());
    let extern_ident = Ident::new(m.full_path_name.as_str(), Span::call_site());

    let this_ident = gen_custom_type_this_ident();
    let all_params = gen_custom_type_all_params(m);

    let mut all_params_invocation = vec![];
    m.params.iter().for_each(|p| {
        gen_params_invocation(p, &mut all_params_invocation);
    });

    let lifetimes = {
        let lifetime_env = &m.lifetime_env;
        if lifetime_env.is_empty() { quote! {} } else { quote! { <#lifetime_env> } }
    };

    let method_invocation = if m.self_param.is_some() {
        quote! { #this_ident.#method_ident }
    } else {
        quote! { #self_ident::#method_ident }
    };

    let (return_tokens, maybe_into) = if let Some(return_type) = &m.return_type {
        if let ast::TypeName::Result(ok, err, true) = return_type {
            let ok = ok.to_syn();
            let err = err.to_syn();
            (
                quote! { -> diplomat_runtime::DiplomatResult<#ok, #err> },
                quote! { .into() },
            )
        } else if let ast::TypeName::Ordering = return_type {
            let return_type_syn = return_type.to_syn();
            (quote! { -> #return_type_syn }, quote! { as i8 })
        } else if let ast::TypeName::Option(ty) = return_type {
            match ty.as_ref() {
                // pass by reference, Option becomes null
                ast::TypeName::Box(..) | ast::TypeName::Reference(..) => {
                    let return_type_syn = return_type.to_syn();
                    (quote! { -> #return_type_syn }, quote! {})
                }
                // anything else goes through DiplomatResult
                _ => {
                    let ty = ty.to_syn();
                    (
                        quote! { -> diplomat_runtime::DiplomatResult<#ty, ()> },
                        quote! { .ok_or(()).into() },
                    )
                }
            }
        } else {
            let return_type_syn = return_type.to_syn();
            (quote! { -> #return_type_syn }, quote! {})
        }
    } else {
        (quote! {}, quote! {})
    };

    let writeable_flushes = m
        .params
        .iter()
        .filter(|p| p.is_writeable())
        .map(|p| {
            let p = &p.name;
            quote! { #p.flush(); }
        })
        .collect::<Vec<_>>();

    let cfg = cfgs_to_stream(&m.attrs.cfg);

    if writeable_flushes.is_empty() {
        Item::Fn(syn::parse_quote! {
            #[no_mangle]
            #cfg
            extern "C" fn #extern_ident #lifetimes(#(#all_params),*) #return_tokens {
                #method_invocation(#(#all_params_invocation),*) #maybe_into
            }
        })
    } else {
        Item::Fn(syn::parse_quote! {
            #[no_mangle]
            #cfg
            extern "C" fn #extern_ident #lifetimes(#(#all_params),*) #return_tokens {
                let ret = #method_invocation(#(#all_params_invocation),*);
                #(#writeable_flushes)*
                ret #maybe_into
            }
        })
    }
}

struct AttributeInfo {
    repr: bool,
    opaque: bool,
    is_out: bool,
}

impl AttributeInfo {
    fn extract(attrs: &mut Vec<Attribute>) -> Self {
        let mut repr = false;
        let mut opaque = false;
        let mut is_out = false;
        attrs.retain(|attr| {
            let ident = &attr.path().segments.iter().next().unwrap().ident;
            if ident == "repr" {
                repr = true;
                // don't actually extract repr attrs, just detect them
                return true;
            } else if ident == "diplomat" {
                if attr.path().segments.len() == 2 {
                    let seg = &attr.path().segments.iter().nth(1).unwrap().ident;
                    if seg == "opaque" {
                        opaque = true;
                        return false;
                    } else if seg == "out" {
                        is_out = true;
                        return false;
                    } else if seg == "rust_link"
                        || seg == "out"
                        || seg == "attr"
                        || seg == "skip_if_ast"
                        || seg == "abi_rename"
                    {
                        // diplomat-tool reads these, not diplomat::bridge.
                        // throw them away so rustc doesn't complain about unknown attributes
                        return false;
                    } else if seg == "enum_convert" || seg == "transparent_convert" {
                        // diplomat::bridge doesn't read this, but it's handled separately
                        // as an attribute
                        return true;
                    } else {
                        panic!("Only #[diplomat::opaque] and #[diplomat::rust_link] are supported")
                    }
                } else {
                    panic!("#[diplomat::foo] attrs have a single-segment path name")
                }
            }
            true
        });

        Self {
            repr,
            opaque,
            is_out,
        }
    }
}

fn gen_bridge(mut input: ItemMod, apiname_and_rs_entrypoint: Option<(Ident, Ident)>) -> ItemMod {
    let module = ast::Module::from_syn(&input, true);
    // Clean out any diplomat attributes so Rust doesn't get mad
    let _attrs = AttributeInfo::extract(&mut input.attrs);
    let (brace, mut new_contents) = input.content.unwrap();

    new_contents.push(parse2(quote! { use diplomat_runtime::*; }).unwrap());

    new_contents.iter_mut().for_each(|c| match c {
        Item::Struct(s) => {
            let info = AttributeInfo::extract(&mut s.attrs);

            // Normal opaque types don't need repr(transparent) because the inner type is
            // never referenced. #[diplomat::transparent_convert] handles adding repr(transparent)
            // on its own
            if !info.opaque {
                let copy = if !info.is_out {
                    // Nothing stops FFI from copying, so we better make sure the struct is Copy.
                    quote!(#[derive(Clone, Copy)])
                } else {
                    quote!()
                };

                let repr = if !info.repr {
                    quote!(#[repr(C)])
                } else {
                    quote!()
                };

                *s = syn::parse_quote! {
                    #repr
                    #copy
                    #s
                }
            }
        }

        Item::Enum(e) => {
            let info = AttributeInfo::extract(&mut e.attrs);
            if info.opaque {
                panic!("#[diplomat::opaque] not allowed on enums")
            }
            for v in &mut e.variants {
                let info = AttributeInfo::extract(&mut v.attrs);
                if info.opaque {
                    panic!("#[diplomat::opaque] not allowed on enum variants");
                }
            }
            *e = syn::parse_quote! {
                #[repr(C)]
                #[derive(Clone, Copy)]
                #e
            };
        }

        Item::Impl(i) => {
            for item in &mut i.items {
                if let syn::ImplItem::Fn(ref mut m) = *item {
                    let info = AttributeInfo::extract(&mut m.attrs);
                    if info.opaque {
                        panic!("#[diplomat::opaque] not allowed on methods")
                    }
                }
            }
        }
        _ => (),
    });

    for custom_type in module.declared_types.values() {
        custom_type.methods().iter().for_each(|m| {
            new_contents.push(gen_custom_type_method(custom_type, m));
        });

        let destroy_ident = Ident::new(custom_type.dtor_name().as_str(), Span::call_site());

        let type_ident = custom_type.name().to_syn();

        let (lifetime_defs, lifetimes) = if let Some(lifetime_env) = custom_type.lifetimes() {
            (
                quote! { <#lifetime_env> },
                lifetime_env.lifetimes_to_tokens(),
            )
        } else {
            (quote! {}, quote! {})
        };

        let cfg = cfgs_to_stream(&custom_type.attrs().cfg);

        // for now, body is empty since all we need to do is drop the box
        // TODO(#13): change to take a `*mut` and handle DST boxes appropriately
        new_contents.push(Item::Fn(syn::parse_quote! {
            #[no_mangle]
            #cfg
            extern "C" fn #destroy_ident #lifetime_defs(this: Box<#type_ident #lifetimes>) {}
        }));
    }

    if let Some((apiname, rs_entrypoint)) = apiname_and_rs_entrypoint {
        push_api_bridge(&module, &mut new_contents, apiname, rs_entrypoint);
    }

    ItemMod {
        attrs: input.attrs,
        vis: input.vis,
        mod_token: input.mod_token,
        ident: input.ident,
        content: Some((brace, new_contents)),
        semi: input.semi,
        unsafety: None,
    }
}

/// Mark a module to be exposed through Diplomat-generated FFI.
#[proc_macro_attribute]
pub fn bridge(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut apiname = None;
    let mut refresh_api_fn = None;
    let mut additional_includes = vec![];
    let mut get_api_fn = None;

    let args = parse_macro_input!(attr with punctuated::Punctuated::<Meta,syn::Token![,]>::parse_terminated);
    args.into_iter().for_each(|arg| {
        match arg {
            Meta::NameValue(MetaNameValue { path, value, .. }) => {
                match path.get_ident() {
                    Some(attr) => match value {
                        Expr::Path(ExprPath { path, .. }) => {
                            if let Some(value) = path.get_ident() {
                                if attr == "apiname" {
                                    apiname = Some(Ident::new(value.to_string().as_str(), Span::call_site()));
                                } else if attr == "refresh_api_fn" {
                                    refresh_api_fn = Some(Ident::new(value.to_string().as_str(), Span::call_site()));
                                } else if attr == "get_api_fn" {
                                    get_api_fn = Some(Ident::new(value.to_string().as_str(), Span::call_site()));
                                } else {
                                    panic!("This macro only accepts `apiname`, `refresh_api_fn` `get_api_fn` or `additional_includes`");
                                }
                            } else {
                                panic!("invalid macro attribute")
                            }
                        },
                        Expr::Array(ExprArray { elems, .. }) => {
                            elems.into_iter().for_each(|e| match e {
                                Expr::Lit(ExprLit { lit, .. }) =>
                                    additional_includes.push(format!("{}", lit.to_token_stream())),
                                x  => panic!("invalid additional_includes {}", x.to_token_stream()),
                            });
                        },
                        _ => panic!("invalid macro attribute value {}", value.to_token_stream()),
                    },
                    _ => panic!("This macro only accepts `apiname`, `refresh_api_fn` `get_api_fn` or `additional_includes`")
                }
            }
            _ => panic!("invalid macro attribute")
        }
    });
    let expanded = gen_bridge(parse_macro_input!(input), apiname.zip(refresh_api_fn));
    //println!("[RUST]\n{}", expanded.to_token_stream());
    proc_macro::TokenStream::from(expanded.to_token_stream())
}

/// Generate From and Into implementations for a Diplomat enum
///
/// This is invoked as `#[diplomat::enum_convert(OtherEnumName)]`
/// on a Diplomat enum. It will assume the other enum has exactly the same variants
/// and generate From and Into implementations using those. In case that enum is `#[non_exhaustive]`,
/// you may use `#[diplomat::enum_convert(OtherEnumName, needs_wildcard)]` to generate a panicky wildcard
/// branch. It is up to the library author to ensure the enums are kept in sync. You may use the `#[non_exhaustive_omitted_patterns]`
/// lint to enforce this.
#[proc_macro_attribute]
pub fn enum_convert(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // proc macros handle compile errors by using special error tokens.
    // In case of an error, we don't want the original code to go away too
    // (otherwise that will cause more errors) so we hold on to it and we tack it in
    // with no modifications below
    let input_cached: proc_macro2::TokenStream = input.clone().into();
    let expanded =
        enum_convert::gen_enum_convert(parse_macro_input!(attr), parse_macro_input!(input));

    let full = quote! {
        #expanded
        #input_cached
    };
    proc_macro::TokenStream::from(full.to_token_stream())
}

/// Generate conversions from inner types for opaque Diplomat types with a single field
///
/// This is invoked as `#[diplomat::transparent_convert]`
/// on an opaque Diplomat type. It will add `#[repr(transparent)]` and implement `pub(crate) fn transparent_convert()`
/// which allows constructing an `&Self` from a reference to the inner field.
#[proc_macro_attribute]
pub fn transparent_convert(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // proc macros handle compile errors by using special error tokens.
    // In case of an error, we don't want the original code to go away too
    // (otherwise that will cause more errors) so we hold on to it and we tack it in
    // with no modifications below
    let input_cached: proc_macro2::TokenStream = input.clone().into();
    let expanded = transparent_convert::gen_transparent_convert(parse_macro_input!(input));

    let full = quote! {
        #expanded
        #input_cached
    };
    proc_macro::TokenStream::from(full.to_token_stream())
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::process::Command;

    use quote::ToTokens;
    use syn::parse_quote;
    use tempfile::tempdir;

    use super::gen_bridge;

    fn rustfmt_code(code: &str) -> String {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("temp.rs");
        let mut file = File::create(file_path.clone()).unwrap();

        writeln!(file, "{code}").unwrap();
        drop(file);

        Command::new("rustfmt")
            .arg(file_path.to_str().unwrap())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        let mut file = File::open(file_path).unwrap();
        let mut data = String::new();
        file.read_to_string(&mut data).unwrap();
        drop(file);
        dir.close().unwrap();
        data
    }

    #[test]
    fn method_taking_str() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn from_str(s: &DiplomatStr) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn method_taking_slice() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn from_slice(s: &[f64]) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn method_taking_mutable_slice() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn fill_slice(s: &mut [f64]) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn method_taking_owned_slice() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn fill_slice(s: Box<[u16]>) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn method_taking_owned_str() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn something_with_str(s: Box<str>) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn mod_with_enum() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    enum Abc {
                        A,
                        B = 123,
                    }

                    impl Abc {
                        pub fn do_something(&self) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn mod_with_writeable_result() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn to_string(&self, to: &mut DiplomatWriteable) -> Result<(), ()> {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn mod_with_rust_result() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        pub fn bar(&self) -> Result<(), ()> {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn multilevel_borrows() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    #[diplomat::opaque]
                    struct Foo<'a>(&'a str);

                    #[diplomat::opaque]
                    struct Bar<'b, 'a: 'b>(&'b Foo<'a>);

                    struct Baz<'x, 'y> {
                        foo: &'y Foo<'x>,
                    }

                    impl<'a> Foo<'a> {
                        pub fn new(x: &'a str) -> Box<Foo<'a>> {
                            unimplemented!()
                        }

                        pub fn get_bar<'b>(&'b self) -> Box<Bar<'b, 'a>> {
                            unimplemented!()
                        }

                        pub fn get_baz<'b>(&'b self) -> Baz<'b, 'a> {
                            Bax { foo: self }
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn self_params() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    #[diplomat::opaque]
                    struct RefList<'a> {
                        data: &'a i32,
                        next: Option<Box<Self>>,
                    }

                    impl<'b> RefList<'b> {
                        pub fn extend(&mut self, other: &Self) -> Self {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn cfged_method() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    impl Foo {
                        #[cfg(feature = "foo")]
                        pub fn bar(s: u8) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));

        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    struct Foo {}

                    #[cfg(feature = "bar")]
                    impl Foo {
                        #[cfg(feature = "foo")]
                        pub fn bar(s: u8) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }

    #[test]
    fn cfgd_struct() {
        insta::assert_snapshot!(rustfmt_code(
            &gen_bridge(parse_quote! {
                mod ffi {
                    #[diplomat::opaque]
                    #[cfg(feature = "foo")]
                    struct Foo {}
                    #[cfg(feature = "foo")]
                    impl Foo {
                        pub fn bar(s: u8) {
                            unimplemented!()
                        }
                    }
                }
            }, None)
            .to_token_stream()
            .to_string()
        ));
    }
}

fn push_api_bridge(module: &ast::Module, new_contents: &mut Vec<Item>, apiname: Ident, rs_entrypoint: Ident) {
    for custom_type in module.declared_types.values() {
        let api_struct_ident = Ident::new(&format!("__{}_API__", custom_type.name().as_str()), Span::call_site());
        let fields = custom_type.methods().iter().map(|m| -> Field {
            let field_ident = Ident::new(m.name.as_str(), Span::call_site());
            let field_ty = gen_custom_type_field(m);
            syn::parse_quote! { pub #field_ident : #field_ty }
        }).chain(std::iter::once({
            // add destructor
            let destroy_ident = Ident::new(custom_type.dtor_name().as_str(), Span::call_site());
            let type_ident = custom_type.name();
            let (lifetime_defs, lifetimes) = match custom_type.lifetimes() { None => (quote! {}, quote! {}),
                Some(lifetime_env) => (quote! { for<#lifetime_env> }, lifetime_env.lifetimes_to_tokens())
            };
            syn::parse_quote! { pub #destroy_ident : #lifetime_defs extern "C" fn(this: Box<#type_ident #lifetimes>)}
        }));
        new_contents.push(syn::parse_quote! {
            #[allow(non_camel_case_types)]
            #[allow(non_snake_case)]
            #[repr(C)]
            pub struct #api_struct_ident {
                #(#fields),*
            }
        });

        let field_idents = custom_type.methods().iter().map(|m| -> FieldValue {
            let field_ident = Ident::new(m.name.as_str(), Span::call_site());
            let extern_ident = Ident::new(m.full_path_name.as_str(), Span::call_site());
            syn::parse_quote! { #field_ident : #extern_ident }
        }).chain(std::iter::once({
            // add destructor
            let destroy_ident = Ident::new(custom_type.dtor_name().as_str(), Span::call_site());
            syn::parse_quote! { #destroy_ident : #destroy_ident }
        }));
        let api_method_ident = Ident::new(&format!("__get_{}_api__", custom_type.name().as_str()), Span::call_site());
        new_contents.push(syn::parse_quote! {
            #[allow(non_snake_case)]
            fn #api_method_ident() -> #api_struct_ident {
                #api_struct_ident {
                    #(#field_idents),*
                }
            }
        });
    }

    let api_fields = module.declared_types.values().map(|custom_type| -> Field {
        let api_field_ident = Ident::new(custom_type.name().as_str(), Span::call_site());
        let api_struct_ident = Ident::new(&format!("__{}_API__", custom_type.name().as_str()), Span::call_site());
        syn::parse_quote! {
            pub #api_field_ident : #api_struct_ident 
        }
    });

    let api_field_idents = module.declared_types.values().map(|custom_type| -> FieldValue {
        let api_field_ident = Ident::new(custom_type.name().as_str(), Span::call_site());
        let api_method_ident = Ident::new(&format!("__get_{}_api__", custom_type.name().as_str()), Span::call_site());
        syn::parse_quote! { #api_field_ident : #api_method_ident() }
    });

    new_contents.push(syn::parse_quote! {
        mod __core__ {
            #[allow(non_camel_case_types)]
            #[allow(non_snake_case)]
            #[repr(C)]
            pub struct __Core_API__ {
                pub free: extern "C" fn(ptr: *mut std::ffi::c_void),
            }

            #[no_mangle]
            pub extern "C" fn free(ptr: *mut std::ffi::c_void) {
                unsafe { drop(Box::from_raw(ptr)); }
            }

            pub fn api_get_core() -> __Core_API__ {
                __Core_API__ {
                    free,
                }
            }
        }
    });

    new_contents.push(syn::parse_quote! {
        #[allow(non_camel_case_types)]
        #[allow(non_snake_case)]
        #[repr(C)]
        pub struct #apiname {
            pub core: __core__::__Core_API__,
            #(#api_fields),*
        }
    });

    new_contents.push(syn::parse_quote! {
        #[no_mangle]
        pub extern "C" fn #rs_entrypoint() -> #apiname {
            #apiname {
                core: __core__::api_get_core(),
                #(#api_field_idents),*
            }
        }
    });
}
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, ExprLit, FnArg, ItemFn, Lit, LitStr, Meta, MetaList, MetaNameValue, Token,
    Type, parse_macro_input,
};

#[proc_macro_derive(SdkProtocol, attributes(mutsuki))]
pub fn derive_sdk_protocol(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    match protocol_attrs(&input.attrs) {
        Ok(attrs) => {
            let ident = input.ident;
            let protocol_id = attrs.protocol_id;
            let version = attrs.version.unwrap_or_else(|| "1.0.0".to_string());
            quote! {
                impl ::mutsuki_runtime_sdk::SdkProtocol for #ident {
                    const PROTOCOL_ID: &'static str = #protocol_id;
                }

                impl ::mutsuki_runtime_sdk::ProtocolSpec for #ident {
                    fn version() -> &'static str {
                        #version
                    }
                }
            }
            .into()
        }
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(ResourceKind, attributes(mutsuki))]
pub fn derive_resource_kind(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    match resource_attrs(&input.attrs) {
        Ok(attrs) => {
            let ident = input.ident;
            let kind_id = attrs.kind_id;
            let schema = attrs.schema;
            let provider_id = attrs.provider_id;
            let semantic = semantic_tokens(&attrs.semantic);
            let operations = attrs.operations;
            quote! {
                impl ::mutsuki_runtime_sdk::ResourceKind for #ident {
                    const KIND_ID: &'static str = #kind_id;
                    const SEMANTIC: ::mutsuki_runtime_sdk::contracts::ResourceSemantic = #semantic;
                }

                impl ::mutsuki_runtime_sdk::ResourceKindSpec for #ident {
                    fn schema() -> &'static str {
                        #schema
                    }

                    fn provider_id() -> &'static str {
                        #provider_id
                    }

                    fn operations() -> &'static [&'static str] {
                        &[#(#operations),*]
                    }
                }
            }
            .into()
        }
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn mutsuki_runner(args: TokenStream, input: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(args as RunnerAttrs);
    let function = parse_macro_input!(input as ItemFn);
    match expand_runner(attrs, function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_runner(attrs: RunnerAttrs, function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if function.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            function.sig.fn_token,
            "mutsuki_runner requires an async fn",
        ));
    }
    if function.sig.inputs.len() != 2 {
        return Err(syn::Error::new_spanned(
            function.sig.ident.clone(),
            "mutsuki_runner expects exactly (AsyncRunnerContext, Task)",
        ));
    }
    for input in &function.sig.inputs {
        if matches!(input, FnArg::Receiver(_)) {
            return Err(syn::Error::new_spanned(
                input,
                "mutsuki_runner cannot be used on methods",
            ));
        }
    }

    let vis = function.vis.clone();
    let fn_ident = function.sig.ident.clone();
    let descriptor_ident = format_ident!("{fn_ident}_descriptor");
    let adapter_ident = format_ident!("{fn_ident}_adapter");
    let runner_id = attrs.runner_id;
    let plugin_id = attrs.plugin_id;
    let purity = purity_tokens(&attrs.purity);
    let execution_class = execution_class_tokens(&attrs.execution_class);
    let accepts = attrs.accepts;

    Ok(quote! {
        #function

        #vis fn #descriptor_ident() -> ::mutsuki_runtime_sdk::contracts::RunnerDescriptor {
            let builder = ::mutsuki_runtime_sdk::RunnerDescriptorBuilder::new(#runner_id, #plugin_id)
                .purity(#purity)
                .execution_class(#execution_class);
            #(let builder = builder.accepts::<#accepts>();)*
            builder.build()
        }

        #vis fn #adapter_ident(
            client: ::mutsuki_runtime_sdk::RuntimeClientRef,
        ) -> ::mutsuki_runtime_sdk::AsyncRunnerAdapter {
            ::mutsuki_runtime_sdk::AsyncRunnerAdapter::new(
                #descriptor_ident(),
                client,
                Box::new(|ctx, task| Box::pin(#fn_ident(ctx, task))),
            )
        }
    })
}

#[derive(Default)]
struct ProtocolAttrs {
    protocol_id: String,
    version: Option<String>,
}

fn protocol_attrs(attrs: &[Attribute]) -> syn::Result<ProtocolAttrs> {
    let mut parsed = ProtocolAttrs::default();
    for meta in mutsuki_meta(attrs)? {
        match meta {
            Meta::NameValue(name_value) if name_value.path.is_ident("protocol_id") => {
                parsed.protocol_id = string_value(&name_value)?;
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("version") => {
                parsed.version = Some(string_value(&name_value)?);
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "unsupported mutsuki attribute",
                ));
            }
        }
    }
    if parsed.protocol_id.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "missing protocol_id",
        ));
    }
    Ok(parsed)
}

struct ResourceAttrs {
    kind_id: String,
    semantic: String,
    schema: String,
    provider_id: String,
    operations: Vec<String>,
}

fn resource_attrs(attrs: &[Attribute]) -> syn::Result<ResourceAttrs> {
    let mut kind_id = None;
    let mut semantic = None;
    let mut schema = None;
    let mut provider_id = None;
    let mut operations = Vec::new();
    for meta in mutsuki_meta(attrs)? {
        match meta {
            Meta::NameValue(name_value) if name_value.path.is_ident("kind_id") => {
                kind_id = Some(string_value(&name_value)?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("semantic") => {
                semantic = Some(string_value(&name_value)?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("schema") => {
                schema = Some(string_value(&name_value)?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("provider_id") => {
                provider_id = Some(string_value(&name_value)?);
            }
            Meta::List(list) if list.path.is_ident("operations") => {
                operations = litstr_list(&list)?;
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "unsupported mutsuki attribute",
                ));
            }
        }
    }
    Ok(ResourceAttrs {
        kind_id: required(kind_id, "kind_id")?,
        semantic: required(semantic, "semantic")?,
        schema: required(schema, "schema")?,
        provider_id: required(provider_id, "provider_id")?,
        operations,
    })
}

struct RunnerAttrs {
    runner_id: String,
    plugin_id: String,
    accepts: Vec<Type>,
    purity: String,
    execution_class: String,
}

impl Parse for RunnerAttrs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let metas = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        let mut runner_id = None;
        let mut plugin_id = None;
        let mut accepts = Vec::new();
        let mut purity = None;
        let mut execution_class = None;
        for meta in metas {
            match meta {
                Meta::NameValue(name_value) if name_value.path.is_ident("runner_id") => {
                    runner_id = Some(string_value(&name_value)?);
                }
                Meta::NameValue(name_value) if name_value.path.is_ident("plugin_id") => {
                    plugin_id = Some(string_value(&name_value)?);
                }
                Meta::NameValue(name_value) if name_value.path.is_ident("purity") => {
                    purity = Some(string_value(&name_value)?);
                }
                Meta::NameValue(name_value) if name_value.path.is_ident("execution_class") => {
                    execution_class = Some(string_value(&name_value)?);
                }
                Meta::List(list) if list.path.is_ident("accepts") => {
                    accepts = type_list(&list)?;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unsupported mutsuki_runner attribute",
                    ));
                }
            }
        }
        if accepts.is_empty() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "mutsuki_runner requires accepts(...)",
            ));
        }
        Ok(Self {
            runner_id: required(runner_id, "runner_id")?,
            plugin_id: required(plugin_id, "plugin_id")?,
            accepts,
            purity: required(purity, "purity")?,
            execution_class: required(execution_class, "execution_class")?,
        })
    }
}

fn mutsuki_meta(attrs: &[Attribute]) -> syn::Result<Vec<Meta>> {
    let mut metas = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("mutsuki")) {
        metas.extend(attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?);
    }
    Ok(metas)
}

fn string_value(name_value: &MetaNameValue) -> syn::Result<String> {
    match &name_value.value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Ok(value.value()),
        value => Err(syn::Error::new_spanned(value, "expected string literal")),
    }
}

fn litstr_list(list: &MetaList) -> syn::Result<Vec<String>> {
    Ok(list
        .parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?
        .into_iter()
        .map(|value| value.value())
        .collect())
}

fn type_list(list: &MetaList) -> syn::Result<Vec<Type>> {
    Ok(list
        .parse_args_with(Punctuated::<Type, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

fn required(value: Option<String>, name: &str) -> syn::Result<String> {
    value.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), format!("missing {name}")))
}

fn semantic_tokens(value: &str) -> proc_macro2::TokenStream {
    match value {
        "frozen_value" => quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::FrozenValue),
        "versioned_snapshot" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::VersionedSnapshot)
        }
        "read_only_fact" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::ReadOnlyFact)
        }
        "cow_versioned_state" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::CowVersionedState)
        }
        "capability_resource" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::CapabilityResource)
        }
        "stream_resource" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::StreamResource)
        }
        "transaction_resource" => {
            quote!(::mutsuki_runtime_sdk::contracts::ResourceSemantic::TransactionResource)
        }
        _ => quote!(compile_error!("unsupported resource semantic")),
    }
}

fn purity_tokens(value: &str) -> proc_macro2::TokenStream {
    match value {
        "pure" => quote!(::mutsuki_runtime_sdk::contracts::RunnerPurity::Pure),
        "committer" => quote!(::mutsuki_runtime_sdk::contracts::RunnerPurity::Committer),
        "effectful" => quote!(::mutsuki_runtime_sdk::contracts::RunnerPurity::Effectful),
        _ => quote!(compile_error!("unsupported runner purity")),
    }
}

fn execution_class_tokens(value: &str) -> proc_macro2::TokenStream {
    match value {
        "control" => quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Control),
        "orchestration" => {
            quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Orchestration)
        }
        "io" => quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Io),
        "cpu" => quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Cpu),
        "blocking" => quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Blocking),
        "script" => quote!(::mutsuki_runtime_sdk::contracts::ExecutionClass::Script),
        _ => quote!(compile_error!("unsupported execution class")),
    }
}

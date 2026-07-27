use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Expr, ExprArray, ExprLit, ExprPath, ItemFn, ItemStruct, Lit, LitBool, MetaNameValue, Token,
    parse_macro_input,
};

#[proc_macro_attribute]
pub fn agent_tool(args: TokenStream, input: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(args as AgentToolAttrs);
    let function = parse_macro_input!(input as ItemFn);
    let fn_ident = function.sig.ident.clone();
    let descriptor_ident = format_ident!("{fn_ident}_agent_tool_descriptor");
    let vis = function.vis.clone();
    let name = attrs.name;
    let target = attrs.target;
    let description = attrs.description;
    let requires_approval = attrs.requires_approval;
    let side_effect = attrs.side_effect;
    let permissions = attrs.permissions;

    quote! {
        #function

        #vis fn #descriptor_ident() -> ::mutsuki_agent_protocol::AgentToolDescriptor {
            let mut descriptor = ::mutsuki_agent_protocol::AgentToolDescriptor::new(
                #name,
                #target,
                #description,
            );
            descriptor.requires_approval = #requires_approval;
            descriptor.side_effect = #side_effect;
            descriptor.permissions = vec![#(#permissions.into()),*];
            descriptor
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn agent_profile(args: TokenStream, input: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(args as AgentProfileAttrs);
    let item = parse_macro_input!(input as ItemStruct);
    let ident = item.ident.clone();
    let profile_id = attrs.profile_id;
    let default_model = attrs.default_model;

    quote! {
        #item

        impl #ident {
            pub const PROFILE_ID: &'static str = #profile_id;
            pub const DEFAULT_MODEL: &'static str = #default_model;
        }
    }
    .into()
}

struct AgentToolAttrs {
    name: String,
    target: proc_macro2::TokenStream,
    description: String,
    side_effect: proc_macro2::TokenStream,
    requires_approval: bool,
    permissions: Vec<String>,
}

impl Parse for AgentToolAttrs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let pairs = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
        let mut name = None;
        let mut target = None;
        let mut description = None;
        let mut side_effect = quote!(::mutsuki_agent_protocol::ToolSideEffect::None);
        let mut requires_approval = false;
        let mut permissions = Vec::new();

        for pair in pairs {
            let key = pair.path.get_ident().map(|ident| ident.to_string());
            match key.as_deref() {
                Some("name") => name = Some(string_expr(&pair.value)?),
                Some("target") => target = Some(tool_target_expr(&pair.value)?),
                Some("description") => description = Some(string_expr(&pair.value)?),
                Some("requires_approval") => requires_approval = bool_expr(&pair.value)?,
                Some("permissions") => permissions = string_array_expr(&pair.value)?,
                Some("side_effect") => {
                    let value = string_expr(&pair.value)?;
                    side_effect = match value.as_str() {
                        "none" => quote!(::mutsuki_agent_protocol::ToolSideEffect::None),
                        "workspace_read" => {
                            quote!(::mutsuki_agent_protocol::ToolSideEffect::WorkspaceRead)
                        }
                        "workspace_write" => {
                            quote!(::mutsuki_agent_protocol::ToolSideEffect::WorkspaceWrite)
                        }
                        "external_read" => {
                            quote!(::mutsuki_agent_protocol::ToolSideEffect::ExternalRead)
                        }
                        "external_write" => {
                            quote!(::mutsuki_agent_protocol::ToolSideEffect::ExternalWrite)
                        }
                        other => {
                            return Err(syn::Error::new_spanned(
                                pair.value,
                                format!("unknown side_effect `{other}`"),
                            ));
                        }
                    };
                }
                Some(other) => {
                    return Err(syn::Error::new_spanned(
                        pair.path,
                        format!("unknown argument `{other}`"),
                    ));
                }
                None => {
                    return Err(syn::Error::new_spanned(
                        pair.path,
                        "expected identifier argument",
                    ));
                }
            }
        }

        Ok(Self {
            name: name.ok_or_else(|| missing("name"))?,
            target: target.ok_or_else(|| missing("target"))?,
            description: description.ok_or_else(|| missing("description"))?,
            side_effect,
            requires_approval,
            permissions,
        })
    }
}

struct AgentProfileAttrs {
    profile_id: String,
    default_model: String,
}

impl Parse for AgentProfileAttrs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let pairs = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
        let mut profile_id = None;
        let mut default_model = Some("default".to_string());
        for pair in pairs {
            let key = pair.path.get_ident().map(|ident| ident.to_string());
            match key.as_deref() {
                Some("id") | Some("profile_id") => profile_id = Some(string_expr(&pair.value)?),
                Some("default_model") => default_model = Some(string_expr(&pair.value)?),
                Some(other) => {
                    return Err(syn::Error::new_spanned(
                        pair.path,
                        format!("unknown argument `{other}`"),
                    ));
                }
                None => {
                    return Err(syn::Error::new_spanned(
                        pair.path,
                        "expected identifier argument",
                    ));
                }
            }
        }
        Ok(Self {
            profile_id: profile_id.ok_or_else(|| missing("profile_id"))?,
            default_model: default_model.expect("default set"),
        })
    }
}

fn string_expr(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Ok(value.value()),
        _ => Err(syn::Error::new_spanned(expr, "expected string literal")),
    }
}

fn tool_target_expr(expr: &Expr) -> syn::Result<proc_macro2::TokenStream> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => {
            let target = value.value();
            Ok(quote!(#target))
        }
        Expr::Path(ExprPath { path, .. }) => {
            Ok(quote!(<#path as ::mutsuki_agent_sdk::SdkProtocol>::PROTOCOL_ID))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "expected string literal or SDK protocol marker path",
        )),
    }
}

fn bool_expr(expr: &Expr) -> syn::Result<bool> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(LitBool { value, .. }),
            ..
        }) => Ok(*value),
        _ => Err(syn::Error::new_spanned(expr, "expected bool literal")),
    }
}

fn string_array_expr(expr: &Expr) -> syn::Result<Vec<String>> {
    match expr {
        Expr::Array(ExprArray { elems, .. }) => elems.iter().map(string_expr).collect(),
        _ => Err(syn::Error::new_spanned(expr, "expected string array")),
    }
}

fn missing(name: &'static str) -> syn::Error {
    syn::Error::new(proc_macro2::Span::call_site(), format!("missing {name}"))
}

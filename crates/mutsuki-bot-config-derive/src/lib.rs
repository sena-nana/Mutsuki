//! `#[derive(MutsukiConfig)]` — generates `MutsukiConfigSchema` impls.

use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::{Attribute, Data, DeriveInput, Expr, Fields, Lit, Meta, parse_macro_input};

#[proc_macro_derive(MutsukiConfig, attributes(config))]
pub fn derive_mutsuki_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

struct FieldAttr {
    title: Option<String>,
    description: Option<String>,
    secret: bool,
    required: bool,
    default: Option<Expr>,
    unit: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    pattern: Option<String>,
    visible_if: Option<String>,
    enabled_if: Option<String>,
    restart: Option<String>,
    format: Option<String>,
    multiline: bool,
    group: Option<String>,
    order: i32,
    provider_id: Option<String>,
    schema_version: Option<u32>,
    value_version: Option<u32>,
}

impl Default for FieldAttr {
    fn default() -> Self {
        Self {
            title: None,
            description: None,
            secret: false,
            required: false,
            default: None,
            unit: None,
            min: None,
            max: None,
            min_length: None,
            max_length: None,
            pattern: None,
            visible_if: None,
            enabled_if: None,
            restart: None,
            format: None,
            multiline: false,
            group: None,
            order: 0,
            provider_id: None,
            schema_version: None,
            value_version: None,
        }
    }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let mut container = FieldAttr::default();
    parse_attrs(&input.attrs, &mut container)?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "MutsukiConfig only supports structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "MutsukiConfig requires named fields",
        ));
    };

    let provider_id = container
        .provider_id
        .unwrap_or_else(|| ident.to_string().to_ascii_lowercase());
    let schema_version = container.schema_version.unwrap_or(1);
    let value_version = container.value_version.unwrap_or(1);
    let title = container.title.unwrap_or_else(|| ident.to_string());

    let mut children = Vec::new();
    for (index, field) in fields.named.iter().enumerate() {
        let name = field.ident.as_ref().unwrap();
        let mut attr = FieldAttr {
            order: index as i32,
            ..FieldAttr::default()
        };
        parse_attrs(&field.attrs, &mut attr)?;
        let field_title = attr.title.clone().unwrap_or_else(|| name.to_string());
        let ty_tokens = type_to_value_type(&field.ty, &attr)?;
        let required = attr.required;
        let secret = attr.secret;
        let unit = opt_string(attr.unit);
        let format = opt_string(attr.format);
        let group = opt_string(attr.group);
        let placeholder = quote!(None);
        let help_link = quote!(None);
        let min = opt_f64(attr.min);
        let max = opt_f64(attr.max);
        let min_length = opt_usize(attr.min_length);
        let max_length = opt_usize(attr.max_length);
        let pattern = opt_string(attr.pattern);
        let order = attr.order;
        let restart = restart_tokens(attr.restart.as_deref());
        let visibility = match attr.visible_if {
            Some(expr) => {
                quote!(Some(::mutsuki_bot_config::ConfigExpr::parse_simple(#expr).expect("visible_if")))
            }
            None => quote!(None),
        };
        let enabled_if = match attr.enabled_if {
            Some(expr) => {
                quote!(Some(::mutsuki_bot_config::ConfigExpr::parse_simple(#expr).expect("enabled_if")))
            }
            None => quote!(None),
        };
        let default_value = match attr.default {
            Some(expr) => {
                let lit = expr_to_config_value(&expr)?;
                quote!(Some(#lit))
            }
            None => quote!(None),
        };
        let description = match attr.description {
            Some(text) => quote!(Some(::mutsuki_bot_config::LocalizedText::new(#text))),
            None => quote!(None),
        };

        children.push(quote! {
            ::mutsuki_bot_config::ConfigNode {
                key: ::mutsuki_bot_config::ConfigKey::new(stringify!(#name)),
                value_type: #ty_tokens,
                title: ::mutsuki_bot_config::LocalizedText::new(#field_title),
                description: #description,
                default_value: #default_value,
                constraints: ::mutsuki_bot_config::ConfigConstraints {
                    required: #required,
                    min: #min,
                    max: #max,
                    min_length: #min_length,
                    max_length: #max_length,
                    pattern: #pattern,
                    min_items: None,
                    max_items: None,
                },
                presentation: ::mutsuki_bot_config::ConfigPresentation {
                    group: #group,
                    order: #order,
                    unit: #unit,
                    placeholder: #placeholder,
                    help_link: #help_link,
                    format: #format,
                    secret: #secret,
                },
                visibility: #visibility,
                enabled_if: #enabled_if,
                mutability: ::mutsuki_bot_config::ConfigMutability::ReadWrite,
                restart_policy: #restart,
                children: Vec::new(),
            }
        });
    }

    Ok(quote! {
        impl ::mutsuki_bot_config::MutsukiConfigSchema for #ident {
            fn schema() -> ::mutsuki_bot_config::ConfigDescriptor {
                let descriptor = ::mutsuki_bot_config::ConfigDescriptor {
                    provider_id: ::mutsuki_bot_config::ConfigProviderId::new(#provider_id),
                    schema_version: #schema_version,
                    value_version: #value_version,
                    title: ::mutsuki_bot_config::LocalizedText::new(#title),
                    description: None,
                    scopes: vec![::mutsuki_bot_config::ConfigScope::PluginInstance],
                    root: ::mutsuki_bot_config::ConfigNode {
                        key: ::mutsuki_bot_config::ConfigKey::new("root"),
                        value_type: ::mutsuki_bot_config::ConfigValueType::Object,
                        title: ::mutsuki_bot_config::LocalizedText::new(#title),
                        description: None,
                        default_value: None,
                        constraints: ::mutsuki_bot_config::ConfigConstraints::default(),
                        presentation: ::mutsuki_bot_config::ConfigPresentation::default(),
                        visibility: None,
                        enabled_if: None,
                        mutability: ::mutsuki_bot_config::ConfigMutability::ReadWrite,
                        restart_policy: ::mutsuki_bot_config::RestartPolicy::None,
                        children: vec![#(#children),*],
                    },
                    groups: Vec::new(),
                };
                descriptor.validate_default_budgets().expect("schema budgets");
                descriptor
            }
        }
    })
}

fn parse_attrs(attrs: &[Attribute], out: &mut FieldAttr) -> syn::Result<()> {
    for attr in attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        match &attr.meta {
            Meta::List(list) => {
                let nested = list.parse_args_with(
                    syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
                )?;
                for meta in nested {
                    apply_meta(meta, out)?;
                }
            }
            Meta::Path(_) => {}
            Meta::NameValue(nv) => apply_meta(Meta::NameValue(nv.clone()), out)?,
        }
    }
    Ok(())
}

fn apply_meta(meta: Meta, out: &mut FieldAttr) -> syn::Result<()> {
    match meta {
        Meta::Path(path) if path.is_ident("secret") => out.secret = true,
        Meta::Path(path) if path.is_ident("required") => out.required = true,
        Meta::Path(path) if path.is_ident("multiline") => out.multiline = true,
        Meta::NameValue(nv) => {
            let key = nv
                .path
                .get_ident()
                .ok_or_else(|| syn::Error::new_spanned(&nv.path, "expected ident"))?
                .to_string();
            match key.as_str() {
                "title" => out.title = Some(lit_str(&nv.value)?),
                "description" => out.description = Some(lit_str(&nv.value)?),
                "unit" => out.unit = Some(lit_str(&nv.value)?),
                "default" => out.default = Some(nv.value.clone()),
                "min" => out.min = Some(lit_float(&nv.value)?),
                "max" => out.max = Some(lit_float(&nv.value)?),
                "min_length" => out.min_length = Some(lit_usize(&nv.value)?),
                "max_length" => out.max_length = Some(lit_usize(&nv.value)?),
                "pattern" => out.pattern = Some(lit_str(&nv.value)?),
                "visible_if" => out.visible_if = Some(lit_str(&nv.value)?),
                "enabled_if" => out.enabled_if = Some(lit_str(&nv.value)?),
                "restart" => out.restart = Some(lit_str(&nv.value)?),
                "format" => out.format = Some(lit_str(&nv.value)?),
                "group" => out.group = Some(lit_str(&nv.value)?),
                "order" => out.order = lit_usize(&nv.value)? as i32,
                "provider_id" => out.provider_id = Some(lit_str(&nv.value)?),
                "schema_version" => out.schema_version = Some(lit_usize(&nv.value)? as u32),
                "value_version" => out.value_version = Some(lit_usize(&nv.value)? as u32),
                other => {
                    return Err(syn::Error::new_spanned(
                        nv.path,
                        format!("unknown config attribute `{other}`"),
                    ));
                }
            }
        }
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "unsupported config attribute",
            ));
        }
    }
    Ok(())
}

fn lit_str(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        _ => Err(syn::Error::new_spanned(expr, "expected string literal")),
    }
}

fn lit_float(expr: &Expr) -> syn::Result<f64> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse(),
        Expr::Lit(syn::ExprLit {
            lit: Lit::Int(i), ..
        }) => Ok(i.base10_parse::<i64>()? as f64),
        _ => Err(syn::Error::new_spanned(expr, "expected numeric literal")),
    }
}

fn lit_usize(expr: &Expr) -> syn::Result<usize> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse(),
        _ => Err(syn::Error::new_spanned(expr, "expected integer literal")),
    }
}

fn opt_string(value: Option<String>) -> proc_macro2::TokenStream {
    match value {
        Some(v) => quote!(Some(#v.to_string())),
        None => quote!(None),
    }
}

fn opt_f64(value: Option<f64>) -> proc_macro2::TokenStream {
    match value {
        Some(v) => quote!(Some(#v)),
        None => quote!(None),
    }
}

fn opt_usize(value: Option<usize>) -> proc_macro2::TokenStream {
    match value {
        Some(v) => quote!(Some(#v)),
        None => quote!(None),
    }
}

fn restart_tokens(value: Option<&str>) -> proc_macro2::TokenStream {
    match value {
        Some("none") | None => quote!(::mutsuki_bot_config::RestartPolicy::None),
        Some("reconfigure") => quote!(::mutsuki_bot_config::RestartPolicy::Reconfigure),
        Some("plugin_reload") => quote!(::mutsuki_bot_config::RestartPolicy::PluginReload),
        Some("bot_restart") => quote!(::mutsuki_bot_config::RestartPolicy::BotRestart),
        Some("host_restart") => quote!(::mutsuki_bot_config::RestartPolicy::HostRestart),
        Some(other) => {
            let msg = format!("unknown restart policy `{other}`");
            quote!(compile_error!(#msg))
        }
    }
}

fn type_to_value_type(ty: &syn::Type, attr: &FieldAttr) -> syn::Result<proc_macro2::TokenStream> {
    let ty_str = ty.to_token_stream().to_string().replace(' ', "");
    if attr.secret || ty_str == "SecretValue" {
        return Ok(quote!(::mutsuki_bot_config::ConfigValueType::Secret));
    }
    if ty_str == "PathBuf" || ty_str == "std::path::PathBuf" {
        return Ok(quote!(::mutsuki_bot_config::ConfigValueType::FileRef));
    }
    Ok(match ty_str.as_str() {
        "bool" => quote!(::mutsuki_bot_config::ConfigValueType::Bool),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" => {
            quote!(::mutsuki_bot_config::ConfigValueType::Integer)
        }
        "f32" | "f64" => quote!(::mutsuki_bot_config::ConfigValueType::Float),
        "String" => {
            let multiline = attr.multiline;
            quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: #multiline })
        }
        other if other.starts_with("Vec<") => {
            let inner = &other[4..other.len() - 1];
            let item = match inner {
                "String" => {
                    quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: false })
                }
                "bool" => quote!(::mutsuki_bot_config::ConfigValueType::Bool),
                "u32" | "i32" | "u64" | "i64" | "usize" => {
                    quote!(::mutsuki_bot_config::ConfigValueType::Integer)
                }
                _ => quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: false }),
            };
            quote!(::mutsuki_bot_config::ConfigValueType::Array {
                item: ::std::boxed::Box::new(#item)
            })
        }
        other if other.starts_with("Option<") => {
            // Optional wrapper — underlying required=false.
            let inner = &other[7..other.len() - 1];
            match inner {
                "String" => {
                    let multiline = attr.multiline;
                    quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: #multiline })
                }
                "bool" => quote!(::mutsuki_bot_config::ConfigValueType::Bool),
                "u32" | "i32" | "u64" | "i64" => {
                    quote!(::mutsuki_bot_config::ConfigValueType::Integer)
                }
                "PathBuf" | "std::path::PathBuf" => {
                    quote!(::mutsuki_bot_config::ConfigValueType::FileRef)
                }
                _ => quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: false }),
            }
        }
        _ => quote!(::mutsuki_bot_config::ConfigValueType::String { multiline: false }),
    })
}

fn expr_to_config_value(expr: &Expr) -> syn::Result<proc_macro2::TokenStream> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(s), ..
        }) => {
            let v = s.value();
            Ok(quote!(::mutsuki_bot_config::ConfigValue::String(#v.to_string())))
        }
        Expr::Lit(syn::ExprLit {
            lit: Lit::Bool(b), ..
        }) => {
            let v = b.value;
            Ok(quote!(::mutsuki_bot_config::ConfigValue::Bool(#v)))
        }
        Expr::Lit(syn::ExprLit {
            lit: Lit::Int(i), ..
        }) => {
            let v: i64 = i.base10_parse()?;
            Ok(quote!(::mutsuki_bot_config::ConfigValue::Integer(#v)))
        }
        Expr::Lit(syn::ExprLit {
            lit: Lit::Float(f), ..
        }) => {
            let v: f64 = f.base10_parse()?;
            Ok(quote!(::mutsuki_bot_config::ConfigValue::Float(#v)))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            "default must be a literal bool/int/float/string",
        )),
    }
}

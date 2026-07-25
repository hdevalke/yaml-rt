//! Derive macro entry point for typed YAML round-trip overlays.
//!
//! The derive supports named-field structs and generates `FromYamlDoc` and
//! `ToYamlDoc` implementations that bind Rust fields to YAML mapping keys.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Meta, Path, parse_macro_input};

/// Derives typed YAML round-trip overlay implementations.
///
/// Supported field attributes:
///
/// - `#[yaml(rename = "yaml-key")]`
/// - `#[yaml(default)]`
/// - `#[yaml(default = expression)]`
/// - `#[yaml(comment = "Comment for inserted entries.")]`
/// - `#[yaml(alias = "legacy-key")]` for read/update fallback keys
/// - `#[yaml(skip)]` to ignore a field and fill it with `Default::default()`
/// - `#[yaml(skip_serializing_if = "path::to::predicate")]` to remove or omit
///   the field when the predicate returns `true`
/// - `#[yaml(flatten)]` to overlay a nested round-trip struct on the same root
///   mapping
/// - Rust doc comments as insertion comments when `yaml(comment = ...)` is not
///   present
///
/// Supported struct attributes:
///
/// - `#[yaml(preserve_unknown_fields)]` keeps unknown mapping entries, which is
///   the default behavior
/// - `#[yaml(prune_unknown_fields)]` removes root mapping entries that are not
///   known fields or aliases after writing typed fields
/// - `#[yaml(insert_order = "append")]` appends missing fields, which is the
///   default behavior
/// - `#[yaml(insert_order = "struct")]` inserts missing fields before the next
///   existing field in declaration order when possible
#[proc_macro_derive(YamlRoundTrip, attributes(yaml))]
pub fn derive_yaml_round_trip(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand_yaml_round_trip(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum UnknownFieldPolicy {
    #[default]
    Preserve,
    Prune,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InsertOrder {
    #[default]
    Append,
    Struct,
}

#[derive(Default)]
struct StructOptions {
    unknown_field_policy: UnknownFieldPolicy,
    insert_order: InsertOrder,
}

#[derive(Default)]
struct FieldOptions {
    rename: Option<String>,
    aliases: Vec<String>,
    default: Option<TokenStream2>,
    comment: Option<String>,
    doc_comment: Option<String>,
    skip: bool,
    skip_serializing_if: Option<Path>,
    flatten: bool,
}

struct FieldExpansion {
    reads: Vec<TokenStream2>,
    writes: Vec<TokenStream2>,
    known_keys: Vec<String>,
    has_flatten: bool,
}

fn expand_yaml_round_trip(input: DeriveInput) -> syn::Result<TokenStream2> {
    let struct_options = parse_struct_options(&input.attrs)?;
    let name = input.ident;
    let fields = named_struct_fields(input.data, &name)?;
    let fields = expand_fields(fields, struct_options.insert_order)?;
    validate_struct_options(&name, &struct_options, fields.has_flatten)?;

    let ordered_keys_binding = match struct_options.insert_order {
        InsertOrder::Append => quote! {},
        InsertOrder::Struct => {
            let known_keys = &fields.known_keys;
            quote! {
                let __yaml_rt_ordered_keys = &[#(#known_keys),*];
            }
        }
    };

    let prune_unknown_fields = match struct_options.unknown_field_policy {
        UnknownFieldPolicy::Preserve => quote! {},
        UnknownFieldPolicy::Prune => {
            let known_keys = &fields.known_keys;
            quote! {
                doc.retain_mapping_entries(root, &[#(#known_keys),*])?;
            }
        }
    };

    let field_reads = fields.reads;
    let field_writes = fields.writes;
    Ok(quote! {
        impl ::yaml_rt::FromYamlDoc for #name {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                Ok(Self {
                    #(#field_reads,)*
                })
            }
        }

        impl ::yaml_rt::ToYamlDoc for #name {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                let root = doc.root_mapping()?;
                #ordered_keys_binding
                #(#field_writes)*
                #prune_unknown_fields
                Ok(())
            }
        }
    })
}

fn named_struct_fields(
    data: Data,
    name: &syn::Ident,
) -> syn::Result<syn::punctuated::Punctuated<syn::Field, syn::token::Comma>> {
    match data {
        Data::Struct(struct_data) => match struct_data.fields {
            Fields::Named(fields) => Ok(fields.named),
            _ => Err(syn::Error::new_spanned(
                name,
                "YamlRoundTrip supports only structs with named fields",
            )),
        },
        _ => Err(syn::Error::new_spanned(
            name,
            "YamlRoundTrip can only be derived for structs",
        )),
    }
}

fn expand_fields(
    fields: syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
    insert_order: InsertOrder,
) -> syn::Result<FieldExpansion> {
    let mut field_reads = Vec::new();
    let mut field_writes = Vec::new();
    let mut known_keys = Vec::new();
    let mut has_flatten = false;

    for field in fields {
        let options = parse_field_options(&field.attrs)?;
        let Some(field_name) = field.ident else {
            return Err(syn::Error::new_spanned(
                field,
                "YamlRoundTrip supports only named fields",
            ));
        };
        let field_type = field.ty;
        if options.flatten {
            has_flatten = true;
            push_flatten_field(
                &mut field_reads,
                &mut field_writes,
                &field_name,
                &field_type,
                &options,
            )?;
            continue;
        }

        push_regular_field(
            &mut field_reads,
            &mut field_writes,
            &mut known_keys,
            &field_name,
            &field_type,
            &options,
            insert_order,
        );
    }

    Ok(FieldExpansion {
        reads: field_reads,
        writes: field_writes,
        known_keys,
        has_flatten,
    })
}

fn push_flatten_field(
    field_reads: &mut Vec<TokenStream2>,
    field_writes: &mut Vec<TokenStream2>,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    options: &FieldOptions,
) -> syn::Result<()> {
    if options.skip {
        return Err(syn::Error::new_spanned(
            field_name,
            "yaml(flatten) cannot be combined with yaml(skip)",
        ));
    }
    if options.rename.is_some()
        || !options.aliases.is_empty()
        || options.default.is_some()
        || options.comment.is_some()
        || options.skip_serializing_if.is_some()
    {
        return Err(syn::Error::new_spanned(
            field_name,
            "yaml(flatten) cannot be combined with rename, alias, default, comment, or skip_serializing_if",
        ));
    }

    field_reads.push(quote! {
        #field_name: <#field_type as ::yaml_rt::FromYamlDoc>::from_yaml_doc(doc)?
    });
    field_writes.push(quote! {
        ::yaml_rt::ToYamlDoc::apply_to_yaml_doc(&self.#field_name, doc)?;
    });
    Ok(())
}

fn push_regular_field(
    field_reads: &mut Vec<TokenStream2>,
    field_writes: &mut Vec<TokenStream2>,
    known_keys: &mut Vec<String>,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    options: &FieldOptions,
    insert_order: InsertOrder,
) {
    let yaml_key = options
        .rename
        .clone()
        .unwrap_or_else(|| field_name.to_string());
    known_keys.push(yaml_key.clone());
    let aliases = options.aliases.clone();
    known_keys.extend(aliases.iter().cloned());
    let insert_comment = if let Some(comment) = options
        .comment
        .clone()
        .or_else(|| options.doc_comment.clone())
    {
        quote! { Some(#comment) }
    } else {
        quote! { None }
    };
    let skip_serializing_if = options.skip_serializing_if.clone();

    if options.skip {
        field_reads.push(quote! {
            #field_name: ::core::default::Default::default()
        });
        return;
    }

    let read_value = if let Some(default) = options.default.clone() {
        quote! {
            match node {
                Some(node) => <#field_type as ::yaml_rt::YamlValue>::read_yaml(doc, node)?,
                None => { #default }
            }
        }
    } else {
        quote! {
            <#field_type as ::yaml_rt::YamlValue>::read_yaml_field(doc, node, #yaml_key)?
        }
    };
    field_reads.push(quote! {
        #field_name: {
            let mut node = doc.get_path(&[#yaml_key])?;
            #(
                if node.is_none() {
                    node = doc.get_path(&[#aliases])?;
                }
            )*
            #read_value
        }
    });

    let write_field = write_field_tokens(
        field_name,
        field_type,
        &yaml_key,
        &aliases,
        insert_order,
        &insert_comment,
    );
    push_field_write(
        field_writes,
        field_name,
        &yaml_key,
        &aliases,
        skip_serializing_if,
        write_field,
    );
}

fn write_field_tokens(
    field_name: &syn::Ident,
    field_type: &syn::Type,
    yaml_key: &str,
    aliases: &[String],
    insert_order: InsertOrder,
    insert_comment: &TokenStream2,
) -> TokenStream2 {
    let insert_missing_field = match insert_order {
        InsertOrder::Append => quote! {
            doc.insert_mapping_value_with_comment(
                root,
                #yaml_key,
                &self.#field_name,
                ::yaml_rt::MappingEntryStyle::default(),
                #insert_comment,
            )?;
        },
        InsertOrder::Struct => quote! {
            doc.insert_mapping_value_ordered_with_comment(
                root,
                #yaml_key,
                &self.#field_name,
                ::yaml_rt::MappingEntryStyle::default(),
                #insert_comment,
                __yaml_rt_ordered_keys,
            )?;
        },
    };

    quote! {
        let mut node = doc.get_path(&[#yaml_key])?;
        #(
            if node.is_none() {
                node = doc.get_path(&[#aliases])?;
            }
        )*
        if let Some(node) = node {
            <#field_type as ::yaml_rt::YamlValue>::write_yaml(&self.#field_name, doc, Some(node))?;
        } else {
            #insert_missing_field
        }
    }
}

fn push_field_write(
    field_writes: &mut Vec<TokenStream2>,
    field_name: &syn::Ident,
    yaml_key: &str,
    aliases: &[String],
    skip_serializing_if: Option<Path>,
    write_field: TokenStream2,
) {
    if let Some(predicate) = skip_serializing_if {
        field_writes.push(quote! {
            if #predicate(&self.#field_name) {
                if doc.get_mapping_entry(root, #yaml_key)?.is_some() {
                    doc.remove_mapping_entry(root, #yaml_key)?;
                } else {
                    #(
                        if doc.get_mapping_entry(root, #aliases)?.is_some() {
                            doc.remove_mapping_entry(root, #aliases)?;
                        }
                    )*
                }
            } else {
                #write_field
            }
        });
    } else {
        field_writes.push(write_field);
    }
}

fn validate_struct_options(
    name: &syn::Ident,
    struct_options: &StructOptions,
    has_flatten: bool,
) -> syn::Result<()> {
    if struct_options.insert_order == InsertOrder::Struct && has_flatten {
        return Err(syn::Error::new_spanned(
            name.clone(),
            "yaml(insert_order = \"struct\") cannot be combined with yaml(flatten)",
        ));
    }

    if struct_options.unknown_field_policy == UnknownFieldPolicy::Prune && has_flatten {
        return Err(syn::Error::new_spanned(
            name.clone(),
            "yaml(prune_unknown_fields) cannot be combined with yaml(flatten)",
        ));
    }

    Ok(())
}

fn parse_struct_options(attrs: &[Attribute]) -> syn::Result<StructOptions> {
    let mut options = StructOptions::default();

    for attr in attrs {
        if attr.path().is_ident("yaml") {
            parse_struct_yaml_attr(attr, &mut options)?;
        }
    }

    Ok(options)
}

fn parse_struct_yaml_attr(attr: &Attribute, options: &mut StructOptions) -> syn::Result<()> {
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("preserve_unknown_fields") {
            options.unknown_field_policy = UnknownFieldPolicy::Preserve;
            Ok(())
        } else if meta.path.is_ident("prune_unknown_fields") {
            options.unknown_field_policy = UnknownFieldPolicy::Prune;
            Ok(())
        } else if meta.path.is_ident("insert_order") {
            let value = meta.value()?;
            let insert_order = value.parse::<LitStr>()?;
            match insert_order.value().as_str() {
                "append" => options.insert_order = InsertOrder::Append,
                "struct" => options.insert_order = InsertOrder::Struct,
                _ => {
                    return Err(meta.error("insert_order must be `append` or `struct`"));
                }
            }
            Ok(())
        } else {
            Err(meta.error("unsupported yaml struct attribute"))
        }
    })
}

fn parse_field_options(attrs: &[Attribute]) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();

    for attr in attrs {
        if attr.path().is_ident("doc") {
            collect_doc_comment(attr, &mut options);
        } else if attr.path().is_ident("yaml") {
            parse_yaml_attr(attr, &mut options)?;
        }
    }

    Ok(options)
}

fn collect_doc_comment(attr: &Attribute, options: &mut FieldOptions) {
    let Meta::NameValue(meta) = &attr.meta else {
        return;
    };
    let syn::Expr::Lit(expr) = &meta.value else {
        return;
    };
    let syn::Lit::Str(text) = &expr.lit else {
        return;
    };

    let text = text.value().trim().to_owned();
    if text.is_empty() {
        return;
    }

    if let Some(existing) = &mut options.doc_comment {
        existing.push('\n');
        existing.push_str(&text);
    } else {
        options.doc_comment = Some(text);
    }
}

fn parse_yaml_attr(attr: &Attribute, options: &mut FieldOptions) -> syn::Result<()> {
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("rename") {
            let value = meta.value()?;
            options.rename = Some(value.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("comment") {
            let value = meta.value()?;
            options.comment = Some(value.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("alias") {
            let value = meta.value()?;
            options.aliases.push(value.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("default") {
            if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let default = value.parse::<syn::Expr>()?;
                options.default = Some(quote! { #default });
            } else {
                options.default = Some(quote! { ::core::default::Default::default() });
            }
            Ok(())
        } else if meta.path.is_ident("skip") {
            options.skip = true;
            Ok(())
        } else if meta.path.is_ident("skip_serializing_if") {
            let value = meta.value()?;
            let predicate = value.parse::<LitStr>()?.parse::<Path>()?;
            options.skip_serializing_if = Some(predicate);
            Ok(())
        } else if meta.path.is_ident("flatten") {
            options.flatten = true;
            Ok(())
        } else {
            Err(meta.error("unsupported yaml attribute"))
        }
    })
}

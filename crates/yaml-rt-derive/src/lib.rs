//! Derive macro entry point for typed YAML round-trip overlays.
//!
//! The derive supports named-field structs and generates `FromYamlDoc` and
//! `ToYamlDoc` implementations that bind Rust fields to YAML mapping keys.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Fields, LitStr, Meta, Path, WherePredicate, parse_macro_input,
};

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
/// - `#[yaml(with = "module::path")]` to convert through the module's `Repr`
///   type and `from_yaml`/`to_yaml` functions
/// - Rust doc comments as insertion comments when `yaml(comment = ...)` is not
///   present
///
/// Adapter modules use this contract:
///
/// ```text
/// type Repr;
///
/// fn from_yaml(value: Repr) -> Result<FieldType, yaml_rt::YamlError>;
/// fn to_yaml(value: &FieldType) -> Result<Repr, yaml_rt::YamlError>;
/// ```
///
/// `Repr` must implement `YamlValue` and `ToYamlFragment`. The usual field
/// attributes are applied outside the conversion. `with` cannot be combined
/// with `skip` or `flatten`.
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
    with: Option<Path>,
}

#[derive(Default)]
struct FieldExpansion {
    reads: Vec<TokenStream2>,
    writes: Vec<TokenStream2>,
    known_keys: Vec<String>,
    has_flatten: bool,
    read_bounds: Vec<WherePredicate>,
    write_bounds: Vec<WherePredicate>,
}

fn expand_yaml_round_trip(input: DeriveInput) -> syn::Result<TokenStream2> {
    let DeriveInput {
        attrs,
        ident: name,
        generics,
        data,
        ..
    } = input;
    match data {
        Data::Struct(struct_data) => match struct_data.fields {
            Fields::Named(fields) => expand_named_struct(attrs, name, generics, fields.named),
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields
                    .unnamed
                    .into_iter()
                    .next()
                    .expect("one unnamed field was checked");
                expand_newtype_struct(attrs, name, generics, field)
            }
            Fields::Unnamed(fields) => Err(syn::Error::new_spanned(
                fields,
                "YamlRoundTrip supports tuple structs only when they contain exactly one field",
            )),
            Fields::Unit => Err(syn::Error::new_spanned(
                name,
                "YamlRoundTrip does not support unit structs",
            )),
        },
        Data::Enum(data) => Err(syn::Error::new_spanned(
            data.enum_token,
            "YamlRoundTrip enum support is not available yet",
        )),
        Data::Union(data) => Err(syn::Error::new_spanned(
            data.union_token,
            "YamlRoundTrip cannot be derived for unions",
        )),
    }
}

fn expand_named_struct(
    attrs: Vec<Attribute>,
    name: syn::Ident,
    generics: syn::Generics,
    fields: syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let struct_options = parse_struct_options(&attrs)?;
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
    let mut read_generics = generics.clone();
    read_generics
        .make_where_clause()
        .predicates
        .extend(fields.read_bounds.iter().cloned());
    let mut write_generics = generics.clone();
    write_generics
        .make_where_clause()
        .predicates
        .extend(fields.write_bounds.iter().cloned());
    let mut combined_generics = generics;
    combined_generics
        .make_where_clause()
        .predicates
        .extend(fields.read_bounds);
    combined_generics
        .make_where_clause()
        .predicates
        .extend(fields.write_bounds);
    let (read_impl_generics, read_type_generics, read_where_clause) =
        read_generics.split_for_impl();
    let (write_impl_generics, write_type_generics, write_where_clause) =
        write_generics.split_for_impl();
    let (combined_impl_generics, combined_type_generics, combined_where_clause) =
        combined_generics.split_for_impl();

    Ok(quote! {
        impl #read_impl_generics ::yaml_rt::FromYamlDoc for #name #read_type_generics #read_where_clause {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                Ok(Self {
                    #(#field_reads,)*
                })
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlDoc for #name #write_type_generics #write_where_clause {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                let root = doc.root_mapping()?;
                #ordered_keys_binding
                #(#field_writes)*
                #prune_unknown_fields
                Ok(())
            }
        }

        impl #combined_impl_generics ::yaml_rt::YamlValue for #name #combined_type_generics #combined_where_clause {
            fn read_yaml(
                doc: &::yaml_rt::YamlDoc,
                node: ::yaml_rt::NodeId,
            ) -> Result<Self, ::yaml_rt::YamlError> {
                ::yaml_rt::__read_mapping_overlay(doc, node)
            }

            fn write_yaml(
                &self,
                doc: &mut ::yaml_rt::YamlDoc,
                node: Option<::yaml_rt::NodeId>,
            ) -> Result<::yaml_rt::NodeId, ::yaml_rt::YamlError> {
                ::yaml_rt::__write_mapping_overlay(self, doc, node)
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlFragment for #name #write_type_generics #write_where_clause {
            fn to_yaml_fragment(
                &self,
                indent: usize,
                line_ending: &str,
            ) -> Result<String, ::yaml_rt::YamlError> {
                ::yaml_rt::__mapping_overlay_to_yaml_fragment(self, indent, line_ending)
            }
        }
    })
}

fn expand_newtype_struct(
    attrs: Vec<Attribute>,
    name: syn::Ident,
    generics: syn::Generics,
    field: syn::Field,
) -> syn::Result<TokenStream2> {
    if let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("yaml")) {
        return Err(syn::Error::new_spanned(
            attr,
            "transparent newtype structs do not support container-level yaml attributes",
        ));
    }
    let options = parse_field_options(&field.attrs)?;
    if options.rename.is_some()
        || !options.aliases.is_empty()
        || options.default.is_some()
        || options.comment.is_some()
        || options.skip
        || options.skip_serializing_if.is_some()
        || options.flatten
    {
        return Err(syn::Error::new_spanned(
            field,
            "transparent newtype fields support only yaml(with)",
        ));
    }
    let field_type = field.ty;
    let (read_value, read_document_value, write_value, fragment_value, read_bound, write_bound) =
        if let Some(with) = options.with {
            (
                quote! {
                    {
                        let __yaml_rt_repr =
                            <#with::Repr as ::yaml_rt::YamlValue>::read_yaml(doc, node)?;
                        #with::from_yaml(__yaml_rt_repr)?
                    }
                },
                quote! {
                    {
                        let __yaml_rt_repr =
                            ::yaml_rt::__read_yaml_document::<#with::Repr>(doc)?;
                        #with::from_yaml(__yaml_rt_repr)?
                    }
                },
                quote! {
                    let __yaml_rt_repr = #with::to_yaml(&self.0)?;
                    <#with::Repr as ::yaml_rt::YamlValue>::write_yaml(
                        &__yaml_rt_repr,
                        doc,
                        node,
                    )
                },
                quote! {
                    let __yaml_rt_repr = #with::to_yaml(&self.0)?;
                    <#with::Repr as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                        &__yaml_rt_repr,
                        indent,
                        line_ending,
                    )
                },
                syn::parse_quote! {
                    #with::Repr: ::yaml_rt::YamlValue
                },
                syn::parse_quote! {
                    #with::Repr: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
                },
            )
        } else {
            (
                quote! {
                    <#field_type as ::yaml_rt::YamlValue>::read_yaml(doc, node)?
                },
                quote! {
                    ::yaml_rt::__read_yaml_document::<#field_type>(doc)?
                },
                quote! {
                    <#field_type as ::yaml_rt::YamlValue>::write_yaml(&self.0, doc, node)
                },
                quote! {
                    <#field_type as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                        &self.0,
                        indent,
                        line_ending,
                    )
                },
                syn::parse_quote! {
                    #field_type: ::yaml_rt::YamlValue
                },
                syn::parse_quote! {
                    #field_type: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
                },
            )
        };

    let mut read_generics = generics.clone();
    read_generics
        .make_where_clause()
        .predicates
        .push(read_bound);
    let mut write_generics = generics;
    write_generics
        .make_where_clause()
        .predicates
        .push(write_bound);
    let (read_impl_generics, read_type_generics, read_where_clause) =
        read_generics.split_for_impl();
    let (write_impl_generics, write_type_generics, write_where_clause) =
        write_generics.split_for_impl();

    Ok(quote! {
        impl #read_impl_generics ::yaml_rt::FromYamlDoc for #name #read_type_generics #read_where_clause {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                Ok(Self(#read_document_value))
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlDoc for #name #write_type_generics #write_where_clause {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                ::yaml_rt::__write_yaml_document(self, doc)
            }
        }

        impl #write_impl_generics ::yaml_rt::YamlValue for #name #write_type_generics #write_where_clause {
            fn read_yaml(
                doc: &::yaml_rt::YamlDoc,
                node: ::yaml_rt::NodeId,
            ) -> Result<Self, ::yaml_rt::YamlError> {
                Ok(Self(#read_value))
            }

            fn write_yaml(
                &self,
                doc: &mut ::yaml_rt::YamlDoc,
                node: Option<::yaml_rt::NodeId>,
            ) -> Result<::yaml_rt::NodeId, ::yaml_rt::YamlError> {
                #write_value
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlFragment for #name #write_type_generics #write_where_clause {
            fn to_yaml_fragment(
                &self,
                indent: usize,
                line_ending: &str,
            ) -> Result<String, ::yaml_rt::YamlError> {
                #fragment_value
            }
        }
    })
}

fn expand_fields(
    fields: syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
    insert_order: InsertOrder,
) -> syn::Result<FieldExpansion> {
    let mut expansion = FieldExpansion::default();

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
            expansion.has_flatten = true;
            push_flatten_field(&mut expansion, &field_name, &field_type, &options)?;
            continue;
        }

        push_regular_field(
            &mut expansion,
            &field_name,
            &field_type,
            &options,
            insert_order,
        )?;
    }

    Ok(expansion)
}

fn push_flatten_field(
    expansion: &mut FieldExpansion,
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
    if options.with.is_some() {
        return Err(syn::Error::new_spanned(
            field_name,
            "yaml(with) cannot be combined with yaml(flatten)",
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

    expansion.read_bounds.push(syn::parse_quote! {
        #field_type: ::yaml_rt::FromYamlDoc
    });
    expansion.write_bounds.push(syn::parse_quote! {
        #field_type: ::yaml_rt::ToYamlDoc
    });
    expansion.reads.push(quote! {
        #field_name: <#field_type as ::yaml_rt::FromYamlDoc>::from_yaml_doc(doc)?
    });
    expansion.writes.push(quote! {
        ::yaml_rt::ToYamlDoc::apply_to_yaml_doc(&self.#field_name, doc)?;
    });
    Ok(())
}

fn push_regular_field(
    expansion: &mut FieldExpansion,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    options: &FieldOptions,
    insert_order: InsertOrder,
) -> syn::Result<()> {
    let yaml_key = options
        .rename
        .clone()
        .unwrap_or_else(|| field_name.to_string());
    expansion.known_keys.push(yaml_key.clone());
    let aliases = options.aliases.clone();
    expansion.known_keys.extend(aliases.iter().cloned());
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
        if options.with.is_some() {
            return Err(syn::Error::new_spanned(
                field_name,
                "yaml(with) cannot be combined with yaml(skip)",
            ));
        }
        expansion.read_bounds.push(syn::parse_quote! {
            #field_type: ::core::default::Default
        });
        expansion.reads.push(quote! {
            #field_name: ::core::default::Default::default()
        });
        return Ok(());
    }

    let (read_present_value, read_required_field, write_type, write_value_binding, write_value_ref) =
        if let Some(with) = &options.with {
            expansion.read_bounds.push(syn::parse_quote! {
                #with::Repr: ::yaml_rt::YamlValue
            });
            expansion.write_bounds.push(syn::parse_quote! {
                #with::Repr: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
            });
            (
                quote! {
                    {
                        let __yaml_rt_repr =
                            <#with::Repr as ::yaml_rt::YamlValue>::read_yaml(doc, node)?;
                        #with::from_yaml(__yaml_rt_repr)?
                    }
                },
                quote! {
                    {
                        let __yaml_rt_repr =
                            <#with::Repr as ::yaml_rt::YamlValue>::read_yaml_field(
                                doc,
                                node,
                                #yaml_key,
                            )?;
                        #with::from_yaml(__yaml_rt_repr)?
                    }
                },
                quote! { #with::Repr },
                quote! {
                    let __yaml_rt_repr = #with::to_yaml(&self.#field_name)?;
                },
                quote! { &__yaml_rt_repr },
            )
        } else {
            expansion.read_bounds.push(syn::parse_quote! {
                #field_type: ::yaml_rt::YamlValue
            });
            expansion.write_bounds.push(syn::parse_quote! {
                #field_type: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
            });
            (
                quote! {
                    <#field_type as ::yaml_rt::YamlValue>::read_yaml(doc, node)?
                },
                quote! {
                    <#field_type as ::yaml_rt::YamlValue>::read_yaml_field(
                        doc,
                        node,
                        #yaml_key,
                    )?
                },
                quote! { #field_type },
                quote! {},
                quote! { &self.#field_name },
            )
        };
    let read_value = if let Some(default) = options.default.clone() {
        quote! {
            match node {
                Some(node) => #read_present_value,
                None => { #default }
            }
        }
    } else {
        read_required_field
    };
    expansion.reads.push(quote! {
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
        &write_type,
        &write_value_ref,
        &yaml_key,
        &aliases,
        insert_order,
        &insert_comment,
    );
    let write_field = quote! {
        #write_value_binding
        #write_field
    };
    push_field_write(
        &mut expansion.writes,
        field_name,
        &yaml_key,
        &aliases,
        skip_serializing_if,
        write_field,
    );
    Ok(())
}

fn write_field_tokens(
    field_type: &TokenStream2,
    field_value: &TokenStream2,
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
                #field_value,
                ::yaml_rt::MappingEntryStyle::default(),
                #insert_comment,
            )?;
        },
        InsertOrder::Struct => quote! {
            doc.insert_mapping_value_ordered_with_comment(
                root,
                #yaml_key,
                #field_value,
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
            <#field_type as ::yaml_rt::YamlValue>::write_yaml(#field_value, doc, Some(node))?;
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
        } else if meta.path.is_ident("with") {
            let value = meta.value()?;
            options.with = Some(value.parse::<LitStr>()?.parse::<Path>()?);
            Ok(())
        } else {
            Err(meta.error("unsupported yaml attribute"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_and_skip_report_a_targeted_error() {
        let input: DeriveInput = syn::parse_quote! {
            struct Invalid {
                #[yaml(with = "adapter", skip)]
                value: String,
            }
        };

        let error = expand_yaml_round_trip(input).expect_err("with plus skip must fail");

        assert!(
            error
                .to_string()
                .contains("yaml(with) cannot be combined with yaml(skip)")
        );
    }

    #[test]
    fn with_and_flatten_report_a_targeted_error() {
        let input: DeriveInput = syn::parse_quote! {
            struct Invalid {
                #[yaml(with = "adapter", flatten)]
                value: String,
            }
        };

        let error = expand_yaml_round_trip(input).expect_err("with plus flatten must fail");

        assert!(
            error
                .to_string()
                .contains("yaml(with) cannot be combined with yaml(flatten)")
        );
    }
}

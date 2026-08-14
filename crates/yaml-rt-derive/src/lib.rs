//! Derive macro entry point for typed YAML round-trip overlays.
//!
//! The derive supports named mapping structs, transparent single-field tuple
//! structs, and every enum variant shape. It generates document- and
//! node-level overlay implementations that retain the lossless YAML document
//! as the source of truth.

use std::collections::BTreeMap;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Fields, LitStr, Meta, Path, WherePredicate, parse_macro_input,
};

/// Derives typed YAML round-trip overlay implementations.
///
/// Named structs overlay YAML mappings. A tuple struct with exactly one field
/// is transparent and delegates directly to that field. Unit and multi-field
/// tuple structs are rejected.
///
/// Enum unit variants are represented as scalar strings. Newtype, tuple, and
/// struct variants use local YAML tags:
///
/// ```text
/// Unit
/// !Newtype 42
/// !Tuple [1, true]
/// !Struct {host: api}
/// ```
///
/// Enum-level `rename_all` accepts `lowercase`, `snake_case`, `kebab-case`,
/// `SCREAMING_SNAKE_CASE`, `camelCase`, and `PascalCase`. Variants accept
/// `rename` and repeated `alias` attributes. Unnamed newtype and tuple payload
/// fields accept `with`.
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
///   mapping or capture unmodeled entries in a string-keyed `BTreeMap` or
///   `HashMap`
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
/// A recursively flattened mapping graph may contain one catch-all map. Its
/// entries exclude canonical field names, aliases, skipped fields, and keys
/// modeled by nested flattened structs. Applying the overlay synchronizes the
/// catch-all entries exactly; inserting a modeled key into the map is an error.
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
///
/// Same-variant enum writes patch payloads incrementally. Variant switches
/// replace the enum node while retaining its anchor and surrounding entry
/// comment. Enum data variants intentionally support only the local-tag
/// representation; internally tagged, adjacently tagged, externally mapped,
/// and untagged representations are outside this derive's current contract.
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

#[derive(Debug, Clone, Copy)]
enum RenameRule {
    Lowercase,
    SnakeCase,
    KebabCase,
    ScreamingSnakeCase,
    CamelCase,
    PascalCase,
}

#[derive(Default)]
struct EnumOptions {
    rename_all: Option<RenameRule>,
}

#[derive(Default)]
struct VariantOptions {
    rename: Option<String>,
    aliases: Vec<String>,
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
    flatten_types: Vec<syn::Type>,
    flatten_values: Vec<TokenStream2>,
    has_flatten: bool,
    read_bounds: Vec<WherePredicate>,
    write_bounds: Vec<WherePredicate>,
}

#[derive(Clone, Copy)]
enum FieldAccess {
    SelfFields,
    Bindings,
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
            Fields::Named(fields) => expand_named_struct(&attrs, &name, generics, fields.named),
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields
                    .unnamed
                    .into_iter()
                    .next()
                    .expect("one unnamed field was checked");
                expand_newtype_struct(&attrs, &name, generics, field)
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
        Data::Enum(data) => expand_enum(&attrs, &name, generics, data.variants),
        Data::Union(data) => Err(syn::Error::new_spanned(
            data.union_token,
            "YamlRoundTrip cannot be derived for unions",
        )),
    }
}

fn expand_named_struct(
    attrs: &[Attribute],
    name: &syn::Ident,
    generics: syn::Generics,
    fields: syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let struct_options = parse_struct_options(attrs)?;
    let fields = expand_fields(fields, struct_options.insert_order, FieldAccess::SelfFields)?;
    validate_struct_options(name, &struct_options, fields.has_flatten)?;

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
            quote! {
                doc.retain_mapping_entries(root, &__yaml_rt_claimed_keys)?;
            }
        }
    };

    let field_reads = &fields.reads;
    let field_writes = &fields.writes;
    let catch_all_count = flatten_catch_all_count(&fields);
    let direct_claimed_keys = flatten_claimed_keys_binding(&fields, quote! { &[] });
    let outer_claimed_keys =
        flatten_claimed_keys_binding(&fields, quote! { __yaml_rt_outer_claimed_keys });
    let flatten_validations = flatten_validation_tokens(&fields);
    let flatten_key_metadata = flatten_key_metadata(&fields);
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
        .extend(fields.read_bounds.iter().cloned());
    combined_generics
        .make_where_clause()
        .predicates
        .extend(fields.write_bounds.iter().cloned());
    let (read_impl_generics, read_type_generics, read_where_clause) =
        read_generics.split_for_impl();
    let (write_impl_generics, write_type_generics, write_where_clause) =
        write_generics.split_for_impl();
    let (combined_impl_generics, combined_type_generics, combined_where_clause) =
        combined_generics.split_for_impl();

    Ok(quote! {
        impl #read_impl_generics ::yaml_rt::FromYamlDoc for #name #read_type_generics #read_where_clause {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                #direct_claimed_keys
                Ok(Self {
                    #(#field_reads,)*
                })
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlDoc for #name #write_type_generics #write_where_clause {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                #direct_claimed_keys
                #(#flatten_validations)*
                let root = doc.root_mapping()?;
                #ordered_keys_binding
                #(#field_writes)*
                #prune_unknown_fields
                Ok(())
            }
        }

        impl #combined_impl_generics ::yaml_rt::YamlFlatten for #name #combined_type_generics #combined_where_clause {
            fn yaml_flatten_keys() -> ::std::vec::Vec<&'static str> {
                #flatten_key_metadata
            }

            fn yaml_flatten_catch_all_count() -> usize {
                #catch_all_count
            }

            fn from_yaml_flattened(
                doc: &::yaml_rt::YamlDoc,
                __yaml_rt_outer_claimed_keys: &[&str],
            ) -> Result<Self, ::yaml_rt::YamlError> {
                ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                #outer_claimed_keys
                Ok(Self {
                    #(#field_reads,)*
                })
            }

            fn validate_yaml_flattened(
                &self,
                doc: &::yaml_rt::YamlDoc,
                __yaml_rt_outer_claimed_keys: &[&str],
            ) -> Result<(), ::yaml_rt::YamlError> {
                ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                #outer_claimed_keys
                #(#flatten_validations)*
                Ok(())
            }

            fn apply_to_yaml_flattened(
                &self,
                doc: &mut ::yaml_rt::YamlDoc,
                __yaml_rt_outer_claimed_keys: &[&str],
            ) -> Result<(), ::yaml_rt::YamlError> {
                <Self as ::yaml_rt::YamlFlatten>::validate_yaml_flattened(
                    self,
                    doc,
                    __yaml_rt_outer_claimed_keys,
                )?;
                #outer_claimed_keys
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

fn flatten_catch_all_count(fields: &FieldExpansion) -> TokenStream2 {
    let counts = fields.flatten_types.iter().map(|field_type| {
        quote! { <#field_type as ::yaml_rt::YamlFlatten>::yaml_flatten_catch_all_count() }
    });
    quote! { 0usize #(+ #counts)* }
}

fn flatten_claimed_keys_binding(
    fields: &FieldExpansion,
    outer_claimed_keys: TokenStream2,
) -> TokenStream2 {
    let known_keys = &fields.known_keys;
    let nested_keys = fields.flatten_types.iter().map(|field_type| {
        quote! {
            for __yaml_rt_key in
                <#field_type as ::yaml_rt::YamlFlatten>::yaml_flatten_keys()
            {
                if !__yaml_rt_claimed_keys.contains(&__yaml_rt_key) {
                    __yaml_rt_claimed_keys.push(__yaml_rt_key);
                }
            }
        }
    });
    quote! {
        let mut __yaml_rt_claimed_keys: ::std::vec::Vec<&str> =
            (#outer_claimed_keys).to_vec();
        #(
            if !__yaml_rt_claimed_keys.contains(&#known_keys) {
                __yaml_rt_claimed_keys.push(#known_keys);
            }
        )*
        #(#nested_keys)*
    }
}

fn flatten_key_metadata(fields: &FieldExpansion) -> TokenStream2 {
    let known_keys = &fields.known_keys;
    let nested_keys = fields.flatten_types.iter().map(|field_type| {
        quote! {
            for __yaml_rt_key in
                <#field_type as ::yaml_rt::YamlFlatten>::yaml_flatten_keys()
            {
                if !__yaml_rt_keys.contains(&__yaml_rt_key) {
                    __yaml_rt_keys.push(__yaml_rt_key);
                }
            }
        }
    });
    quote! {
        let mut __yaml_rt_keys = ::std::vec::Vec::new();
        #(
            if !__yaml_rt_keys.contains(&#known_keys) {
                __yaml_rt_keys.push(#known_keys);
            }
        )*
        #(#nested_keys)*
        __yaml_rt_keys
    }
}

fn flatten_validation_tokens(fields: &FieldExpansion) -> Vec<TokenStream2> {
    fields
        .flatten_types
        .iter()
        .zip(&fields.flatten_values)
        .map(|(field_type, field_value)| {
            quote! {
                <#field_type as ::yaml_rt::YamlFlatten>::validate_yaml_flattened(
                    #field_value,
                    doc,
                    &__yaml_rt_claimed_keys,
                )?;
            }
        })
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "the generated trait implementations share validated newtype metadata"
)]
fn expand_newtype_struct(
    attrs: &[Attribute],
    name: &syn::Ident,
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

struct UnitVariant {
    ident: syn::Ident,
    canonical: String,
    aliases: Vec<String>,
}

fn expand_enum(
    attrs: &[Attribute],
    name: &syn::Ident,
    generics: syn::Generics,
    variants: syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    if variants
        .iter()
        .all(|variant| matches!(variant.fields, Fields::Unit))
    {
        expand_unit_enum(attrs, name, &generics, variants)
    } else {
        expand_tagged_enum(attrs, name, generics, variants)
    }
}

fn expand_unit_enum(
    attrs: &[Attribute],
    name: &syn::Ident,
    generics: &syn::Generics,
    variants: syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let expanded = parse_unit_variants(attrs, variants)?;
    Ok(render_unit_enum(name, generics, &expanded))
}

fn parse_unit_variants(
    attrs: &[Attribute],
    variants: syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> syn::Result<Vec<UnitVariant>> {
    let options = parse_enum_options(attrs)?;
    let mut expanded = Vec::new();
    let mut names = BTreeMap::<String, String>::new();
    for variant in variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "tagged enum payload variants are not available yet",
            ));
        }
        let variant_options = parse_variant_options(&variant.attrs)?;
        let rust_name = variant.ident.to_string();
        let canonical = variant_options
            .rename
            .unwrap_or_else(|| apply_rename_rule(&rust_name, options.rename_all));
        register_enum_variant_names(
            &variant.ident,
            &canonical,
            &variant_options.aliases,
            &mut names,
        )?;
        expanded.push(UnitVariant {
            ident: variant.ident,
            canonical,
            aliases: variant_options.aliases,
        });
    }
    Ok(expanded)
}

fn render_unit_enum(
    name: &syn::Ident,
    generics: &syn::Generics,
    expanded: &[UnitVariant],
) -> TokenStream2 {
    let expected = expanded
        .iter()
        .map(|variant| variant.canonical.as_str())
        .collect::<Vec<_>>();
    let read_arms = expanded.iter().map(unit_variant_read_arm);
    let write_arms = expanded.iter().map(unit_variant_write_arm);
    let fragment_arms = expanded.iter().map(unit_variant_fragment_arm);
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics ::yaml_rt::FromYamlDoc for #name #type_generics #where_clause {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                ::yaml_rt::__read_yaml_document(doc)
            }
        }

        impl #impl_generics ::yaml_rt::ToYamlDoc for #name #type_generics #where_clause {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                ::yaml_rt::__write_yaml_document(self, doc)
            }
        }

        impl #impl_generics ::yaml_rt::YamlValue for #name #type_generics #where_clause {
            fn read_yaml(
                doc: &::yaml_rt::YamlDoc,
                node: ::yaml_rt::NodeId,
            ) -> Result<Self, ::yaml_rt::YamlError> {
                if let Some(tag) = doc.raw_tag(node).filter(|tag| {
                    tag.starts_with('!') && !tag.starts_with("!!")
                }) {
                    return Err(::yaml_rt::__typed_node_error(
                        doc,
                        node,
                        format!("unknown YAML enum tag `{tag}`"),
                        &[#(#expected),*],
                    ));
                }
                let value = <String as ::yaml_rt::YamlValue>::read_yaml(doc, node)?;
                match value.as_str() {
                    #(#read_arms,)*
                    _ => Err(::yaml_rt::__typed_node_error(
                        doc,
                        node,
                        format!("unknown YAML enum variant `{value}`"),
                        &[#(#expected),*],
                    )),
                }
            }

            fn write_yaml(
                &self,
                doc: &mut ::yaml_rt::YamlDoc,
                node: Option<::yaml_rt::NodeId>,
            ) -> Result<::yaml_rt::NodeId, ::yaml_rt::YamlError> {
                let node = node.ok_or_else(|| {
                    ::yaml_rt::YamlError::new(::yaml_rt::Diagnostic::new(
                        ::yaml_rt::DiagnosticKind::Typed,
                        "cannot insert a standalone YAML enum without collection context",
                        ::yaml_rt::Span::empty(0),
                    ))
                })?;
                match self {
                    #(#write_arms,)*
                }
            }
        }

        impl #impl_generics ::yaml_rt::ToYamlFragment for #name #type_generics #where_clause {
            fn to_yaml_fragment(
                &self,
                indent: usize,
                line_ending: &str,
            ) -> Result<String, ::yaml_rt::YamlError> {
                match self {
                    #(#fragment_arms,)*
                }
            }
        }
    }
}

fn unit_variant_read_arm(variant: &UnitVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let names = unit_variant_names(variant);
    quote! {
        #(#names)|* => Ok(Self::#ident)
    }
}

fn unit_variant_write_arm(variant: &UnitVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let canonical = &variant.canonical;
    let names = unit_variant_names(variant);
    quote! {
        Self::#ident => {
            let __yaml_rt_has_local_tag = doc.raw_tag(node).is_some_and(|tag| {
                tag.starts_with('!') && !tag.starts_with("!!")
            });
            if !__yaml_rt_has_local_tag
                && <String as ::yaml_rt::YamlValue>::read_yaml(doc, node)
                    .is_ok_and(|value| matches!(value.as_str(), #(#names)|*))
            {
                return Ok(node);
            }
            <String as ::yaml_rt::YamlValue>::write_yaml(
                &#canonical.to_owned(),
                doc,
                Some(node),
            )
        }
    }
}

fn unit_variant_fragment_arm(variant: &UnitVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let canonical = &variant.canonical;
    quote! {
        Self::#ident => <String as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
            &#canonical.to_owned(),
            indent,
            line_ending,
        )
    }
}

fn unit_variant_names(variant: &UnitVariant) -> Vec<&String> {
    std::iter::once(&variant.canonical)
        .chain(variant.aliases.iter())
        .collect()
}

struct PayloadField {
    ty: syn::Type,
    with: Option<Path>,
}

enum EnumVariantKind {
    Unit,
    Newtype(PayloadField),
    Tuple(Vec<PayloadField>),
    Struct {
        fields: Vec<syn::Ident>,
        expansion: FieldExpansion,
    },
}

struct EnumVariant {
    ident: syn::Ident,
    canonical: String,
    aliases: Vec<String>,
    kind: EnumVariantKind,
}

struct TaggedEnumExpansion {
    variants: Vec<EnumVariant>,
    read_bounds: Vec<syn::WherePredicate>,
    write_bounds: Vec<syn::WherePredicate>,
}

fn expand_tagged_enum(
    attrs: &[Attribute],
    name: &syn::Ident,
    generics: syn::Generics,
    variants: syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let expansion = parse_tagged_enum_variants(attrs, variants)?;
    Ok(render_tagged_enum(name, generics, expansion))
}

fn parse_tagged_enum_variants(
    attrs: &[Attribute],
    variants: syn::punctuated::Punctuated<syn::Variant, syn::token::Comma>,
) -> syn::Result<TaggedEnumExpansion> {
    let options = parse_enum_options(attrs)?;
    let mut expanded = Vec::new();
    let mut names = BTreeMap::<String, String>::new();
    let mut read_bounds = Vec::new();
    let mut write_bounds = Vec::new();

    for variant in variants {
        let variant_options = parse_variant_options(&variant.attrs)?;
        let rust_name = variant.ident.to_string();
        let canonical = variant_options
            .rename
            .unwrap_or_else(|| apply_rename_rule(&rust_name, options.rename_all));
        register_enum_variant_names(
            &variant.ident,
            &canonical,
            &variant_options.aliases,
            &mut names,
        )?;

        let kind = match variant.fields {
            Fields::Unit => EnumVariantKind::Unit,
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                validate_enum_tag_names(&variant.ident, &canonical, &variant_options.aliases)?;
                let field = parse_payload_field(
                    fields
                        .unnamed
                        .into_iter()
                        .next()
                        .expect("one unnamed field was checked"),
                    &mut read_bounds,
                    &mut write_bounds,
                )?;
                EnumVariantKind::Newtype(field)
            }
            Fields::Unnamed(fields) => {
                validate_enum_tag_names(&variant.ident, &canonical, &variant_options.aliases)?;
                let fields = fields
                    .unnamed
                    .into_iter()
                    .map(|field| parse_payload_field(field, &mut read_bounds, &mut write_bounds))
                    .collect::<syn::Result<Vec<_>>>()?;
                EnumVariantKind::Tuple(fields)
            }
            Fields::Named(fields) => {
                validate_enum_tag_names(&variant.ident, &canonical, &variant_options.aliases)?;
                let field_names = fields
                    .named
                    .iter()
                    .filter_map(|field| field.ident.clone())
                    .collect::<Vec<_>>();
                let expansion =
                    expand_fields(fields.named, InsertOrder::Append, FieldAccess::Bindings)?;
                validate_struct_options(
                    &variant.ident,
                    &StructOptions::default(),
                    expansion.has_flatten,
                )?;
                read_bounds.extend(expansion.read_bounds.iter().cloned());
                write_bounds.extend(expansion.write_bounds.iter().cloned());
                EnumVariantKind::Struct {
                    fields: field_names,
                    expansion,
                }
            }
        };
        expanded.push(EnumVariant {
            ident: variant.ident,
            canonical,
            aliases: variant_options.aliases,
            kind,
        });
    }

    Ok(TaggedEnumExpansion {
        variants: expanded,
        read_bounds,
        write_bounds,
    })
}

fn tagged_enum_read_body(expanded: &[EnumVariant]) -> TokenStream2 {
    let expected_units = expanded
        .iter()
        .filter(|variant| matches!(variant.kind, EnumVariantKind::Unit))
        .map(|variant| variant.canonical.as_str())
        .collect::<Vec<_>>();
    let expected_tags = expanded
        .iter()
        .filter(|variant| !matches!(variant.kind, EnumVariantKind::Unit))
        .map(|variant| variant.canonical.as_str())
        .collect::<Vec<_>>();
    let unit_read_arms = expanded
        .iter()
        .filter(|variant| matches!(variant.kind, EnumVariantKind::Unit))
        .map(|variant| {
            let ident = &variant.ident;
            let accepted = accepted_variant_names(variant);
            quote! { #(#accepted)|* => Ok(Self::#ident) }
        })
        .collect::<Vec<_>>();
    let tagged_read_arms = expanded
        .iter()
        .filter(|variant| !matches!(variant.kind, EnumVariantKind::Unit))
        .map(tagged_variant_read_arm)
        .collect::<Vec<_>>();
    let untagged_scalar_read = if unit_read_arms.is_empty() {
        quote! {
            Err(::yaml_rt::__typed_node_error(
                doc,
                node,
                "enum data variant requires a local YAML tag",
                &[#(#expected_tags),*],
            ))
        }
    } else {
        quote! {
            let value = <String as ::yaml_rt::YamlValue>::read_yaml(doc, node)?;
            match value.as_str() {
                #(#unit_read_arms,)*
                _ => Err(::yaml_rt::__typed_node_error(
                    doc,
                    node,
                    format!("unknown YAML enum variant `{value}`"),
                    &[#(#expected_units),*],
                )),
            }
        }
    };

    quote! {
        let __yaml_rt_raw_tag = doc.raw_tag(node);
        let __yaml_rt_local_tag = __yaml_rt_raw_tag.and_then(|tag| {
            let suffix = tag.strip_prefix('!')?;
            (!suffix.is_empty()
                && !suffix.starts_with('!')
                && !suffix.starts_with('<'))
                .then_some(suffix)
        });
        if let Some(tag) = __yaml_rt_local_tag {
            match tag {
                #(#tagged_read_arms,)*
                _ => Err(::yaml_rt::__typed_node_error(
                    doc,
                    node,
                    format!("unknown YAML enum tag `!{tag}`"),
                    &[#(#expected_tags),*],
                )),
            }
        } else if __yaml_rt_raw_tag.is_some_and(|tag| {
            tag.starts_with('!') && !tag.starts_with("!!")
        }) {
            Err(::yaml_rt::__typed_node_error(
                doc,
                node,
                format!(
                    "unknown YAML enum tag `{}`",
                    __yaml_rt_raw_tag.unwrap_or_default()
                ),
                &[#(#expected_tags),*],
            ))
        } else if matches!(
            doc.semantic_kind(node),
            Some(::yaml_rt::SemanticKind::Scalar { .. })
        ) {
            #untagged_scalar_read
        } else {
            Err(::yaml_rt::__typed_node_error(
                doc,
                node,
                "enum data variant requires a local YAML tag",
                &[#(#expected_tags),*],
            ))
        }
    }
}

fn render_tagged_enum(
    name: &syn::Ident,
    generics: syn::Generics,
    expansion: TaggedEnumExpansion,
) -> TokenStream2 {
    let TaggedEnumExpansion {
        variants,
        read_bounds,
        write_bounds,
    } = expansion;
    let read_body = tagged_enum_read_body(&variants);
    let write_arms = variants.iter().map(enum_variant_write_arm);
    let fragment_arms = variants.iter().map(enum_variant_fragment_arm);
    let (read_generics, write_generics, combined_generics) =
        enum_generics_with_bounds(generics, read_bounds, write_bounds);
    let (read_impl_generics, read_type_generics, read_where_clause) =
        read_generics.split_for_impl();
    let (write_impl_generics, write_type_generics, write_where_clause) =
        write_generics.split_for_impl();
    let (combined_impl_generics, combined_type_generics, combined_where_clause) =
        combined_generics.split_for_impl();

    quote! {
        impl #read_impl_generics ::yaml_rt::FromYamlDoc for #name #read_type_generics #read_where_clause {
            fn from_yaml_doc(doc: &::yaml_rt::YamlDoc) -> Result<Self, ::yaml_rt::YamlError> {
                let node = doc.document_root(0)?.ok_or_else(|| {
                    ::yaml_rt::YamlError::new(::yaml_rt::Diagnostic::new(
                        ::yaml_rt::DiagnosticKind::Typed,
                        "document does not contain a YAML enum value",
                        ::yaml_rt::Span::empty(0),
                    ))
                })?;
                #read_body
            }
        }

        impl #combined_impl_generics ::yaml_rt::ToYamlDoc for #name #combined_type_generics #combined_where_clause {
            fn apply_to_yaml_doc(&self, doc: &mut ::yaml_rt::YamlDoc) -> Result<(), ::yaml_rt::YamlError> {
                ::yaml_rt::__write_yaml_document(self, doc)
            }
        }

        impl #combined_impl_generics ::yaml_rt::YamlValue for #name #combined_type_generics #combined_where_clause {
            fn read_yaml(
                doc: &::yaml_rt::YamlDoc,
                node: ::yaml_rt::NodeId,
            ) -> Result<Self, ::yaml_rt::YamlError> {
                #read_body
            }

            fn write_yaml(
                &self,
                doc: &mut ::yaml_rt::YamlDoc,
                node: Option<::yaml_rt::NodeId>,
            ) -> Result<::yaml_rt::NodeId, ::yaml_rt::YamlError> {
                let node = node.ok_or_else(|| {
                    ::yaml_rt::YamlError::new(::yaml_rt::Diagnostic::new(
                        ::yaml_rt::DiagnosticKind::Typed,
                        "cannot insert a standalone YAML enum without collection context",
                        ::yaml_rt::Span::empty(0),
                    ))
                })?;
                match self {
                    #(#write_arms,)*
                }
            }
        }

        impl #write_impl_generics ::yaml_rt::ToYamlFragment for #name #write_type_generics #write_where_clause {
            fn to_yaml_fragment(
                &self,
                indent: usize,
                line_ending: &str,
            ) -> Result<String, ::yaml_rt::YamlError> {
                match self {
                    #(#fragment_arms,)*
                }
            }
        }
    }
}

fn enum_generics_with_bounds(
    generics: syn::Generics,
    read_bounds: Vec<syn::WherePredicate>,
    write_bounds: Vec<syn::WherePredicate>,
) -> (syn::Generics, syn::Generics, syn::Generics) {
    let mut read_generics = generics.clone();
    read_generics
        .make_where_clause()
        .predicates
        .extend(read_bounds.iter().cloned());
    let mut write_generics = generics.clone();
    write_generics
        .make_where_clause()
        .predicates
        .extend(write_bounds.iter().cloned());
    let mut combined_generics = generics;
    combined_generics
        .make_where_clause()
        .predicates
        .extend(read_bounds);
    combined_generics
        .make_where_clause()
        .predicates
        .extend(write_bounds);
    (read_generics, write_generics, combined_generics)
}

fn accepted_variant_names(variant: &EnumVariant) -> Vec<&String> {
    std::iter::once(&variant.canonical)
        .chain(variant.aliases.iter())
        .collect()
}

fn register_enum_variant_names(
    ident: &syn::Ident,
    canonical: &str,
    aliases: &[String],
    names: &mut BTreeMap<String, String>,
) -> syn::Result<()> {
    let rust_name = ident.to_string();
    for accepted in std::iter::once(canonical).chain(aliases.iter().map(String::as_str)) {
        if let Some(previous) = names.insert(accepted.to_owned(), rust_name.clone()) {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "yaml enum variant name `{accepted}` is used by both `{previous}` and `{rust_name}`"
                ),
            ));
        }
    }
    Ok(())
}

fn validate_enum_tag_names(
    ident: &syn::Ident,
    canonical: &str,
    aliases: &[String],
) -> syn::Result<()> {
    for name in std::iter::once(canonical).chain(aliases.iter().map(String::as_str)) {
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err(syn::Error::new_spanned(
                ident,
                format!(
                    "`{name}` is not a valid local YAML tag name; use only ASCII letters, digits, `_`, or `-`"
                ),
            ));
        }
    }
    Ok(())
}

fn parse_payload_field(
    field: syn::Field,
    read_bounds: &mut Vec<WherePredicate>,
    write_bounds: &mut Vec<WherePredicate>,
) -> syn::Result<PayloadField> {
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
            &field,
            "unnamed enum fields support only yaml(with)",
        ));
    }
    let ty = field.ty;
    if let Some(with) = &options.with {
        read_bounds.push(syn::parse_quote! {
            #with::Repr: ::yaml_rt::YamlValue
        });
        write_bounds.push(syn::parse_quote! {
            #with::Repr: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
        });
    } else {
        read_bounds.push(syn::parse_quote! {
            #ty: ::yaml_rt::YamlValue
        });
        write_bounds.push(syn::parse_quote! {
            #ty: ::yaml_rt::YamlValue + ::yaml_rt::ToYamlFragment
        });
    }
    Ok(PayloadField {
        ty,
        with: options.with,
    })
}

fn tagged_variant_read_arm(variant: &EnumVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let accepted = accepted_variant_names(variant);
    let read = match &variant.kind {
        EnumVariantKind::Unit => unreachable!("unit variants do not use YAML tags"),
        EnumVariantKind::Newtype(field) => {
            let ty = &field.ty;
            if let Some(with) = &field.with {
                quote! {
                    {
                        let __yaml_rt_repr =
                            ::yaml_rt::__read_tagged_yaml_value::<#with::Repr>(doc, node)?;
                        Ok(Self::#ident(#with::from_yaml(__yaml_rt_repr)?))
                    }
                }
            } else {
                quote! {
                    Ok(Self::#ident(
                        ::yaml_rt::__read_tagged_yaml_value::<#ty>(doc, node)?
                    ))
                }
            }
        }
        EnumVariantKind::Tuple(fields) => {
            let arity = fields.len();
            let reads = fields.iter().enumerate().map(|(index, field)| {
                let ty = &field.ty;
                if let Some(with) = &field.with {
                    quote! {
                        {
                            let __yaml_rt_repr =
                                <#with::Repr as ::yaml_rt::YamlValue>::read_yaml(
                                    doc,
                                    __yaml_rt_items[#index],
                                )?;
                            #with::from_yaml(__yaml_rt_repr)?
                        }
                    }
                } else {
                    quote! {
                        <#ty as ::yaml_rt::YamlValue>::read_yaml(
                            doc,
                            __yaml_rt_items[#index],
                        )?
                    }
                }
            });
            quote! {
                {
                    if !matches!(
                        doc.semantic_kind(node),
                        Some(::yaml_rt::SemanticKind::Sequence { .. })
                    ) {
                        return Err(::yaml_rt::__typed_node_error(
                            doc,
                            node,
                            concat!(
                                "YAML enum variant `",
                                stringify!(#ident),
                                "` requires a sequence payload"
                            ),
                            &["a YAML sequence"],
                        ));
                    }
                    let __yaml_rt_items =
                        doc.sequence_items(node).collect::<::std::vec::Vec<_>>();
                    if __yaml_rt_items.len() != #arity {
                        return Err(::yaml_rt::__typed_node_error(
                            doc,
                            node,
                            format!(
                                "YAML enum variant `{}` expects {} tuple fields, found {}",
                                stringify!(#ident),
                                #arity,
                                __yaml_rt_items.len(),
                            ),
                            &[concat!(#arity, " sequence items")],
                        ));
                    }
                    Ok(Self::#ident(#(#reads),*))
                }
            }
        }
        EnumVariantKind::Struct { expansion, .. } => {
            let reads = &expansion.reads;
            let catch_all_count = flatten_catch_all_count(expansion);
            let claimed_keys = flatten_claimed_keys_binding(expansion, quote! { &[] });
            quote! {
                ::yaml_rt::__read_mapping_fields(doc, node, |doc| {
                    ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                    #claimed_keys
                    Ok(Self::#ident {
                        #(#reads,)*
                    })
                })
            }
        }
    };
    quote! {
        #(#accepted)|* => #read
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "all variant shapes must generate parallel write match arms"
)]
fn enum_variant_write_arm(variant: &EnumVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let accepted = accepted_variant_names(variant);
    let same_tag = quote! {
        doc.raw_tag(node)
            .and_then(|tag| tag.strip_prefix('!'))
            .is_some_and(|tag| matches!(tag, #(#accepted)|*))
    };

    match &variant.kind {
        EnumVariantKind::Unit => {
            quote! {
                Self::#ident => {
                    let __yaml_rt_has_local_tag = doc.raw_tag(node).is_some_and(|tag| {
                        tag.starts_with('!') && !tag.starts_with("!!")
                    });
                    if !__yaml_rt_has_local_tag
                        && <String as ::yaml_rt::YamlValue>::read_yaml(doc, node)
                            .is_ok_and(|value| matches!(value.as_str(), #(#accepted)|*))
                    {
                        Ok(node)
                    } else {
                        ::yaml_rt::__replace_yaml_value(self, doc, node)
                    }
                }
            }
        }
        EnumVariantKind::Newtype(field) => {
            let write = if let Some(with) = &field.with {
                quote! {
                    let __yaml_rt_repr = #with::to_yaml(value)?;
                    ::yaml_rt::__write_tagged_yaml_value(
                        &__yaml_rt_repr,
                        doc,
                        node,
                    )
                }
            } else {
                quote! {
                    ::yaml_rt::__write_tagged_yaml_value(value, doc, node)
                }
            };
            quote! {
                Self::#ident(value) => {
                    if #same_tag {
                        #write
                    } else {
                        ::yaml_rt::__replace_yaml_value(self, doc, node)
                    }
                }
            }
        }
        EnumVariantKind::Tuple(fields) => {
            let arity = fields.len();
            let bindings = (0..arity)
                .map(|index| syn::Ident::new(&format!("field_{index}"), ident.span()))
                .collect::<Vec<_>>();
            let writes =
                fields
                    .iter()
                    .zip(bindings.iter())
                    .enumerate()
                    .map(|(index, (field, binding))| {
                        let ty = &field.ty;
                        if let Some(with) = &field.with {
                            quote! {
                                let __yaml_rt_repr = #with::to_yaml(#binding)?;
                                <#with::Repr as ::yaml_rt::YamlValue>::write_yaml(
                                    &__yaml_rt_repr,
                                    doc,
                                    Some(__yaml_rt_items[#index]),
                                )?;
                            }
                        } else {
                            quote! {
                                <#ty as ::yaml_rt::YamlValue>::write_yaml(
                                    #binding,
                                    doc,
                                    Some(__yaml_rt_items[#index]),
                                )?;
                            }
                        }
                    });
            quote! {
                Self::#ident(#(#bindings),*) => {
                    if !(#same_tag) {
                        return ::yaml_rt::__replace_yaml_value(self, doc, node);
                    }
                    if !matches!(
                        doc.semantic_kind(node),
                        Some(::yaml_rt::SemanticKind::Sequence { .. })
                    ) {
                        return Err(::yaml_rt::__typed_node_error(
                            doc,
                            node,
                            concat!(
                                "YAML enum variant `",
                                stringify!(#ident),
                                "` requires a sequence payload"
                            ),
                            &["a YAML sequence"],
                        ));
                    }
                    let __yaml_rt_items =
                        doc.sequence_items(node).collect::<::std::vec::Vec<_>>();
                    if __yaml_rt_items.len() != #arity {
                        return Err(::yaml_rt::__typed_node_error(
                            doc,
                            node,
                            format!(
                                "YAML enum variant `{}` expects {} tuple fields, found {}",
                                stringify!(#ident),
                                #arity,
                                __yaml_rt_items.len(),
                            ),
                            &[concat!(#arity, " sequence items")],
                        ));
                    }
                    #(#writes)*
                    Ok(node)
                }
            }
        }
        EnumVariantKind::Struct { fields, expansion } => {
            let writes = &expansion.writes;
            let catch_all_count = flatten_catch_all_count(expansion);
            let claimed_keys = flatten_claimed_keys_binding(expansion, quote! { &[] });
            let validations = flatten_validation_tokens(expansion);
            quote! {
                Self::#ident { #(#fields),* } => {
                    if !(#same_tag) {
                        return ::yaml_rt::__replace_yaml_value(self, doc, node);
                    }
                    ::yaml_rt::__write_mapping_fields(doc, node, |doc| {
                        ::yaml_rt::__validate_yaml_flatten_layout(doc, #catch_all_count)?;
                        #claimed_keys
                        #(#validations)*
                        let root = doc.root_mapping()?;
                        #(#writes)*
                        Ok(())
                    })
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "all variant shapes must generate parallel fragment match arms"
)]
fn enum_variant_fragment_arm(variant: &EnumVariant) -> TokenStream2 {
    let ident = &variant.ident;
    let canonical = &variant.canonical;
    match &variant.kind {
        EnumVariantKind::Unit => quote! {
            Self::#ident => <String as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                &#canonical.to_owned(),
                indent,
                line_ending,
            )
        },
        EnumVariantKind::Newtype(field) => {
            let payload = if let Some(with) = &field.with {
                quote! {
                    let __yaml_rt_repr = #with::to_yaml(value)?;
                    <#with::Repr as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                        &__yaml_rt_repr,
                        indent,
                        line_ending,
                    )?
                }
            } else {
                let ty = &field.ty;
                quote! {
                    <#ty as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                        value,
                        indent,
                        line_ending,
                    )?
                }
            };
            quote! {
                Self::#ident(value) => {
                    let __yaml_rt_payload = { #payload };
                    ::yaml_rt::__tag_yaml_fragment(
                        #canonical,
                        &__yaml_rt_payload,
                        indent,
                        line_ending,
                    )
                }
            }
        }
        EnumVariantKind::Tuple(fields) => {
            let bindings = (0..fields.len())
                .map(|index| syn::Ident::new(&format!("field_{index}"), ident.span()))
                .collect::<Vec<_>>();
            let fragments = fields.iter().zip(bindings.iter()).map(|(field, binding)| {
                let ty = &field.ty;
                if let Some(with) = &field.with {
                    quote! {
                        {
                            let __yaml_rt_repr = #with::to_yaml(#binding)?;
                            <#with::Repr as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                                &__yaml_rt_repr,
                                indent + 2,
                                line_ending,
                            )?
                        }
                    }
                } else {
                    quote! {
                        <#ty as ::yaml_rt::ToYamlFragment>::to_yaml_fragment(
                            #binding,
                            indent + 2,
                            line_ending,
                        )?
                    }
                }
            });
            quote! {
                Self::#ident(#(#bindings),*) => {
                    let __yaml_rt_fields = [#(#fragments),*];
                    let __yaml_rt_payload = ::yaml_rt::__sequence_fields_to_yaml_fragment(
                        &__yaml_rt_fields,
                        indent,
                        line_ending,
                    );
                    ::yaml_rt::__tag_yaml_fragment(
                        #canonical,
                        &__yaml_rt_payload,
                        indent,
                        line_ending,
                    )
                }
            }
        }
        EnumVariantKind::Struct { fields, expansion } => {
            let writes = &expansion.writes;
            let catch_all_count = flatten_catch_all_count(expansion);
            let claimed_keys = flatten_claimed_keys_binding(expansion, quote! { &[] });
            let validations = flatten_validation_tokens(expansion);
            quote! {
                Self::#ident { #(#fields),* } => {
                    let __yaml_rt_payload =
                        ::yaml_rt::__mapping_fields_to_yaml_fragment(
                            indent,
                            line_ending,
                            |doc| {
                                ::yaml_rt::__validate_yaml_flatten_layout(
                                    doc,
                                    #catch_all_count,
                                )?;
                                #claimed_keys
                                #(#validations)*
                                let root = doc.root_mapping()?;
                                #(#writes)*
                                Ok(())
                            },
                        )?;
                    ::yaml_rt::__tag_yaml_fragment(
                        #canonical,
                        &__yaml_rt_payload,
                        indent,
                        line_ending,
                    )
                }
            }
        }
    }
}

fn apply_rename_rule(name: &str, rule: Option<RenameRule>) -> String {
    let name = name.strip_prefix("r#").unwrap_or(name);
    match rule {
        None | Some(RenameRule::PascalCase) => name.to_owned(),
        Some(RenameRule::Lowercase) => name.to_ascii_lowercase(),
        Some(RenameRule::SnakeCase) => variant_snake_case(name),
        Some(RenameRule::KebabCase) => variant_snake_case(name).replace('_', "-"),
        Some(RenameRule::ScreamingSnakeCase) => variant_snake_case(name).to_ascii_uppercase(),
        Some(RenameRule::CamelCase) => {
            let mut value = name.to_owned();
            if let Some(first) = value.get_mut(0..1) {
                first.make_ascii_lowercase();
            }
            value
        }
    }
}

fn variant_snake_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    for (index, character) in name.char_indices() {
        if index > 0 && character.is_uppercase() {
            output.push('_');
        }
        output.push(character.to_ascii_lowercase());
    }
    output
}

fn expand_fields(
    fields: syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
    insert_order: InsertOrder,
    access: FieldAccess,
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
            push_flatten_field(&mut expansion, &field_name, &field_type, &options, access)?;
            continue;
        }

        push_regular_field(
            &mut expansion,
            &field_name,
            &field_type,
            &options,
            insert_order,
            access,
        )?;
    }

    Ok(expansion)
}

fn push_flatten_field(
    expansion: &mut FieldExpansion,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    options: &FieldOptions,
    access: FieldAccess,
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
        #field_type: ::yaml_rt::YamlFlatten
    });
    expansion.write_bounds.push(syn::parse_quote! {
        #field_type: ::yaml_rt::YamlFlatten
    });
    expansion.reads.push(quote! {
        #field_name: <#field_type as ::yaml_rt::YamlFlatten>::from_yaml_flattened(
            doc,
            &__yaml_rt_claimed_keys,
        )?
    });
    let field_value = field_reference(field_name, access);
    expansion.writes.push(quote! {
        <#field_type as ::yaml_rt::YamlFlatten>::apply_to_yaml_flattened(
            #field_value,
            doc,
            &__yaml_rt_claimed_keys,
        )?;
    });
    expansion.flatten_types.push(field_type.clone());
    expansion.flatten_values.push(field_value);
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "field read, write, and insertion tokens share one validated option set"
)]
fn push_regular_field(
    expansion: &mut FieldExpansion,
    field_name: &syn::Ident,
    field_type: &syn::Type,
    options: &FieldOptions,
    insert_order: InsertOrder,
    access: FieldAccess,
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
    let field_value = field_reference(field_name, access);

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
                    let __yaml_rt_repr = #with::to_yaml(#field_value)?;
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
                field_value.clone(),
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
        &field_value,
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
    field_value: &TokenStream2,
    yaml_key: &str,
    aliases: &[String],
    skip_serializing_if: Option<Path>,
    write_field: TokenStream2,
) {
    if let Some(predicate) = skip_serializing_if {
        field_writes.push(quote! {
            if #predicate(#field_value) {
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

fn field_reference(field_name: &syn::Ident, access: FieldAccess) -> TokenStream2 {
    match access {
        FieldAccess::SelfFields => quote! { &self.#field_name },
        FieldAccess::Bindings => quote! { #field_name },
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

fn parse_enum_options(attrs: &[Attribute]) -> syn::Result<EnumOptions> {
    let mut options = EnumOptions::default();
    for attr in attrs {
        if !attr.path().is_ident("yaml") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("rename_all") {
                return Err(meta.error("unsupported yaml enum attribute"));
            }
            let value = meta.value()?.parse::<LitStr>()?;
            options.rename_all = Some(match value.value().as_str() {
                "lowercase" => RenameRule::Lowercase,
                "snake_case" => RenameRule::SnakeCase,
                "kebab-case" => RenameRule::KebabCase,
                "SCREAMING_SNAKE_CASE" => RenameRule::ScreamingSnakeCase,
                "camelCase" => RenameRule::CamelCase,
                "PascalCase" => RenameRule::PascalCase,
                _ => {
                    return Err(syn::Error::new_spanned(
                        value,
                        "rename_all must be lowercase, snake_case, kebab-case, SCREAMING_SNAKE_CASE, camelCase, or PascalCase",
                    ));
                }
            });
            Ok(())
        })?;
    }
    Ok(options)
}

fn parse_variant_options(attrs: &[Attribute]) -> syn::Result<VariantOptions> {
    let mut options = VariantOptions::default();
    for attr in attrs {
        if !attr.path().is_ident("yaml") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                options.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("alias") {
                options
                    .aliases
                    .push(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("enum variants support only yaml(rename) and yaml(alias)"))
            }
        })?;
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

    #[test]
    fn duplicate_variant_names_report_both_variants() {
        let input: DeriveInput = syn::parse_quote! {
            enum Invalid {
                First,
                #[yaml(rename = "First")]
                Second,
            }
        };

        let error = expand_yaml_round_trip(input).expect_err("duplicate names must fail");
        let message = error.to_string();

        assert!(message.contains("yaml enum variant name `First`"));
        assert!(message.contains("both `First` and `Second`"));
    }

    #[test]
    fn data_variant_tags_reject_invalid_local_tag_characters() {
        let input: DeriveInput = syn::parse_quote! {
            enum Invalid {
                #[yaml(rename = "not/a/tag")]
                Value(u16),
            }
        };

        let error = expand_yaml_round_trip(input).expect_err("invalid tag must fail");

        assert!(
            error
                .to_string()
                .contains("not a valid local YAML tag name")
        );
    }

    #[test]
    fn unsupported_rename_rule_is_targeted() {
        let input: DeriveInput = syn::parse_quote! {
            #[yaml(rename_all = "UPPERCASE")]
            enum Invalid {
                Value,
            }
        };

        let error = expand_yaml_round_trip(input).expect_err("unsupported rename rule must fail");

        assert!(error.to_string().contains(
            "rename_all must be lowercase, snake_case, kebab-case, SCREAMING_SNAKE_CASE, camelCase, or PascalCase"
        ));
    }

    #[test]
    fn invalid_variant_and_unnamed_payload_attributes_are_targeted() {
        let variant_input: DeriveInput = syn::parse_quote! {
            enum Invalid {
                #[yaml(default)]
                Value(u16),
            }
        };
        let payload_input: DeriveInput = syn::parse_quote! {
            enum Invalid {
                Value(#[yaml(rename = "value")] u16),
            }
        };

        let variant_error =
            expand_yaml_round_trip(variant_input).expect_err("invalid variant attr must fail");
        let payload_error =
            expand_yaml_round_trip(payload_input).expect_err("invalid payload attr must fail");

        assert!(
            variant_error
                .to_string()
                .contains("enum variants support only yaml(rename) and yaml(alias)")
        );
        assert!(
            payload_error
                .to_string()
                .contains("unnamed enum fields support only yaml(with)")
        );
    }
}

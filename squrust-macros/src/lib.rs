//! Procedural macros for Squrust: `#[derive(FromRow)]`, `#[derive(ToParams)]`,
//! the compile-time-checked `sql!` macro, and `migrate!`.

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr, parse_macro_input};

mod schema;

/// Per-field `#[squrust(...)]` options.
#[derive(Default)]
struct FieldOpts {
    rename: Option<String>,
    skip: bool,
}

fn parse_field_opts(attrs: &[syn::Attribute]) -> syn::Result<FieldOpts> {
    let mut opts = FieldOpts::default();
    for attr in attrs {
        if !attr.path().is_ident("squrust") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                opts.skip = true;
                Ok(())
            } else if meta.path.is_ident("rename") {
                let value = meta.value()?;
                let s: LitStr = value.parse()?;
                opts.rename = Some(s.value());
                Ok(())
            } else {
                Err(meta.error("unknown squrust attribute"))
            }
        })?;
    }
    Ok(opts)
}

/// `#[derive(FromRow)]` — maps result columns to struct fields by name.
#[proc_macro_derive(FromRow, attributes(squrust))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;
    let (impl_g, ty_g, where_c) = ast.generics.split_for_impl();

    let fields = match named_fields(&ast) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };

    let mut inits = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        let opts = match parse_field_opts(&field.attrs) {
            Ok(o) => o,
            Err(e) => return e.to_compile_error().into(),
        };
        if opts.skip {
            inits.push(quote! { #ident: ::core::default::Default::default() });
        } else {
            let col = opts.rename.unwrap_or_else(|| ident.to_string());
            inits.push(quote! {
                #ident: ::squrust_serde::RowAccess::get_by_name::<#ty>(row, #col)?
            });
        }
    }

    quote! {
        impl #impl_g ::squrust_serde::FromRow for #name #ty_g #where_c {
            fn from_row<__R: ::squrust_serde::RowAccess + ?Sized>(
                row: &__R,
            ) -> ::core::result::Result<Self, ::squrust_sql::SqlError> {
                ::core::result::Result::Ok(#name { #(#inits),* })
            }
        }
    }
    .into()
}

/// `#[derive(ToParams)]` — turns a struct into a positional parameter list.
#[proc_macro_derive(ToParams, attributes(squrust))]
pub fn derive_to_params(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;
    let (impl_g, ty_g, where_c) = ast.generics.split_for_impl();

    let fields = match named_fields(&ast) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };

    let mut pushes = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap();
        let opts = match parse_field_opts(&field.attrs) {
            Ok(o) => o,
            Err(e) => return e.to_compile_error().into(),
        };
        if opts.skip {
            continue;
        }
        pushes.push(quote! { out.push(::squrust_sql::Value::from(self.#ident)); });
    }

    quote! {
        impl #impl_g ::squrust_serde::ToParams for #name #ty_g #where_c {
            fn to_params(self) -> ::std::vec::Vec<::squrust_sql::Value> {
                let mut out = ::std::vec::Vec::new();
                #(#pushes)*
                out
            }
        }
    }
    .into()
}

fn named_fields(ast: &DeriveInput) -> syn::Result<Vec<&syn::Field>> {
    match &ast.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => Ok(named.named.iter().collect()),
            _ => Err(syn::Error::new(
                ast.span(),
                "FromRow/ToParams require a struct with named fields",
            )),
        },
        _ => Err(syn::Error::new(
            ast.span(),
            "FromRow/ToParams can only be derived for structs",
        )),
    }
}

/// `sql!("...")` — validate the SQL against the project schema at compile time,
/// expanding to the validated string literal.
#[proc_macro]
pub fn sql(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    let query = lit.value();
    match schema::validate_sql(&query) {
        Ok(()) => quote! { #lit }.into(),
        Err(msg) => quote_spanned! { lit.span() => compile_error!(#msg) }.into(),
    }
}

/// `migrate!("./migrations")` — embed all `.sql` files in a directory as a
/// `&[Migration]`, ordered by numeric filename prefix.
#[proc_macro]
pub fn migrate(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    match schema::load_migrations(&lit.value()) {
        Ok(entries) => {
            let items = entries.into_iter().map(|m| {
                let version = m.version;
                let description = m.description;
                let sql = m.sql;
                quote! {
                    ::squrust_async::Migration {
                        version: #version,
                        description: #description,
                        sql: #sql,
                    }
                }
            });
            quote! { &[ #(#items),* ] }.into()
        }
        Err(msg) => quote_spanned! { lit.span() => compile_error!(#msg) }.into(),
    }
}

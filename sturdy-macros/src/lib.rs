//! Derive macros for `sturdy`. Currently just `#[derive(Bundle)]`; see `sturdy::ecs::Bundle`'s doc
//! comment for what it produces and why.
//!
//! Generated code refers to the trait/types it needs through `::sturdy::...` paths rather than
//! `::sturdy_macros::...` ones: this crate is a plain `proc-macro = true` crate with no runtime
//! dependency on `sturdy` (that would be circular, since `sturdy` depends on this crate for the
//! derive itself), so it has no way to name `sturdy`'s types directly — it can only emit source
//! that resolves them from the *user's* crate, which is expected to depend on `sturdy` under that
//! name (as `#[derive(sturdy::Bundle)]`, or `use sturdy::prelude::*;` then `#[derive(Bundle)]`,
//! implies).

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

/// Derives `sturdy::ecs::Bundle` for a plain struct whose every field implements
/// `sturdy::ecs::Component`. See that trait's doc comment for the generated shape and
/// `World::spawn`/`insert_bundle`/`remove_bundle` for how it's used.
#[proc_macro_derive(Bundle)]
pub fn derive_bundle(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            Fields::Unnamed(_) => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "#[derive(Bundle)] requires named fields, not a tuple struct",
                )
                .to_compile_error()
                .into();
            }
            Fields::Unit => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "#[derive(Bundle)] requires at least one field",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "#[derive(Bundle)] only supports structs with named fields",
            )
            .to_compile_error()
            .into();
        }
    };

    if fields.is_empty() {
        return syn::Error::new_spanned(
            &input.ident,
            "#[derive(Bundle)] requires at least one field",
        )
        .to_compile_error()
        .into();
    }

    let field_idents: Vec<_> = fields.iter().map(|f| f.ident.clone().unwrap()).collect();
    let field_types: Vec<_> = fields.iter().map(|f| f.ty.clone()).collect();

    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics ::sturdy::Bundle for #name #ty_generics #where_clause {
            fn register_all(
                world: &mut ::sturdy::World<'_>,
            ) -> ::core::result::Result<::std::vec::Vec<::sturdy::ComponentId>, ::sturdy::EcsError> {
                ::core::result::Result::Ok(::std::vec![
                    #( world.register::<#field_types>()?, )*
                ])
            }

            fn into_parts(
                self,
                world: &mut ::sturdy::World<'_>,
            ) -> ::core::result::Result<
                ::std::vec::Vec<(::sturdy::ComponentId, ::std::boxed::Box<[u8]>)>,
                ::sturdy::EcsError,
            > {
                ::core::result::Result::Ok(::std::vec![
                    #(
                        (
                            world.register::<#field_types>()?,
                            ::sturdy::bytemuck::bytes_of(&self.#field_idents)
                                .to_vec()
                                .into_boxed_slice(),
                        ),
                    )*
                ])
            }
        }
    };

    expanded.into()
}

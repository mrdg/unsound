use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DataStruct, DeriveInput, Fields, Ident};

#[proc_macro_derive(Params)]
pub fn derive_params(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_name = ast.ident;
    let fields = match ast.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(fields),
            ..
        }) => fields,
        _ => {
            return syn::Error::new(
                struct_name.span(),
                "Params can only be derived for a struct with named fields",
            )
            .to_compile_error()
            .into();
        }
    };

    let mut num_params: usize = 0;
    let mut match_arms = Vec::new();
    let mut constants = Vec::new();

    // TODO: only select fields marked by an attribute?
    for (index, field) in fields.named.iter().enumerate() {
        if let Some(ident) = &field.ident {
            num_params += 1;
            match_arms.push(quote! {
                #index => &self.#ident
            });

            let const_name = Ident::new(&ident.to_string().to_uppercase(), ident.span());
            constants.push(quote! {
                pub const #const_name: usize = #index;
            });
        }
    }
    match_arms.push(quote! {
        _ => unreachable!(),
    });

    let constant_impl = quote! {
        impl #struct_name {
            #(#constants)*
        }
    };

    let params_impl = quote! {
        impl Params for #struct_name {
            fn len(&self) -> usize {
                #num_params
            }

            fn get_param(&self, idx: usize) -> &params::Param {
                match idx {
                    #(#match_arms),*
                }
            }
        }
    };

    TokenStream::from(quote! {
        #constant_impl
        #params_impl
    })
}

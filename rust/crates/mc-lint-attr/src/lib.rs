use proc_macro::TokenStream;
use quote::quote;
use syn::{parse::Parser, punctuated::Punctuated, ItemFn, Meta, Token};

/// Marks a function as a hot-path root.
///
/// Hotness is meant to propagate through calls in the external linter.
/// This macro itself does not enforce anything. It preserves the function
/// unchanged and exists so code can be annotated naturally.
///
/// Supported options:
///
/// #[hot_path]
/// #[hot_path(allow_validation)]
/// #[hot_path(allow_allocation)]
/// #[hot_path(allow_branching)]
/// #[hot_path(allow_logging)]
/// #[hot_path(allow_panics)]
/// #[hot_path(allow_formatting)]
///
/// Options can be combined:
///
/// #[hot_path(allow_validation, allow_branching)]
#[proc_macro_attribute]
pub fn hot_path(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_marker_attribute("hot_path", args, input)
}

/// Marks a function as a hot-path boundary.
///
/// Intended meaning for the linter:
///
/// - the boundary function itself may do setup/validation
/// - hotness should not automatically propagate through every call
/// - explicitly marked calls inside can still be hot roots
///
/// The macro preserves the function unchanged.
#[proc_macro_attribute]
pub fn hot_path_boundary(args: TokenStream, input: TokenStream) -> TokenStream {
    expand_marker_attribute("hot_path_boundary", args, input)
}

fn expand_marker_attribute(
    attribute_name: &str,
    args: TokenStream,
    input: TokenStream,
) -> TokenStream {
    let item_fn = match syn::parse::<ItemFn>(input.clone()) {
        Ok(item_fn) => item_fn,
        Err(error) => {
            return error.to_compile_error().into();
        }
    };

    if let Err(error) = validate_args(attribute_name, args) {
        return error.to_compile_error().into();
    }

    quote!(#item_fn).into()
}

fn validate_args(attribute_name: &str, args: TokenStream) -> syn::Result<()> {
    let args_ts: proc_macro2::TokenStream = args.into();

    if args_ts.is_empty() {
        return Ok(());
    }

    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse2(args_ts)?;

    for meta in metas {
        let Meta::Path(path) = meta else {
            return Err(syn::Error::new_spanned(
                meta,
                format!("`{attribute_name}` only supports simple flags like `allow_validation`"),
            ));
        };

        if path.is_ident("allow_validation")
            || path.is_ident("allow_allocation")
            || path.is_ident("allow_branching")
            || path.is_ident("allow_logging")
            || path.is_ident("allow_panics")
            || path.is_ident("allow_formatting")
        {
            continue;
        }

        return Err(syn::Error::new_spanned(
            path,
            format!("unknown `{attribute_name}` option"),
        ));
    }

    Ok(())
}

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse_macro_input, visit_mut::VisitMut, ItemFn, Expr, ExprCall, ExprPath,
};

/// Visit function body and rewrite calls:
/// - `tokio::spawn(expr)` -> `detector::spawn_monitored("auto", expr)`
/// - `tokio::task::spawn_blocking(expr)` -> `detector::spawn_blocking_monitored("auto", || expr())`
///
/// This is intentionally conservative: it looks for path segments exactly matching the patterns.
struct SpawnRewriter;

impl VisitMut for SpawnRewriter {
    fn visit_expr_call_mut(&mut self, node: &mut ExprCall) {
        // First recursively visit args
        syn::visit_mut::visit_expr_call_mut(self, node);

        // We're looking for function path expressions
        if let Expr::Path(ExprPath { path, .. }) = &*node.func {
            let segs: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();

            // Match tokio::spawn(...)
            if segs == ["tokio", "spawn"] {
                // node.args is the function/future expression — keep as-is.
                if node.args.len() == 1 {
                    let orig_arg = node.args.first().unwrap();
                    let loc = std::panic::Location::caller();
                    let file = loc.file();
                    let line = loc.line();
                    let column = loc.column();
                    let new = quote! {
                        ::detector::spawn_monitored(::std::format!("{}:{}:{}", #file, #line, #column), #orig_arg)
                    };
                    *node = syn::parse2(new).expect("parsed");
                }
            }

            // Match tokio::task::spawn_blocking(...)
            if segs == ["tokio", "task", "spawn_blocking"] {
                // spawn_blocking normally takes a closure or fn; keep arg as-is.
                if node.args.len() == 1 {
                    let orig_arg = node.args.first().unwrap();
                    let loc = std::panic::Location::caller();
                    let file = loc.file();
                    let line = loc.line();
                    let column = loc.column();
                    // Ensure we pass a closure producing R — we keep the arg verbatim.
                    let new = quote! {
                        ::detector::spawn_blocking_monitored(::std::format!("{}:{}:{}", #file, #line, #column), #orig_arg)
                    };
                    *node = syn::parse2(new).expect("parsed");
                }
            }
        }
    }
}

/// Attribute macro entrypoint.
#[proc_macro_attribute]
pub fn monitored(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);
    let mut rewriter = SpawnRewriter;
    rewriter.visit_item_fn_mut(&mut func);
    TokenStream::from(quote! { #func })
}
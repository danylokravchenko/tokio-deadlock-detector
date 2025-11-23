use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Expr, ExprCall, ExprPath, Item, ItemFn, UseTree, parse_macro_input, visit_mut::VisitMut,
};

/// Visit function body and rewrite calls:
/// - `tokio::spawn(expr)` -> `detector::spawn_monitored("auto", expr)`
/// - `tokio::task::spawn_blocking(expr)` -> `detector::spawn_blocking_monitored("auto", || expr())`
///
/// This is intentionally conservative: it looks for path segments exactly matching the patterns.

struct SpawnRewriter {
    spawn_names: Vec<String>,
    spawn_blocking_names: Vec<String>,
}

impl VisitMut for SpawnRewriter {
    fn visit_expr_call_mut(&mut self, node: &mut ExprCall) {
        // First recursively visit args
        syn::visit_mut::visit_expr_call_mut(self, node);

        // We're looking for function path expressions
        if let Expr::Path(ExprPath { path, .. }) = &*node.func {
            let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            // Check for direct calls or aliases
            if segs.len() == 1 {
                let name = &segs[0];
                if self.spawn_names.contains(name) {
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
                } else if self.spawn_blocking_names.contains(name) {
                    if node.args.len() == 1 {
                        let orig_arg = node.args.first().unwrap();
                        let loc = std::panic::Location::caller();
                        let file = loc.file();
                        let line = loc.line();
                        let column = loc.column();
                        let new = quote! {
                            ::detector::spawn_blocking_monitored(::std::format!("{}:{}:{}", #file, #line, #column), #orig_arg)
                        };
                        *node = syn::parse2(new).expect("parsed");
                    }
                }
            }
            // Also keep explicit tokio paths
            else if segs.len() >= 2 && segs[0] == "tokio" {
                let segs_str: Vec<&str> = segs.iter().map(|s| s.as_str()).collect();
                match segs_str.as_slice() {
                    ["tokio", "spawn"] => {
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
                    ["tokio", "task", "spawn_blocking"] => {
                        if node.args.len() == 1 {
                            let orig_arg = node.args.first().unwrap();
                            let loc = std::panic::Location::caller();
                            let file = loc.file();
                            let line = loc.line();
                            let column = loc.column();
                            let new = quote! {
                                ::detector::spawn_blocking_monitored(::std::format!("{}:{}:{}", #file, #line, #column), #orig_arg)
                            };
                            *node = syn::parse2(new).expect("parsed");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Attribute macro entrypoint.
#[proc_macro_attribute]
pub fn monitored(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the function as usual
    let func = parse_macro_input!(item as ItemFn);

    // Try to get the full file from the macro input (hack: proc_macro input is the item, not the file)
    // If you want to use this in a real project, use a function-like macro at the module level
    // For now, fallback to default names and scan the function block for use statements

    // Fallback: parse the function block for use statements
    let mut spawn_names = vec!["spawn".to_string()];
    let mut spawn_blocking_names = vec!["spawn_blocking".to_string()];

    // Scan the function block for use statements
    for stmt in &func.block.stmts {
        if let syn::Stmt::Item(Item::Use(u)) = stmt {
            collect_spawn_aliases(&u.tree, &mut spawn_names, &mut spawn_blocking_names);
        }
    }

    let mut func = func;
    let mut rewriter = SpawnRewriter {
        spawn_names,
        spawn_blocking_names,
    };
    rewriter.visit_item_fn_mut(&mut func);
    TokenStream::from(quote! { #func })
}

fn collect_spawn_aliases(
    tree: &UseTree,
    spawn_names: &mut Vec<String>,
    spawn_blocking_names: &mut Vec<String>,
) {
    match tree {
        UseTree::Path(p) => {
            // Recurse into the path
            collect_spawn_aliases(&p.tree, spawn_names, spawn_blocking_names);
        }
        UseTree::Name(n) => {
            // Check for direct import
            let ident = n.ident.to_string();
            if ident == "spawn" {
                spawn_names.push(ident);
            } else if ident == "spawn_blocking" {
                spawn_blocking_names.push(ident);
            }
        }
        UseTree::Rename(r) => {
            // Check for alias
            let orig = r.ident.to_string();
            let alias = r.rename.to_string();
            if orig == "spawn" {
                spawn_names.push(alias);
            } else if orig == "spawn_blocking" {
                spawn_blocking_names.push(alias);
            }
        }
        UseTree::Group(g) => {
            for t in &g.items {
                collect_spawn_aliases(t, spawn_names, spawn_blocking_names);
            }
        }
        _ => {}
    }
}

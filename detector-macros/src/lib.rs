/// Holds all relevant aliases for rewriting.
#[derive(Default)]
struct RewriteAliases {
    spawn: Vec<String>,
    spawn_blocking: Vec<String>,
}

/// Collects all relevant aliases from use statements.
fn collect_rewrite_aliases(stmts: &[syn::Stmt]) -> RewriteAliases {
    let mut aliases = RewriteAliases::default();
    for stmt in stmts {
        if let syn::Stmt::Item(Item::Use(u)) = stmt {
            collect_from_use_tree(&u.tree, &mut aliases);
        }
    }
    // Always include default names
    if !aliases.spawn.contains(&"spawn".to_string()) {
        aliases.spawn.push("spawn".to_string());
    }
    if !aliases
        .spawn_blocking
        .contains(&"spawn_blocking".to_string())
    {
        aliases.spawn_blocking.push("spawn_blocking".to_string());
    }
    aliases
}

fn collect_from_use_tree(tree: &UseTree, aliases: &mut RewriteAliases) {
    match tree {
        UseTree::Path(p) => {
            // Recurse into the path
            collect_from_use_tree(&p.tree, aliases);
        }
        UseTree::Name(n) => {
            let ident = n.ident.to_string();
            match ident.as_str() {
                "spawn" => aliases.spawn.push(ident),
                "spawn_blocking" => aliases.spawn_blocking.push(ident),
                _ => {}
            }
        }
        UseTree::Rename(r) => {
            let orig = r.ident.to_string();
            let alias = r.rename.to_string();
            match orig.as_str() {
                "spawn" => aliases.spawn.push(alias),
                "spawn_blocking" => aliases.spawn_blocking.push(alias),
                _ => {}
            }
        }
        UseTree::Group(g) => {
            for t in &g.items {
                collect_from_use_tree(t, aliases);
            }
        }
        _ => {}
    }
}
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
    aliases: RewriteAliases,
}

impl VisitMut for SpawnRewriter {
    fn visit_expr_call_mut(&mut self, node: &mut ExprCall) {
        syn::visit_mut::visit_expr_call_mut(self, node);
        if let Expr::Path(ExprPath { path, .. }) = &*node.func {
            let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            if segs.len() == 1 {
                let name = &segs[0];
                if self.aliases.spawn.contains(name) {
                    if node.args.len() == 1 {
                        let orig_arg = node.args.first().unwrap();
                        let new = quote! {
                            ::detector::spawn_monitored(stringify!(#name), #orig_arg)
                        };
                        *node = syn::parse2(new).expect("parsed");
                    }
                } else if self.aliases.spawn_blocking.contains(name) {
                    if node.args.len() == 1 {
                        let orig_arg = node.args.first().unwrap();
                        let new = quote! {
                            ::detector::spawn_blocking_monitored(stringify!(#name), #orig_arg)
                        };
                        *node = syn::parse2(new).expect("parsed");
                    }
                }
            } else if segs.len() >= 2 && segs[0] == "tokio" {
                let segs_str: Vec<&str> = segs.iter().map(|s| s.as_str()).collect();
                match segs_str.as_slice() {
                    ["tokio", "spawn"] => {
                        if node.args.len() == 1 {
                            let orig_arg = node.args.first().unwrap();
                            let new = quote! {
                                ::detector::spawn_monitored("tokio::spawn", #orig_arg)
                            };
                            *node = syn::parse2(new).expect("parsed");
                        }
                    }
                    ["tokio", "task", "spawn_blocking"] => {
                        if node.args.len() == 1 {
                            let orig_arg = node.args.first().unwrap();
                            let new = quote! {
                                ::detector::spawn_blocking_monitored("tokio::task::spawn_blocking", #orig_arg)
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
    let mut func = parse_macro_input!(item as ItemFn);
    let aliases = collect_rewrite_aliases(&func.block.stmts);
    let mut rewriter = SpawnRewriter { aliases };
    rewriter.visit_item_fn_mut(&mut func);
    TokenStream::from(quote! { #func })
}

use std::{env, fs, path::PathBuf};
use syn::Item;

fn main() {
    let authority = "../../src/rpc_routes/user/handlers.rs";
    println!("cargo:rerun-if-changed={authority}");
    let source = fs::read_to_string(authority).expect("read actual production Rust RPC handlers");
    let tree = syn::parse_file(&source).expect("parse production handlers Rust syntax");
    let user_summary = tree
        .items
        .into_iter()
        .find_map(|item| match item {
            Item::Struct(item) if item.ident == "UserSummary" => Some(item),
            _ => None,
        })
        .expect("UserSummary must exist in real production Rust source");
    let rendered = quote::quote!(#user_summary).to_string();
    let out = PathBuf::from(env::var("OUT_DIR").expect("Cargo OUT_DIR"));
    fs::write(out.join("user_summary.rs"), rendered).expect("materialize exact source struct");
}

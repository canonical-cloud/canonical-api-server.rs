use std::{env, fs, path::PathBuf};

use ores_api_docs::{api_server_glue, discover_fs_routes, FsRouteKind, RouteMap};

fn main() {
    if let Err(error) = generate() {
        panic!("canonical filesystem route generation failed: {error}");
    }
}

fn generate() -> Result<(), String> {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|error| error.to_string())?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|error| error.to_string())?);
    let route_map_path = root.join("contracts/filesystem-pilot.route-map.json");

    println!("cargo:rerun-if-changed=src/routes");
    println!("cargo:rerun-if-changed={}", route_map_path.display());

    let routes = discover_fs_routes(&root, FsRouteKind::ApiHandler)?;
    for route in &routes {
        println!("cargo:rerun-if-changed={}", root.join(&route.source).display());
    }

    let route_map_source = fs::read_to_string(&route_map_path)
        .map_err(|error| format!("read {}: {error}", route_map_path.display()))?;
    let route_map = RouteMap::from_json_str(&route_map_source)
        .map_err(|error| format!("parse {}: {error}", route_map_path.display()))?;
    let glue = api_server_glue(&root, &routes, &route_map)?;
    fs::write(out_dir.join("ores_filesystem_api.rs"), glue)
        .map_err(|error| format!("write generated API route glue: {error}"))?;
    Ok(())
}

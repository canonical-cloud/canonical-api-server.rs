use std::{env, fs, path::Path};

const LIB_NAME: &str = "canonical-lib";
const ORM_NAME: &str = "canonical-orm-core";
const API_NAME: &str = "canonical-api-server";

fn dependency_rev(manifest: &str, dependency: &str) -> Result<String, String> {
    let prefix = format!("{dependency} = {{");
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .ok_or_else(|| format!("Cargo.toml does not declare {dependency}"))?;
    let rev_marker = "rev = \"";
    let rev_start = line
        .find(rev_marker)
        .map(|index| index + rev_marker.len())
        .ok_or_else(|| format!("{dependency} must use an exact rev"))?;
    let tail = &line[rev_start..];
    let rev_end = tail
        .find('"')
        .ok_or_else(|| format!("{dependency} rev is unterminated"))?;
    let rev = &tail[..rev_end];
    if rev.len() != 40 || !rev.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return Err(format!("{dependency} rev must be exact lowercase 40-hex"));
    }
    Ok(rev.to_owned())
}

fn direct_dependency_present(manifest: &str, dependency: &str) -> bool {
    let prefix = format!("{dependency} = ");
    manifest
        .lines()
        .any(|line| line.trim_start().starts_with(&prefix))
}

fn package_bounds(lock: &str, package: &str) -> Result<(usize, usize), String> {
    let marker = format!("[[package]]\nname = \"{package}\"\n");
    let start = lock
        .find(&marker)
        .ok_or_else(|| format!("Cargo.lock does not contain package {package}"))?;
    let after = start + marker.len();
    let end = lock[after..]
        .find("\n[[package]]\n")
        .map(|offset| after + offset)
        .unwrap_or(lock.len());
    Ok((start, end))
}

fn replace_git_source(
    lock: &mut String,
    package: &str,
    repository: &str,
    desired_rev: &str,
) -> Result<(), String> {
    let (start, end) = package_bounds(lock, package)?;
    let block = &lock[start..end];
    let source_prefix = format!("source = \"git+{repository}?rev=");
    let source_start = block
        .find(&source_prefix)
        .ok_or_else(|| format!("{package} lock entry does not use expected repository {repository}"))?;
    let absolute_start = start + source_start;
    let line_end = lock[absolute_start..]
        .find('\n')
        .map(|offset| absolute_start + offset)
        .unwrap_or(lock.len());
    let old_line = lock[absolute_start..line_end].to_owned();
    let new_line = format!(
        "source = \"git+{repository}?rev={desired_rev}#{desired_rev}\""
    );
    if old_line == new_line {
        return Err(format!("{package} source already equals desired revision; expected one source transition"));
    }
    lock.replace_range(absolute_start..line_end, &new_line);
    Ok(())
}

fn remove_api_direct_sea_orm(lock: &mut String, manifest: &str) -> Result<(), String> {
    if direct_dependency_present(manifest, "sea-orm") {
        return Err("Cargo.toml still declares direct sea-orm; refusing opaque-boundary lock derivation".into());
    }
    let (start, end) = package_bounds(lock, API_NAME)?;
    let block = lock[start..end].to_owned();
    let needle = " \"sea-orm\",\n";
    if block.matches(needle).count() != 1 {
        return Err("canonical-api-server lock entry must contain exactly one direct sea-orm edge before derivation".into());
    }
    let updated = block.replacen(needle, "", 1);
    lock.replace_range(start..end, &updated);
    Ok(())
}

fn require_source(lock: &str, package: &str, repository: &str, rev: &str) -> Result<(), String> {
    let (start, end) = package_bounds(lock, package)?;
    let expected = format!("source = \"git+{repository}?rev={rev}#{rev}\"");
    if !lock[start..end].contains(&expected) {
        return Err(format!("derived lock does not contain expected {package} source"));
    }
    Ok(())
}

fn run(manifest_path: &Path, lock_path: &Path, output_path: &Path) -> Result<(), String> {
    let manifest = fs::read_to_string(manifest_path).map_err(|error| error.to_string())?;
    let original = fs::read_to_string(lock_path).map_err(|error| error.to_string())?;
    let lib_rev = dependency_rev(&manifest, LIB_NAME)?;
    let orm_rev = dependency_rev(&manifest, ORM_NAME)?;

    let mut derived = original.clone();
    replace_git_source(
        &mut derived,
        LIB_NAME,
        "https://github.com/canonical-cloud/canonical-lib-core",
        &lib_rev,
    )?;
    replace_git_source(
        &mut derived,
        ORM_NAME,
        "https://github.com/canonical-cloud/canonical-orm-core",
        &orm_rev,
    )?;
    remove_api_direct_sea_orm(&mut derived, &manifest)?;

    require_source(
        &derived,
        LIB_NAME,
        "https://github.com/canonical-cloud/canonical-lib-core",
        &lib_rev,
    )?;
    require_source(
        &derived,
        ORM_NAME,
        "https://github.com/canonical-cloud/canonical-orm-core",
        &orm_rev,
    )?;
    let (api_start, api_end) = package_bounds(&derived, API_NAME)?;
    if derived[api_start..api_end].contains(" \"sea-orm\",\n") {
        return Err("derived API package still has a direct sea-orm edge".into());
    }

    // The current persistence transition is intentionally constrained to three
    // textual changes: two immutable git source lines and one direct dependency
    // edge. This count makes the witness fail closed if either dependency graph
    // grows or Cargo.lock structure drifts.
    let original_lines: Vec<_> = original.lines().collect();
    let derived_lines: Vec<_> = derived.lines().collect();
    let changed_or_removed = original_lines
        .iter()
        .filter(|line| !derived_lines.contains(line))
        .count();
    let changed_or_added = derived_lines
        .iter()
        .filter(|line| !original_lines.contains(line))
        .count();
    if changed_or_removed != 3 || changed_or_added != 2 {
        return Err(format!(
            "unexpected lock transition shape: removed/changed={changed_or_removed}, added/changed={changed_or_added}"
        ));
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(output_path, derived).map_err(|error| error.to_string())?;
    println!("derived persistence lock: canonical-lib={lib_rev} canonical-orm-core={orm_rev}; direct sea-orm edge removed");
    Ok(())
}

fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 4 {
        eprintln!("usage: derive-persistence-lock <Cargo.toml> <Cargo.lock> <output>");
        std::process::exit(2);
    }
    if let Err(error) = run(Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3])) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir.join("../..");
    let cargo_lock = workspace_root.join("Cargo.lock");

    println!("cargo:rerun-if-changed={}", cargo_lock.display());

    let Ok(lockfile) = fs::read_to_string(&cargo_lock) else {
        return;
    };

    let mut current_package = None::<String>;
    for line in lockfile.lines() {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            current_package = None;
            continue;
        }

        if let Some(name) = trimmed.strip_prefix("name = ") {
            current_package = Some(unquote(name));
            continue;
        }

        if let Some(source) = trimmed.strip_prefix("source = ") {
            let source = unquote(source);
            if source.contains("github.com/zed-industries/zed") {
                let package = current_package
                    .clone()
                    .unwrap_or_else(|| "<unknown>".to_string());
                panic!(
                    "Detected forbidden upstream Zed dependency `{package}` from `{source}`. \
Only the vendored `third_party/gpui` is allowed in this workspace."
                );
            }
        }
    }
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

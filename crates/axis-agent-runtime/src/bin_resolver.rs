use std::env;
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const COMMON_UNIX_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

pub fn provider_base_argv_from_env_or_default(env_name: &str, default_binary: &str) -> Vec<String> {
    vec![provider_bin_override(env_name).unwrap_or_else(|| {
        resolve_binary(default_binary).unwrap_or_else(|| default_binary.to_string())
    })]
}

fn provider_bin_override(env_name: &str) -> Option<String> {
    env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_binary(binary: &str) -> Option<String> {
    let fallback_dirs = fallback_search_dirs();
    resolve_binary_from_path_and_dirs(binary, env::var_os("PATH").as_deref(), &fallback_dirs)
        .map(|path| path.to_string_lossy().into_owned())
}

fn resolve_binary_from_path_and_dirs(
    binary: &str,
    path_env: Option<&OsStr>,
    fallback_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if binary.trim().is_empty() {
        return None;
    }

    let binary_path = Path::new(binary);
    if binary_path.components().count() > 1 {
        return is_executable(binary_path).then(|| binary_path.to_path_buf());
    }

    for dir in path_dirs(path_env)
        .into_iter()
        .chain(fallback_dirs.iter().cloned())
    {
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn path_dirs(path_env: Option<&OsStr>) -> Vec<PathBuf> {
    env::split_paths(path_env.unwrap_or_else(|| OsStr::new(""))).collect()
}

fn fallback_search_dirs() -> Vec<PathBuf> {
    let mut dirs = COMMON_UNIX_BIN_DIRS
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{provider_base_argv_from_env_or_default, resolve_binary_from_path_and_dirs};
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    #[test]
    fn provider_base_argv_prefers_env_override() {
        let env_name = "AXIS_TEST_PROVIDER_BIN_OVERRIDE";
        let _guard = EnvVarGuard::set(env_name, Some("/tmp/codex-demo"));

        assert_eq!(
            provider_base_argv_from_env_or_default(env_name, "codex"),
            vec!["/tmp/codex-demo".to_string()]
        );
    }

    #[test]
    fn resolve_binary_uses_path_entries() {
        let dir = temp_dir("path");
        let expected = create_executable(&dir, "codex");
        let path_env = std::env::join_paths([dir.as_path()]).expect("path should join");

        let resolved = resolve_binary_from_path_and_dirs("codex", Some(path_env.as_os_str()), &[])
            .expect("binary should resolve from PATH");

        assert_eq!(resolved, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_binary_uses_fallback_dirs_when_path_misses_tool() {
        let dir = temp_dir("fallback");
        let expected = create_executable(&dir, "codex");
        let empty_path = OsString::new();

        let resolved = resolve_binary_from_path_and_dirs(
            "codex",
            Some(empty_path.as_os_str()),
            std::slice::from_ref(&dir),
        )
        .expect("binary should resolve from fallback dirs");

        assert_eq!(resolved, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = format!(
            "axis-bin-resolver-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be available")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn create_executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("script should be written");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&path)
                .expect("metadata should load")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("permissions should be set");
        }
        path
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

//! Shared user-level daemon path helpers.

use std::path::PathBuf;

pub const AXIS_SOCKET_PATH_ENV: &str = "AXIS_SOCKET_PATH";
pub const AXIS_DAEMON_DATA_DIR_ENV: &str = "AXIS_DAEMON_DATA_DIR";

const PRODUCT_DIR: &str = "axis";
const DAEMON_SOCKET_FILE: &str = "axisd.sock";

pub fn axis_user_data_dir() -> PathBuf {
    axis_user_data_dir_for(
        std::env::var_os(AXIS_DAEMON_DATA_DIR_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        home_dir(),
        std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    )
}

pub fn axis_user_data_dir_for(
    explicit_override: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
) -> PathBuf {
    if let Some(path) = explicit_override {
        return path;
    }

    #[cfg(target_os = "macos")]
    if let Some(home_dir) = home_dir.clone() {
        return home_dir
            .join("Library")
            .join("Application Support")
            .join(PRODUCT_DIR);
    }

    if let Some(xdg_data_home) = xdg_data_home {
        return xdg_data_home.join(PRODUCT_DIR);
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".local").join("share").join(PRODUCT_DIR);
    }

    PathBuf::from(format!(".{PRODUCT_DIR}"))
}

pub fn daemon_socket_path() -> PathBuf {
    daemon_socket_path_for(
        std::env::var_os(AXIS_SOCKET_PATH_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        axis_user_data_dir(),
    )
}

pub fn daemon_socket_path_for(explicit_socket_override: Option<PathBuf>, data_dir: PathBuf) -> PathBuf {
    explicit_socket_override.unwrap_or_else(|| data_dir.join(DAEMON_SOCKET_FILE))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

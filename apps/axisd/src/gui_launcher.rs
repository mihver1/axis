use anyhow::Context;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const AXIS_APP_BIN_ENV: &str = "AXIS_APP_BIN";

#[derive(Clone, Debug)]
pub struct GuiLauncher {
    app_bin: PathBuf,
}

impl GuiLauncher {
    pub fn from_env() -> Self {
        let app_bin = std::env::var_os(AXIS_APP_BIN_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("axis-app"));
        Self { app_bin }
    }

    pub fn app_bin(&self) -> &Path {
        &self.app_bin
    }

    pub fn launch(&self, workspace_root: &str) -> anyhow::Result<()> {
        Command::new(&self.app_bin)
            .env("AXIS_WORKSPACE_ROOT", workspace_root)
            .spawn()
            .with_context(|| {
                format!(
                    "launch gui with {} for workspace {}",
                    self.app_bin.display(),
                    workspace_root
                )
            })?;
        Ok(())
    }
}

impl Default for GuiLauncher {
    fn default() -> Self {
        Self::from_env()
    }
}

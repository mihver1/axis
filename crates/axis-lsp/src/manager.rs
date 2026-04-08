use std::collections::HashMap;
use std::process::{Child, Command, Stdio};

#[derive(Clone, Debug)]
pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub extensions: Vec<String>,
}

pub struct LspManager {
    configs: HashMap<String, LspServerConfig>,
    running: HashMap<String, Child>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            running: HashMap::new(),
        }
    }

    pub fn register(&mut self, language: &str, config: LspServerConfig) {
        self.configs.insert(language.to_string(), config);
    }

    /// Returns the language key whose config lists the given file extension.
    pub fn language_for_extension(&self, ext: &str) -> Option<String> {
        self.configs
            .iter()
            .find(|(_, cfg)| cfg.extensions.iter().any(|e| e == ext))
            .map(|(lang, _)| lang.clone())
    }

    /// Starts the language server for `language` if it is not already running.
    pub fn ensure_server(&mut self, language: &str) -> anyhow::Result<()> {
        if self.running.contains_key(language) {
            return Ok(());
        }

        let config = self
            .configs
            .get(language)
            .ok_or_else(|| anyhow::anyhow!("no LSP config registered for language: {language}"))?
            .clone();

        let child = Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to spawn LSP server '{}': {e}",
                    config.command
                )
            })?;

        self.running.insert(language.to_string(), child);
        Ok(())
    }

    /// Terminates the language server for `language` if it is running.
    pub fn stop_server(&mut self, language: &str) {
        if let Some(mut child) = self.running.remove(language) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Terminates all running language servers.
    pub fn stop_all(&mut self) {
        let languages: Vec<String> = self.running.keys().cloned().collect();
        for lang in languages {
            self.stop_server(&lang);
        }
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_config() -> LspServerConfig {
        LspServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            extensions: vec!["rs".to_string()],
        }
    }

    fn ts_config() -> LspServerConfig {
        LspServerConfig {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            extensions: vec!["ts".to_string(), "tsx".to_string()],
        }
    }

    #[test]
    fn register_and_find_language() {
        let mut manager = LspManager::new();
        manager.register("rust", rust_config());
        manager.register("typescript", ts_config());

        assert_eq!(
            manager.language_for_extension("rs"),
            Some("rust".to_string())
        );
        assert_eq!(
            manager.language_for_extension("ts"),
            Some("typescript".to_string())
        );
        assert_eq!(
            manager.language_for_extension("tsx"),
            Some("typescript".to_string())
        );
        assert_eq!(manager.language_for_extension("py"), None);
    }

    #[test]
    fn ensure_server_returns_error_for_unknown_language() {
        let mut manager = LspManager::new();
        let result = manager.ensure_server("unknown");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown"), "unexpected error: {msg}");
    }

    #[test]
    fn stop_server_is_noop_when_not_running() {
        let mut manager = LspManager::new();
        manager.register("rust", rust_config());
        // Should not panic
        manager.stop_server("rust");
    }
}

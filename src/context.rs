use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct PluginContext {
    pub focused_pane_cwd: Option<PathBuf>,
    pub workspace_cwd: Option<PathBuf>,
}

impl PluginContext {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn current_cwd(&self) -> Option<&Path> {
        self.focused_pane_cwd
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
            .or(self.workspace_cwd.as_deref())
            .filter(|path| !path.as_os_str().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_pane_cwd_takes_precedence_and_unknown_fields_are_ignored() {
        let context = PluginContext::from_json(
            r#"{
                "workspace_cwd": "/workspace",
                "focused_pane_cwd": "/focused",
                "workspace_id": "w_1",
                "future_field": true
            }"#,
        )
        .unwrap();
        assert_eq!(context.current_cwd(), Some(Path::new("/focused")));
    }

    #[test]
    fn workspace_cwd_is_the_fallback() {
        let context = PluginContext::from_json(r#"{"workspace_cwd":"/workspace"}"#).unwrap();
        assert_eq!(context.current_cwd(), Some(Path::new("/workspace")));
    }

    #[test]
    fn empty_focused_pane_cwd_falls_back_to_workspace() {
        let context =
            PluginContext::from_json(r#"{"focused_pane_cwd":"","workspace_cwd":"/workspace"}"#)
                .unwrap();
        assert_eq!(context.current_cwd(), Some(Path::new("/workspace")));
    }

    #[test]
    fn invalid_context_is_reported() {
        assert!(PluginContext::from_json("not json").is_err());
    }
}

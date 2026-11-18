//! Local patch: the dock, a pane area at the bottom of the screen that belongs
//! to no workspace. This module holds only its config: the seed the server
//! builds a dock from when no saved dock exists.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Deserialize;

/// Content rows of a freshly seeded dock. Three fits hayami's `row` layout.
pub const DEFAULT_DOCK_ROWS: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DockConfig {
    /// Height of a seeded dock in content rows; a rule row is drawn above it.
    pub rows: u16,
    /// Seed layout: one row of panes, left to right.
    pub panes: Vec<DockPaneConfig>,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            rows: DEFAULT_DOCK_ROWS,
            panes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DockPaneConfig {
    /// Names the pane; unique within the dock.
    pub label: String,
    /// Shell command the pane runs, respawned if it exits.
    pub command: String,
    /// Working directory. Default: the home directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Relative share of the dock width. Default: 1.
    #[serde(default = "default_width")]
    pub width: u16,
}

fn default_width() -> u16 {
    1
}

impl DockConfig {
    pub fn diagnostics(&self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if self.rows == 0 {
            diagnostics
                .push("ui.dock.rows must be greater than zero; ignoring the dock seed".into());
        }
        let mut labels = HashSet::new();
        for (index, pane) in self.panes.iter().enumerate() {
            let label = pane.label.trim();
            if label.is_empty() {
                diagnostics.push(format!(
                    "ui.dock.panes[{index}].label is empty; ignoring the dock seed"
                ));
            } else if !labels.insert(label) {
                diagnostics.push(format!(
                    "ui.dock.panes label {label:?} is used more than once; ignoring the dock seed"
                ));
            }
            if pane.command.trim().is_empty() {
                diagnostics.push(format!(
                    "ui.dock.panes[{index}].command is empty; ignoring the dock seed"
                ));
            }
            if pane.width == 0 {
                diagnostics.push(format!(
                    "ui.dock.panes[{index}].width must be greater than zero; ignoring the dock seed"
                ));
            }
        }
        diagnostics
    }

    /// The seed to build a dock from: `None` when no panes are configured or
    /// the config is invalid, so a broken seed never produces a half-built dock.
    pub fn seed(&self) -> Option<&Self> {
        (!self.panes.is_empty() && self.diagnostics().is_empty()).then_some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> DockConfig {
        #[derive(Deserialize)]
        struct Wrapper {
            dock: DockConfig,
        }
        toml::from_str::<Wrapper>(toml_text).unwrap().dock
    }

    #[test]
    fn defaults_to_no_dock_and_three_rows() {
        let dock = DockConfig::default();
        assert_eq!(dock.rows, 3);
        assert!(dock.panes.is_empty());
        assert!(dock.seed().is_none(), "no panes means no dock");
        assert!(dock.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_seed_with_optional_cwd_and_width() {
        let dock = parse(
            r#"
[dock]
rows = 4

[[dock.panes]]
label = "usage"
command = "hayami-tui --sections usage --arrangement row"
cwd = "~"
width = 3

[[dock.panes]]
label = "net"
command = "htop"
"#,
        );
        assert_eq!(dock.rows, 4);
        assert_eq!(dock.panes.len(), 2);
        assert_eq!(dock.panes[0].cwd, Some(PathBuf::from("~")));
        assert_eq!(dock.panes[0].width, 3);
        assert_eq!(dock.panes[1].cwd, None);
        assert_eq!(dock.panes[1].width, 1, "width defaults to 1");
        assert!(dock.seed().is_some());
    }

    #[test]
    fn invalid_seeds_are_reported_and_not_used() {
        let cases = [
            ("[dock]\nrows = 0\n[[dock.panes]]\nlabel = \"a\"\ncommand = \"x\"\n", "rows"),
            ("[[dock.panes]]\nlabel = \"\"\ncommand = \"x\"\n", "label is empty"),
            (
                "[[dock.panes]]\nlabel = \"a\"\ncommand = \"x\"\n[[dock.panes]]\nlabel = \"a\"\ncommand = \"y\"\n",
                "more than once",
            ),
            ("[[dock.panes]]\nlabel = \"a\"\ncommand = \"  \"\n", "command is empty"),
            ("[[dock.panes]]\nlabel = \"a\"\ncommand = \"x\"\nwidth = 0\n", "width"),
        ];
        for (text, expected) in cases {
            let dock = parse(&format!("[dock]\n{}", text.trim_start_matches("[dock]\n")));
            let diagnostics = dock.diagnostics();
            assert!(
                diagnostics.iter().any(|d| d.contains(expected)),
                "{expected:?} not in {diagnostics:?}"
            );
            assert!(
                dock.seed().is_none(),
                "an invalid seed must not build a dock"
            );
        }
    }
}

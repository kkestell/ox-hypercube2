//! Ox's config, from `config.json` in Ox's config directory.
//!
//! `harnesses` lists the harnesses, models, effort levels, and modes Ox
//! offers: an object keyed by each harness's executable name, whose value
//! lists `models`, each with `id`, `name`, an optional `provider`, and
//! `efforts`, and `modes`. Effort levels and modes are `{ "id", "name" }`
//! objects. Order is the order pickers offer them in.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::messages::Harness;

/// Ox's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The harnesses Ox offers, in [`Harness::ALL`] order.
    pub harnesses: Vec<HarnessConfig>,
}

/// What Ox offers for one harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub harness: Harness,
    pub models: Vec<ModelConfig>,
    pub modes: Vec<Choice>,
}

/// A model Ox offers, with the effort levels it offers it at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// The value the harness's `model` option takes.
    pub id: String,
    pub name: String,
    /// The name of the provider that serves the model, shown beside it.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub efforts: Vec<Choice>,
}

/// An effort level or mode: the value its option takes, and its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// The file's contents.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    harnesses: BTreeMap<String, HarnessEntry>,
}

/// A harness's entry in `harnesses`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessEntry {
    #[serde(default)]
    models: Vec<ModelConfig>,
    #[serde(default)]
    modes: Vec<Choice>,
}

/// Ox's config directory: `$XDG_CONFIG_HOME/ox` when `XDG_CONFIG_HOME` is
/// an absolute path, else `~/.config/ox`, else `<temp dir>/ox` when there is
/// no home directory.
pub fn config_dir() -> PathBuf {
    config_dir_from(std::env::var_os("XDG_CONFIG_HOME"), dirs::home_dir())
}

fn config_dir_from(xdg: Option<OsString>, home: Option<PathBuf>) -> PathBuf {
    let base = match (xdg.map(PathBuf::from), home) {
        (Some(xdg), _) if xdg.is_absolute() => xdg,
        (_, Some(home)) => home.join(".config"),
        (_, None) => std::env::temp_dir(),
    };
    base.join("ox")
}

/// The config file's path.
pub fn path() -> PathBuf {
    config_dir().join("config.json")
}

/// Ox's config. A missing or unparsable file, or a harness entry that names
/// no harness, is an error naming the file.
pub fn load() -> Result<Config, String> {
    load_from(&path())
}

fn load_from(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn parse(text: &str) -> Result<Config, String> {
    let File { mut harnesses } = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if let Some(name) = harnesses
        .keys()
        .find(|name| !Harness::ALL.iter().any(|h| h.executable() == *name))
    {
        let known: Vec<&str> = Harness::ALL.iter().map(|h| h.executable()).collect();
        return Err(format!(
            "{name} is not a harness; the harnesses are {}",
            known.join(", ")
        ));
    }
    Ok(Config {
        harnesses: Harness::ALL
            .into_iter()
            .filter_map(|harness| {
                let entry = harnesses.remove(harness.executable())?;
                Some(HarnessConfig {
                    harness,
                    models: entry.models,
                    modes: entry.modes,
                })
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: &str, name: &str) -> Choice {
        Choice {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn load_reads_harnesses_in_harness_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "harnesses": {
    "pi": {
      "models": [
        {
          "id": "openrouter/deepseek/deepseek-v4.1-flash-1017",
          "name": "DeepSeek V4.1 Flash",
          "provider": "OpenRouter",
          "efforts": [{ "id": "low", "name": "Low" }]
        }
      ]
    },
    "claude": {
      "modes": [{ "id": "manual", "name": "Manual" }, { "id": "auto", "name": "Auto" }],
      "models": [
        {
          "id": "claude-opus-5-5",
          "name": "Opus 5.5",
          "efforts": [{ "id": "medium", "name": "Medium" }, { "id": "high", "name": "High" }]
        },
        { "id": "claude-haiku-5-5", "name": "Haiku 5.5" }
      ]
    }
  }
}"#,
        )
        .unwrap();
        assert_eq!(
            load_from(&path),
            Ok(Config {
                harnesses: vec![
                    HarnessConfig {
                        harness: Harness::Claude,
                        models: vec![
                            ModelConfig {
                                id: "claude-opus-5-5".into(),
                                name: "Opus 5.5".into(),
                                provider: None,
                                efforts: vec![choice("medium", "Medium"), choice("high", "High")],
                            },
                            ModelConfig {
                                id: "claude-haiku-5-5".into(),
                                name: "Haiku 5.5".into(),
                                provider: None,
                                efforts: Vec::new(),
                            },
                        ],
                        modes: vec![choice("manual", "Manual"), choice("auto", "Auto")],
                    },
                    HarnessConfig {
                        harness: Harness::Pi,
                        models: vec![ModelConfig {
                            id: "openrouter/deepseek/deepseek-v4.1-flash-1017".into(),
                            name: "DeepSeek V4.1 Flash".into(),
                            provider: Some("OpenRouter".into()),
                            efforts: vec![choice("low", "Low")],
                        }],
                        modes: Vec::new(),
                    },
                ],
            })
        );
    }

    #[test]
    fn load_refuses_a_bad_file_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for (text, error) in [
            (
                r#"{ "harnesses": { "codex": { "models": [] } } }"#,
                "codex is not a harness; the harnesses are claude, pi",
            ),
            (r#"{ "harnesses": { "claude": "#, "EOF while parsing"),
            ("{}", "missing field `harnesses`"),
        ] {
            std::fs::write(&path, text).unwrap();
            let message = load_from(&path).unwrap_err();
            assert!(
                message.starts_with(&format!("{}: ", path.display())) && message.contains(error),
                "{text:?}: {message}"
            );
        }
    }

    #[test]
    fn load_of_a_missing_file_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let message = load_from(&path).unwrap_err();
        assert!(
            message.starts_with(&format!("reading {}: ", path.display())),
            "{message}"
        );
    }

    #[test]
    fn the_example_config_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/config.json");
        let config = load_from(&path).unwrap();
        assert_eq!(
            config
                .harnesses
                .iter()
                .map(|h| h.harness)
                .collect::<Vec<_>>(),
            Harness::ALL
        );
    }

    fn home() -> PathBuf {
        PathBuf::from("/home/user")
    }

    #[test]
    fn config_dir_is_under_an_absolute_xdg_config_home() {
        assert_eq!(
            config_dir_from(Some("/config".into()), Some(home())),
            Path::new("/config/ox")
        );
    }

    #[test]
    fn config_dir_ignores_a_relative_or_empty_xdg_config_home() {
        for xdg in [Some("config".into()), Some("".into()), None] {
            assert_eq!(
                config_dir_from(xdg, Some(home())),
                Path::new("/home/user/.config/ox")
            );
        }
    }

    #[test]
    fn config_dir_without_a_home_is_in_the_temp_dir() {
        assert_eq!(config_dir_from(None, None), std::env::temp_dir().join("ox"));
    }
}

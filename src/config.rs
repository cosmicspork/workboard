//! Checks on what Workboard is configured with, and the per-channel
//! `plugin.toml` read in plugin mode.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

/// A Jira site: an HTTPS origin without credentials, path, query or fragment.
pub fn site(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("invalid Jira site URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Jira site must be an HTTPS origin without credentials, path, query, or fragment");
    }
    Ok(url)
}

/// A jira-review profile: 1-64 letters, digits, `_`, `-` or `.`, which both
/// a Tracon binding name (`jira-<profile>`) and a keyring user can hold.
pub fn profile(raw: &str) -> Result<String> {
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("Jira profile must be 1-64 letters, digits, '_', '-' or '.'");
    }
    Ok(raw.to_owned())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPluginConfig {
    jira_site: Option<String>,
    jira_profile: Option<String>,
    /// No longer read: a refresh goes through the node socket. Accepted so a
    /// `plugin.toml` written for 0.3 still starts.
    #[allow(dead_code)]
    tracon_url: Option<toml::Value>,
}

/// `<data>/plugin.toml`: the one place a node gives a plugin instance its own
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConfig {
    pub jira_site: Option<Url>,
    /// Passed to jira-review's `search` as `profile`, so the node lends the
    /// channel's `jira-<profile>` binding instead of `jira`.
    pub jira_profile: Option<String>,
}

impl PluginConfig {
    /// Read `path` when it exists; a missing file is the default
    /// configuration, while an unreadable or invalid one is an error.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<RawPluginConfig>(&text)
                .with_context(|| format!("reading {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RawPluginConfig::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let jira_site = raw
            .jira_site
            .as_deref()
            .map(site)
            .transpose()
            .with_context(|| format!("jira_site in {}", path.display()))?;
        let jira_profile = raw
            .jira_profile
            .as_deref()
            .map(profile)
            .transpose()
            .with_context(|| format!("jira_profile in {}", path.display()))?;
        Ok(Self {
            jira_site,
            jira_profile,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_site_is_an_origin() {
        assert!(site("https://example.atlassian.net").is_ok());
        for raw in [
            "http://example.atlassian.net",
            "https://user@example.atlassian.net",
            "https://example.atlassian.net/path",
            "https://example.atlassian.net/?a=1",
        ] {
            assert!(site(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn a_profile_is_a_binding_suffix() {
        for raw in ["work", "client-x", "a_b.c", &"p".repeat(64)] {
            assert_eq!(profile(raw).unwrap(), raw);
        }
        for raw in ["", "two words", "a/b", "jira*", &"p".repeat(65)] {
            assert!(profile(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn plugin_config_is_optional_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.toml");
        let config = PluginConfig::load(&path).unwrap();
        assert_eq!(config.jira_site, None);
        assert_eq!(config.jira_profile, None);

        std::fs::write(
            &path,
            "jira_site = \"https://example.atlassian.net\"\njira_profile = \"work\"\n",
        )
        .unwrap();
        let config = PluginConfig::load(&path).unwrap();
        assert_eq!(
            config.jira_site.unwrap().as_str(),
            "https://example.atlassian.net/"
        );
        assert_eq!(config.jira_profile.as_deref(), Some("work"));

        // 0.3's tracon_url is ignored rather than refused.
        std::fs::write(&path, "tracon_url = \"http://127.0.0.1:7420\"\n").unwrap();
        assert_eq!(PluginConfig::load(&path).unwrap().jira_profile, None);

        for bad in [
            "jira_site = \"http://example.atlassian.net\"\n",
            "jira_profile = \"\"\n",
            "jira_profile = \"a b\"\n",
            "jira_channel = \"work\"\n",
            "jira_site = [\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            let err = format!("{:#}", PluginConfig::load(&path).unwrap_err());
            assert!(err.contains("plugin.toml"), "{bad}: {err}");
        }
    }
}

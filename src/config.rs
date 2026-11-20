//! Checks on the URLs Workboard is configured with, and the per-channel
//! `plugin.toml` read in plugin mode.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

/// Where Tracon listens unless told otherwise.
pub const DEFAULT_TRACON_URL: &str = "http://127.0.0.1:7420";

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

/// A Tracon node: a plain `http://` loopback origin.
pub fn tracon(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("invalid Tracon URL")?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "http"
        || !loopback
        || url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Tracon URL must be a plain http:// loopback origin, such as http://127.0.0.1:7420");
    }
    Ok(url)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPluginConfig {
    jira_site: Option<String>,
    tracon_url: Option<String>,
}

/// `<data>/plugin.toml`: the one place a node gives a plugin instance its own
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConfig {
    pub jira_site: Option<Url>,
    pub tracon_url: Url,
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
        let tracon_url = tracon(raw.tracon_url.as_deref().unwrap_or(DEFAULT_TRACON_URL))
            .with_context(|| format!("tracon_url in {}", path.display()))?;
        Ok(Self {
            jira_site,
            tracon_url,
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
    fn tracon_url_is_a_loopback_origin() {
        for raw in [
            "http://127.0.0.1:7420",
            "http://localhost:7420",
            "http://[::1]:7420",
        ] {
            assert!(tracon(raw).is_ok(), "{raw}");
        }
        for raw in [
            "https://127.0.0.1:7420",
            "http://example.com:7420",
            "http://127.0.0.1:7420/api",
            "http://user@127.0.0.1:7420",
        ] {
            assert!(tracon(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn plugin_config_is_optional_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.toml");
        let config = PluginConfig::load(&path).unwrap();
        assert_eq!(config.jira_site, None);
        assert_eq!(config.tracon_url.as_str(), "http://127.0.0.1:7420/");

        std::fs::write(
            &path,
            "jira_site = \"https://example.atlassian.net\"\ntracon_url = \"http://localhost:7421\"\n",
        )
        .unwrap();
        let config = PluginConfig::load(&path).unwrap();
        assert_eq!(
            config.jira_site.unwrap().as_str(),
            "https://example.atlassian.net/"
        );
        assert_eq!(config.tracon_url.as_str(), "http://localhost:7421/");

        for bad in [
            "jira_site = \"http://example.atlassian.net\"\n",
            "tracon_url = \"http://example.com:7420\"\n",
            "jira_channel = \"work\"\n",
            "jira_site = [\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            let err = format!("{:#}", PluginConfig::load(&path).unwrap_err());
            assert!(err.contains("plugin.toml"), "{bad}: {err}");
        }
    }
}

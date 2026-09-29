use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use url::Url;
use workboard::{
    http::{self, App},
    store::Store,
};

#[derive(Parser)]
#[command(name = "workboard")]
struct Cli {
    #[arg(long, global = true, env = "WORKBOARD_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value = "acli")]
    acli: String,
    #[arg(long, global = true, default_value = "https://example.atlassian.net")]
    jira_site: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value_t = 4510)]
        port: u16,
    },
}

fn site(raw: &str) -> Result<Url> {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let jira_site = site(&cli.jira_site)?;
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => PathBuf::from(
            std::env::var_os("HOME").context("HOME is required unless --data-dir is set")?,
        )
        .join(".local/share/workboard"),
    };
    match cli.command {
        Command::Serve { port } => {
            let store = Arc::new(Store::init(&data_dir, &jira_site)?);
            let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .context("binding local workboard port")?;
            let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
            let app = App {
                store,
                site: jira_site,
                acli: cli.acli,
                origin: origin.clone(),
                refresh_in_progress: Arc::new(AtomicBool::new(false)),
            };
            println!(
                "Workboard: http://localhost:{}",
                listener.local_addr()?.port()
            );
            axum::serve(listener, http::router(app))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .context("serving workboard")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::site;
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
}

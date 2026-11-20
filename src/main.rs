use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use workboard::{
    config::{self, site, tracon},
    http::{self, App, JiraSite, Local},
    plugin,
    store::Store,
};

#[derive(Parser)]
#[command(name = "workboard")]
struct Cli {
    #[arg(long, global = true, env = "WORKBOARD_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value = config::DEFAULT_TRACON_URL)]
    tracon_url: String,
    #[arg(long, global = true, default_value = "work")]
    jira_channel: String,
    #[arg(long, global = true, env = "WORKBOARD_JIRA_SITE")]
    jira_site: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the board on a loopback port, or as a Tracon plugin when the node
    /// passes `--socket` and `--data`.
    Serve {
        #[arg(long, default_value_t = 4510)]
        port: u16,
        /// Plugin mode: the Unix socket the node names. Configuration comes from
        /// `<data>/plugin.toml`, not the standalone options.
        #[arg(long, requires = "data", conflicts_with = "port")]
        socket: Option<PathBuf>,
        /// Plugin mode: this instance's data directory.
        #[arg(long, requires = "socket")]
        data: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Serve {
        socket: Some(socket),
        data: Some(data),
        ..
    } = &cli.command
    {
        return plugin::run(socket, data).await;
    }
    let jira_site = site(
        cli.jira_site
            .as_deref()
            .context("--jira-site or WORKBOARD_JIRA_SITE is required")?,
    )?;
    let tracon_url = tracon(&cli.tracon_url)?;
    if cli.jira_channel.trim().is_empty() {
        bail!("Jira channel must not be empty");
    }
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => PathBuf::from(
            std::env::var_os("HOME").context("HOME is required unless --data-dir is set")?,
        )
        .join(".local/share/workboard"),
    };
    match cli.command {
        Command::Serve { port, .. } => {
            let store = Arc::new(Store::init(&data_dir, &jira_site)?);
            let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .context("binding local workboard port")?;
            let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
            let app = App {
                store,
                site: JiraSite::Configured(jira_site),
                tracon: tracon_url,
                refresh_in_progress: Arc::new(AtomicBool::new(false)),
            };
            let local = Local {
                origin,
                jira_channel: cli.jira_channel,
            };
            println!(
                "Workboard: http://localhost:{}",
                listener.local_addr()?.port()
            );
            axum::serve(listener, http::router(app, local))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .context("serving workboard")?;
        }
    }
    Ok(())
}

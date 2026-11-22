use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use workboard::{
    config::{self, site},
    http::{self, App, JiraSite, Local},
    jira, mcp, ops, plugin,
    proposals::{Outcome, Proposals, Status},
    stdio,
    store::Store,
};

#[derive(Parser)]
#[command(name = "workboard")]
struct Cli {
    #[arg(long, global = true, env = "WORKBOARD_DATA_DIR")]
    data_dir: Option<PathBuf>,
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
        /// Plugin mode: the node's socket for calling it back, where a refresh
        /// calls jira-review's `search`.
        #[arg(long, requires = "socket")]
        node_socket: Option<PathBuf>,
        /// The jira-review profile a refresh reads Jira with: its keyring
        /// entry. Unset, jira-review uses its own default.
        #[arg(long, env = "WORKBOARD_JIRA_PROFILE", conflicts_with = "socket")]
        jira_profile: Option<String>,
        /// The jira-review program a refresh starts as `jira-review mcp`.
        #[arg(
            long,
            env = "WORKBOARD_JIRA_REVIEW",
            default_value = "jira-review",
            conflicts_with = "socket"
        )]
        jira_review: PathBuf,
    },
    /// An MCP server on stdin and stdout with the board's tools, for any agent
    /// client. Reads answer; writes wait as proposals for `workboard review`.
    Mcp,
    /// Run one board operation as yourself and print its result: the same
    /// operations as the tools and the HTTP API. With no name, list them.
    Op {
        name: Option<String>,
        /// The arguments, as a JSON object.
        #[arg(default_value = "{}")]
        arguments: String,
    },
    /// Review the writes `workboard mcp` proposed. With no action, list them.
    Review {
        #[command(subcommand)]
        action: Option<Review>,
    },
}

#[derive(Subcommand)]
enum Review {
    /// What is waiting, oldest first.
    List,
    /// A proposal's change, and whether the board has moved since.
    Show { id: String },
    /// Apply a proposal, if the board is still as it was previewed.
    Approve { id: String },
    /// Turn a proposal down.
    Reject {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Serve {
        socket: Some(socket),
        data: Some(data),
        node_socket,
        ..
    } = &cli.command
    {
        return plugin::run(socket, data, node_socket.as_deref()).await;
    }
    let data_dir = match cli.data_dir.clone() {
        Some(dir) => dir,
        None => PathBuf::from(
            std::env::var_os("HOME").context("HOME is required unless --data-dir is set")?,
        )
        .join(".local/share/workboard"),
    };
    let optional_site = cli.jira_site.as_deref().map(site).transpose()?;
    match cli.command {
        Command::Mcp => {
            let host = mcp::Standalone {
                store: Arc::new(Store::open(&data_dir, optional_site.as_ref())?),
                proposals: Proposals::open(data_dir.join("proposals"))
                    .map_err(anyhow::Error::msg)?,
            };
            stdio::run(Arc::new(host))
                .await
                .context("serving MCP on stdio")?;
            return Ok(());
        }
        Command::Op { name, arguments } => {
            let Some(name) = name else {
                for op in ops::OPS {
                    let kind = if op.write { "write" } else { "read" };
                    println!("{:<20} {:<5} {:<6} {}", op.name, kind, op.method, op.path);
                }
                return Ok(());
            };
            let args: Value =
                serde_json::from_str(&arguments).context("arguments must be a JSON object")?;
            let store = Store::open(&data_dir, optional_site.as_ref())?;
            let ctx = ops::Ctx {
                store: &store,
                refresh_in_progress: false,
            };
            match ops::run(&ctx, &name, &args) {
                Ok(value) => println!("{}", serde_json::to_string_pretty(&value)?),
                Err(err) => bail!("{}", serde_json::to_string_pretty(&err.body())?),
            }
            return Ok(());
        }
        Command::Review { action } => {
            let store = Store::open(&data_dir, optional_site.as_ref())?;
            let proposals =
                Proposals::open(data_dir.join("proposals")).map_err(anyhow::Error::msg)?;
            return review(&store, &proposals, action.unwrap_or(Review::List));
        }
        Command::Serve { .. } => {}
    }
    let jira_site = optional_site.context("--jira-site or WORKBOARD_JIRA_SITE is required")?;
    match cli.command {
        Command::Serve {
            port,
            jira_profile,
            jira_review,
            ..
        } => {
            let profile = jira_profile
                .as_deref()
                .map(config::profile)
                .transpose()
                .context("--jira-profile")?;
            if jira_review.as_os_str().is_empty() {
                bail!("--jira-review must name a program");
            }
            let store = Arc::new(Store::init(&data_dir, &jira_site)?);
            let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .context("binding local workboard port")?;
            let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
            let app = App {
                store,
                site: JiraSite::Configured(jira_site),
                jira: jira::Source::Child {
                    program: jira_review,
                    profile,
                },
                refresh_in_progress: Arc::new(AtomicBool::new(false)),
            };
            let local = Local { origin };
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
        _ => unreachable!("handled above"),
    }
    Ok(())
}

fn review(store: &Store, proposals: &Proposals, action: Review) -> Result<()> {
    let msg = anyhow::Error::msg;
    match action {
        Review::List => {
            let pending = proposals.pending().map_err(msg)?;
            if pending.is_empty() {
                println!("nothing is waiting for review");
            }
            for p in pending {
                println!("{}  {}  {}", p.id, p.created_at, p.summary);
            }
        }
        Review::Show { id } => {
            let p = proposals
                .get(&id)
                .map_err(msg)?
                .with_context(|| format!("no proposal {id}"))?;
            println!("{}: {}\n\n{}", p.id, p.summary, p.body);
            match &p.outcome {
                Some(o) => println!("decided: {}", json!(o.status)),
                None => {
                    let now = workboard::preview::preview(store, &p.tool, &p.arguments);
                    let current = matches!(&now, Ok(now) if now.token == p.token);
                    println!(
                        "{}",
                        if current {
                            "The board is as it was when this was proposed."
                        } else {
                            "The board has moved since this was proposed; approving it will be refused as stale."
                        }
                    );
                }
            }
        }
        Review::Approve { id } => {
            let p = proposals.claim(&id).map_err(msg)?;
            let outcome = mcp::approve(store, &p);
            let p = proposals.decide(p, outcome).map_err(msg)?;
            println!("{}", serde_json::to_string_pretty(&p.status())?);
        }
        Review::Reject { id, reason } => {
            let p = proposals.claim(&id).map_err(msg)?;
            let p = proposals
                .decide(p, Outcome::new(Status::Rejected, None, reason))
                .map_err(msg)?;
            println!("{}", serde_json::to_string_pretty(&p.status())?);
        }
    }
    Ok(())
}

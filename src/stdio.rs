//! `workboard mcp`: the MCP server on stdin and stdout, one JSON-RPC
//! message per line. Each request is answered as soon as it is done, so a
//! `proposal_status` waiting on a reviewer holds up nothing else.

use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::mcp::{self, Host};

pub async fn run<H: Host + 'static>(host: Arc<H>) -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = rx.recv().await {
            let mut line = msg.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut calls = JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(e) => {
                let _ = tx.send(mcp::parse_error(&e.to_string()));
                continue;
            }
        };
        let (host, tx) = (host.clone(), tx.clone());
        calls.spawn(async move {
            if let Some(answer) = mcp::handle(&*host, msg).await {
                let _ = tx.send(answer);
            }
        });
        // Reap what has finished, so a long session does not hold them all.
        while calls.try_join_next().is_some() {}
    }
    // The client closed stdin: finish what it asked, then stop.
    while calls.join_next().await.is_some() {}
    drop(tx);
    let _ = writer.await;
    Ok(())
}

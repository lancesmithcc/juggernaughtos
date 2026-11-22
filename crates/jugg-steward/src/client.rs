//! App-side client for the steward socket. Never blocks the request path:
//! events go through a bounded queue; if the steward is down they are dropped
//! (the app's in-process correlator still protects it).

use crate::daemon::{Request, Status};
use crate::Event;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct StewardClient {
    socket: PathBuf,
    tx: mpsc::Sender<Event>,
}

impl StewardClient {
    pub fn spawn(socket: PathBuf) -> Self {
        let (tx, mut rx) = mpsc::channel::<Event>(1024);
        let path = socket.clone();
        tokio::spawn(async move {
            let mut conn: Option<tokio::net::UnixStream> = None;
            while let Some(ev) = rx.recv().await {
                for _ in 0..2 {
                    if conn.is_none() {
                        conn = tokio::time::timeout(Duration::from_millis(300), tokio::net::UnixStream::connect(&path)).await.ok().and_then(Result::ok);
                    }
                    let Some(c) = conn.as_mut() else { break };
                    let line = serde_json::to_string(&Request::Event { event: ev.clone() }).unwrap_or_default() + "\n";
                    let ok = c.write_all(line.as_bytes()).await.is_ok() && read_line(c).await.is_some();
                    if ok {
                        break;
                    }
                    conn = None;
                }
            }
        });
        Self { socket, tx }
    }

    pub fn send(&self, ev: Event) {
        let _ = self.tx.try_send(ev);
    }

    /// Ask for current status; `None` if the steward isn't running.
    pub async fn status(&self) -> Option<Status> {
        let mut c = tokio::time::timeout(Duration::from_millis(500), tokio::net::UnixStream::connect(&self.socket)).await.ok()?.ok()?;
        c.write_all(b"{\"op\":\"status\"}\n").await.ok()?;
        let v: serde_json::Value = serde_json::from_str(&read_line(&mut c).await?).ok()?;
        serde_json::from_value(v["status"].clone()).ok()
    }
}

async fn read_line(c: &mut tokio::net::UnixStream) -> Option<String> {
    let mut r = BufReader::new(c);
    let mut s = String::new();
    tokio::time::timeout(Duration::from_secs(2), r.read_line(&mut s)).await.ok()?.ok()?;
    (!s.is_empty()).then_some(s)
}

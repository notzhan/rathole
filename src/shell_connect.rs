/// Shell-connect client: connects to a rathole server's shell service port and
/// provides a fully interactive terminal session.
///
/// Usage example:
///   rathole --shell-connect server.example.com:2222
use anyhow::{Context, Result};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::info;

pub async fn run(addr: &str) -> Result<()> {
    info!("Connecting to shell service at {}", addr);
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("Failed to connect to {}", addr))?;

    // Disable Nagle's algorithm for lower keystroke latency.
    stream
        .set_nodelay(true)
        .with_context(|| "Failed to set TCP_NODELAY")?;

    info!("Connected. Press Ctrl+C or close the connection to exit.");

    // Put the local terminal into raw mode so that all key-strokes (including
    // Ctrl+C, arrow keys, etc.) are forwarded verbatim to the remote shell.
    enable_raw_mode()?;

    let result = forward(stream).await;

    // Always restore the terminal – even if forward() returned an error.
    if let Err(e) = disable_raw_mode() {
        tracing::warn!("Failed to restore terminal raw mode: {:#}", e);
    }

    // Print a final newline so the shell prompt of the calling process appears
    // on its own line.
    println!();

    result
}

async fn forward(stream: TcpStream) -> Result<()> {
    use std::io::Read;
    use tokio::sync::mpsc;

    let (mut tcp_rd, mut tcp_wr) = stream.into_split();

    // Use a regular OS thread for stdin so that the tokio runtime is not
    // blocked waiting for the stdin read to complete when the session ends.
    // tokio::io::stdin() uses an internal blocking thread that cannot be
    // cancelled, which would prevent the process from exiting after the
    // connection closes.  A plain OS thread is not tracked by the tokio
    // runtime, so the runtime can shut down cleanly even if the thread is
    // still blocked inside read(2).
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Async task: drain the stdin channel and forward bytes to the TCP stream.
    let write_task = tokio::spawn(async move {
        while let Some(data) = stdin_rx.recv().await {
            if tcp_wr.write_all(&data).await.is_err() {
                break;
            }
        }
    });

    // Main task: copies TCP → stdout
    let mut stdout = tokio::io::stdout();
    let mut buf = [0u8; 4096];
    loop {
        match tcp_rd.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).await.is_err() {
                    break;
                }
                let _ = stdout.flush().await;
            }
        }
    }

    write_task.abort();
    Ok(())
}

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
    let (mut tcp_rd, mut tcp_wr) = stream.into_split();

    // Spawn a task that copies stdin → TCP
    let write_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tcp_wr.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
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

//! Stream bounded command output without detached readers or abandoned child processes.
use crate::{CommandOutput, OutputStream, ProgressSink};
use anyhow::{Context, Result};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

async fn read_output(
    mut reader: impl AsyncRead + Unpin,
    stream: OutputStream,
    progress: Option<ProgressSink>,
) -> Result<String> {
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut overflow = false;
    let mut buffer = [0u8; 4096];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        anyhow::ensure!(
            output.len() + count <= 4 * 1024 * 1024,
            "Worker command exceeded its 4 MiB output limit"
        );
        output.extend_from_slice(&buffer[..count]);
        if let Some(progress) = &progress {
            for byte in &buffer[..count] {
                if *byte == b'\n' || *byte == b'\r' {
                    if overflow {
                        progress(stream, "[Output omitted: line exceeds 4096 bytes]");
                    } else if !line.is_empty() {
                        progress(stream, &String::from_utf8_lossy(&line));
                    }
                    line.clear();
                    overflow = false;
                } else if line.len() < 4096 {
                    line.push(*byte);
                } else {
                    overflow = true;
                }
            }
        }
    }
    if let Some(progress) = progress {
        if overflow {
            progress(stream, "[Output omitted: line exceeds 4096 bytes]");
        } else if !line.is_empty() {
            progress(stream, &String::from_utf8_lossy(&line));
        }
    }
    Ok(String::from_utf8_lossy(&output).into())
}

pub async fn run_command(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    timeout: u64,
    progress: Option<ProgressSink>,
) -> Result<CommandOutput> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Could not start the worker command")?;
    let stdout = child
        .stdout
        .take()
        .context("Worker output is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Worker error output is unavailable")?;
    let input = child.stdin.take();
    let work = async {
        let write = async {
            if let Some(bytes) = stdin {
                let mut input = input.context("Worker input is unavailable")?;
                input.write_all(&bytes).await?;
                input.shutdown().await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (status, stdout, stderr, ()) = tokio::try_join!(
            async { Ok::<_, anyhow::Error>(child.wait().await?) },
            read_output(stdout, OutputStream::Stdout, progress.clone()),
            read_output(stderr, OutputStream::Stderr, progress),
            write
        )?;
        Ok(CommandOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    };
    tokio::time::timeout(Duration::from_secs(timeout), work)
        .await
        .with_context(|| format!("The worker operation timed out after {timeout} seconds"))?
}

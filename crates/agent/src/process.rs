//! Stream bounded command output without detached readers or abandoned child processes.
use crate::{CommandOutput, OutputStream, ProgressSink};
use anyhow::{Context, Result};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

#[derive(Clone, Copy)]
pub enum OutputFormat {
    Lines,
    /// Multipass's stdout uses ANSI redraws and a backspace followed by a spinner glyph.
    Terminal,
}

/// Stream an opaque archive without text conversion, host extraction or an
/// unbounded in-memory buffer. Dropping the operation kills its runtime child.
pub async fn stream_command(
    mut command: Command,
    input: Option<std::fs::File>,
    output: Option<std::fs::File>,
    limit: u64,
) -> Result<()> {
    command
        .stdin(input.map_or(Stdio::null(), Stdio::from))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Cannot start the worker backup stream")?;
    let mut stdout = child.stdout.take().context("No backup output")?;
    let stderr = child.stderr.take().context("No backup error output")?;
    let work = async {
        let copy = async {
            let mut output = output.map(tokio::fs::File::from_std);
            let mut bytes = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let count = stdout.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                bytes = bytes
                    .checked_add(count as u64)
                    .context("Backup size overflow")?;
                anyhow::ensure!(
                    bytes <= limit,
                    "Backup exceeded its reviewed temporary space; source images preserved"
                );
                if let Some(file) = &mut output {
                    file.write_all(&buffer[..count]).await?;
                }
            }
            if let Some(file) = &mut output {
                file.sync_all().await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (status, errors, ()) = tokio::try_join!(
            async { Ok::<_, anyhow::Error>(child.wait().await?) },
            read_output(stderr, OutputStream::Stderr, None, OutputFormat::Lines),
            copy
        )?;
        anyhow::ensure!(
            status.success(),
            "Worker backup operation failed: {}",
            errors.chars().take(1200).collect::<String>()
        );
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(24 * 3600), work)
        .await
        .context("Worker backup operation timed out")?
}

async fn read_output(
    mut reader: impl AsyncRead + Unpin,
    stream: OutputStream,
    progress: Option<ProgressSink>,
    format: OutputFormat,
) -> Result<String> {
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut overflow = false;
    let terminal = matches!(format, OutputFormat::Terminal) && stream == OutputStream::Stdout;
    let mut spinner_glyph = false;
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
                if spinner_glyph && !byte.is_ascii_control() {
                    spinner_glyph = false;
                    continue;
                }
                spinner_glyph = false;
                if *byte == b'\n'
                    || *byte == b'\r'
                    || (terminal && matches!(byte, b'\x08' | b'\x1b'))
                {
                    if overflow {
                        progress(stream, "[Output omitted: line exceeds 4096 bytes]");
                    } else if !line.is_empty() {
                        progress(stream, &String::from_utf8_lossy(&line));
                    }
                    line.clear();
                    overflow = false;
                    if *byte == b'\x1b' {
                        // Keep the escape sequence intact for the log's terminal-control sanitizer.
                        line.push(*byte);
                    }
                    spinner_glyph = terminal && *byte == b'\x08';
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
    format: OutputFormat,
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
            read_output(stdout, OutputStream::Stdout, progress.clone(), format),
            read_output(stderr, OutputStream::Stderr, progress, format),
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

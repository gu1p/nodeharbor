use nodeharbor_agent::{
    process::{run_command, OutputFormat},
    OutputStream, ProgressSink,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{process::Command, sync::Notify};

fn child(directory: &std::path::Path, mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process_fixture", "--ignored", "--nocapture"])
        .env("NODEHARBOR_TEST_CHILD_DIRECTORY", directory)
        .env("NODEHARBOR_TEST_CHILD_MODE", mode);
    command
}

#[test]
#[ignore = "Executed only as a controlled subprocess by the progress tests"]
fn process_fixture() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("NODEHARBOR_TEST_CHILD_DIRECTORY").expect("Controlled subprocess only"),
    );
    std::fs::write(directory.join("pid"), std::process::id().to_string()).unwrap();
    if std::env::var("NODEHARBOR_TEST_CHILD_MODE").unwrap() == "spinner" {
        print!("\x1b[2K\x1b[0A\x1b[0EWaiting for the VM to receive an IP address  ");
        std::io::stdout().flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !directory.join("release").exists() && std::time::Instant::now() < deadline {
            for glyph in ['/', '-', '\\', '|'] {
                print!("\x08{glyph}");
                std::io::stdout().flush().unwrap();
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        println!("\x08 \x1b[2K\x1b[0A\x1b[0ELaunched");
        return;
    }
    if std::env::var("NODEHARBOR_TEST_CHILD_MODE").unwrap() == "fragmented" {
        print!("Access denied for private-");
        std::io::stdout().flush().unwrap();
        std::fs::write(directory.join("fragment"), "").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !directory.join("release").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        println!("bootstrap-value");
        return;
    }
    if std::env::var("NODEHARBOR_TEST_CHILD_MODE").unwrap() == "large" {
        std::io::stdout()
            .write_all(&vec![b'x'; 5 * 1024 * 1024])
            .unwrap();
        return;
    }
    print!("Downloading Ubuntu: 25%\r");
    std::io::stdout().flush().unwrap();
    eprintln!("Waiting for network");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !directory.join("release").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("Finished");
}

#[tokio::test]
async fn multipass_spinner_messages_appear_without_waiting_for_a_newline() {
    let directory = tempfile::tempdir().unwrap();
    let log = nodeharbor_agent::activity::ActivityLog::default();
    let copy = log.clone();
    let progress: ProgressSink = Arc::new(move |_, line| copy.record("info", "multipass", line));
    let task = tokio::spawn(run_command(
        child(directory.path(), "spinner"),
        None,
        10,
        Some(progress),
        OutputFormat::Terminal,
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !log
            .snapshot()
            .entries
            .iter()
            .any(|entry| entry.message == "Waiting for the VM to receive an IP address")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Multipass spinner progress must appear before its command finishes");
    assert!(!task.is_finished());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!log
        .snapshot()
        .entries
        .iter()
        .any(|entry| matches!(entry.message.as_str(), "/" | "-" | "\\" | "|")));
    std::fs::write(directory.path().join("release"), "").unwrap();
    assert!(task.await.unwrap().unwrap().success);
}

#[tokio::test]
async fn split_output_is_reassembled_before_redaction_and_cancellation_stops_the_process() {
    let directory = tempfile::tempdir().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let output = lines.clone();
    let progress: ProgressSink = Arc::new(move |_, line| {
        output
            .lock()
            .unwrap()
            .push(nodeharbor_agent::activity::redact(
                line,
                &["private-bootstrap-value".into()],
            ))
    });
    let task = tokio::spawn(run_command(
        child(directory.path(), "fragmented"),
        None,
        10,
        Some(progress),
        OutputFormat::Lines,
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !directory.path().join("fragment").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!lines
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.contains("private-")));
    std::fs::write(directory.path().join("release"), "").unwrap();
    task.await.unwrap().unwrap();
    let output = lines.lock().unwrap().join("\n");
    assert!(output.contains("Access denied for [redacted]"));
    assert!(!output.contains("private-bootstrap-value"));

    let directory = tempfile::tempdir().unwrap();
    let task = tokio::spawn(run_command(
        child(directory.path(), "cancelled"),
        None,
        10,
        None,
        OutputFormat::Lines,
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !directory.path().join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.err().unwrap().is_cancelled());
    let pid = sysinfo::Pid::from(
        std::fs::read_to_string(directory.path().join("pid"))
            .unwrap()
            .parse::<usize>()
            .unwrap(),
    );
    let mut system = sysinfo::System::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if system.process(pid).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Cancelled commands must not keep a child process running");
}

#[tokio::test]
async fn actual_process_output_is_read_while_it_is_running_on_both_streams() {
    let directory = tempfile::tempdir().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let notified = Arc::new(Notify::new());
    let copy = lines.clone();
    let wake = notified.clone();
    let progress: ProgressSink = Arc::new(move |source, line| {
        copy.lock().unwrap().push((source, line.to_string()));
        wake.notify_one();
    });
    let task = tokio::spawn(run_command(
        child(directory.path(), "progress"),
        None,
        10,
        Some(progress),
        OutputFormat::Terminal,
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lines
                .lock()
                .unwrap()
                .iter()
                .any(|(s, l)| *s == OutputStream::Stdout && l.contains("25%"))
                && lines
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(s, l)| *s == OutputStream::Stderr && l.contains("Waiting for network"))
            {
                break;
            }
            notified.notified().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    std::fs::write(directory.path().join("release"), "").unwrap();
    let result = task.await.unwrap().unwrap();
    assert!(result.success);
    assert!(result.stdout.contains("Finished"));
}

#[tokio::test]
async fn excessive_output_is_bounded_and_a_timeout_terminates_the_child() {
    let directory = tempfile::tempdir().unwrap();
    let error = run_command(
        child(directory.path(), "large"),
        None,
        10,
        None,
        OutputFormat::Lines,
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("output limit"));
    let error = run_command(
        child(directory.path(), "timeout"),
        None,
        1,
        None,
        OutputFormat::Lines,
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("timed out"));
    let pid = sysinfo::Pid::from(
        std::fs::read_to_string(directory.path().join("pid"))
            .unwrap()
            .parse::<usize>()
            .unwrap(),
    );
    let mut system = sysinfo::System::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if system.process(pid).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Timed-out child must not keep running");
}

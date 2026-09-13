//! Bounded, local diagnostics. Commands, input payloads and credentials are never recorded.
use crate::{CommandOutput, OutputStream, ProgressSink, Runner};
use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: u64,
    pub timestamp: String,
    pub level: String,
    pub source: String,
    pub message: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStep {
    pub message: String,
    pub started_at: String,
    pub timeout_seconds: Option<u64>,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySnapshot {
    pub entries: Vec<ActivityEntry>,
    pub dropped: u64,
    pub step: Option<ActivityStep>,
}
#[derive(Default)]
struct History {
    entries: VecDeque<ActivityEntry>,
    next: u64,
    dropped: u64,
    step: Option<ActivityStep>,
}
#[derive(Clone, Default)]
pub struct ActivityLog(Arc<Mutex<History>>);
impl ActivityLog {
    pub fn record(&self, level: &str, source: &str, message: &str) {
        let message = redact(message, &[]);
        if message.is_empty() {
            return;
        }
        let mut history = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if history.entries.back().is_some_and(|entry| {
            entry.level == level && entry.source == source && entry.message == message
        }) {
            return;
        }
        history.next += 1;
        let id = history.next;
        history.entries.push_back(ActivityEntry {
            id,
            timestamp: Utc::now().to_rfc3339(),
            level: level.into(),
            source: source.into(),
            message,
        });
        if history.entries.len() > 500 {
            history.entries.pop_front();
            history.dropped += 1;
        }
    }
    pub fn begin_step(&self, message: &str, timeout_seconds: Option<u64>) {
        {
            let mut history = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if history
                .step
                .as_ref()
                .is_some_and(|step| step.message == message)
            {
                return;
            }
            history.step = Some(ActivityStep {
                message: redact(message, &[]),
                started_at: Utc::now().to_rfc3339(),
                timeout_seconds,
            });
        }
        self.record("info", "agent", message);
    }
    pub fn finish_step(&self) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .step = None;
    }
    pub fn snapshot(&self) -> ActivitySnapshot {
        let history = self.0.lock().unwrap_or_else(|error| error.into_inner());
        ActivitySnapshot {
            entries: history.entries.iter().cloned().collect(),
            dropped: history.dropped,
            step: history.step.clone(),
        }
    }
}

pub fn redact(message: &str, secrets: &[String]) -> String {
    if message.len() > 4096 {
        return "[Output omitted: line exceeds 4096 bytes]".into();
    }
    let mut clean = String::new();
    let mut escape = 0;
    for character in message.chars() {
        match escape {
            1 => {
                escape = match character {
                    '[' => 2,
                    ']' => 3,
                    _ => 0,
                }
            }
            2 => {
                if ('@'..='~').contains(&character) {
                    escape = 0;
                }
            }
            3 => {
                if character == '\u{7}' {
                    escape = 0;
                } else if character == '\u{1b}' {
                    escape = 4;
                }
            }
            4 => escape = if character == '\\' { 0 } else { 3 },
            _ => {
                if character == '\u{1b}' {
                    escape = 1;
                } else if !character.is_control() || character == '\t' {
                    clean.push(character);
                }
            }
        }
    }
    for secret in secrets {
        if !secret.is_empty() {
            clean = clean.replace(secret, "[redacted]");
        }
    }
    let lower = clean.to_ascii_lowercase();
    if [
        "authorization",
        "password",
        "token",
        "secret",
        "setup_key",
        "setup-key",
        "setupkey",
    ]
    .iter()
    .any(|key| lower.contains(key))
    {
        return "[Sensitive output omitted]".into();
    }
    clean.trim().into()
}

pub(crate) struct ActivityRunner {
    pub inner: Arc<dyn Runner>,
    pub log: ActivityLog,
}
fn command_step(args: &[String]) -> Option<&'static str> {
    match args.first()?.as_str() {
        "launch" => Some("Creating Ubuntu VM"),
        "start" => Some("Starting Ubuntu VM"),
        "stop" => Some("Stopping Ubuntu VM"),
        "delete" => Some("Removing the stopped worker VM"),
        "set" => Some("Applying worker resource limits"),
        "exec"
            if args
                .iter()
                .any(|arg| arg == "/usr/local/lib/nodeharbor/configure_worker.py") =>
        {
            Some("Configuring the private network and Kubernetes")
        }
        "exec" if args.iter().any(|arg| arg == "cloud-init") => {
            Some("Waiting for Ubuntu initialization")
        }
        _ => None,
    }
}
fn input_secrets(input: Option<&[u8]>) -> Vec<String> {
    fn collect(value: &serde_json::Value, secrets: &mut Vec<String>) {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if ["token", "secret", "key", "password", "code"]
                    .iter()
                    .any(|word| key.to_ascii_lowercase().contains(word))
                {
                    if let Some(secret) = value.as_str() {
                        secrets.push(secret.into());
                    }
                } else {
                    collect(value, secrets);
                }
            }
        }
    }
    let mut secrets = vec![];
    if let Some(value) = input.and_then(|bytes| serde_json::from_slice(bytes).ok()) {
        collect(&value, &mut secrets);
    }
    secrets
}
#[async_trait]
impl Runner for ActivityRunner {
    async fn run(
        &self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        timeout: u64,
    ) -> anyhow::Result<CommandOutput> {
        let Some(step) = command_step(args) else {
            return self.inner.run(args, stdin, timeout).await;
        };
        self.log.begin_step(step, Some(timeout));
        let secrets = input_secrets(stdin.as_deref());
        let log = self.log.clone();
        let redactions = secrets.clone();
        let source = if args[0] == "exec" {
            "worker"
        } else {
            "multipass"
        };
        let progress: ProgressSink = Arc::new(move |stream, line| {
            let message = redact(line, &redactions);
            if stream == OutputStream::Stdout {
                if let Some(step) = message.strip_prefix("NodeHarbor step: ") {
                    log.begin_step(step, None);
                    return;
                }
            }
            log.record("info", source, &message);
        });
        let result = self
            .inner
            .run_with_progress(args, stdin, timeout, progress)
            .await;
        self.log.finish_step();
        match result {
            Ok(mut output) => {
                output.stderr = redact(&output.stderr, &secrets);
                if output.success {
                    self.log
                        .record("info", "agent", &format!("{step}: completed"));
                } else {
                    self.log.record(
                        "error",
                        source,
                        &format!("{step} failed: {}", output.stderr),
                    );
                }
                Ok(output)
            }
            Err(error) => {
                let message = redact(&error.to_string(), &secrets);
                self.log
                    .record("error", "agent", &format!("{step} failed: {message}"));
                Err(anyhow::anyhow!(message))
            }
        }
    }
}

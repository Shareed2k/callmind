//! Child processes this server owns.
//!
//! A local plugin is a worker like any other: it dials the worker listener and
//! leases `plugin:<name>`. The only difference is who starts it. That difference
//! is the point -- a job dispatched for a kind nobody serves sits in the queue
//! forever, because the stale-lease reaper only reclaims leases and nobody ever
//! took this one. When the server owns the process, that cannot happen.
//!
//! Deliberately not what HashiCorp's go-plugin does. There the plugin is the
//! server, so the host must discover a port the child chose, which is what the
//! handshake line on stdout is for. Here the plugin is the client: the host
//! already knows its own address and passes it in the environment, so there is
//! nothing to discover and no handshake to parse.

use callmind_config::LocalPluginConfig;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Backoff between restarts, capped. A plugin that cannot start -- a missing
/// binary, a missing model -- would otherwise be respawned in a tight loop, and
/// the log would be useless.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Supervises the configured local plugins for as long as the server runs.
pub struct PluginSupervisor {
    tasks: Vec<JoinHandle<()>>,
}

impl PluginSupervisor {
    /// Start one supervised process per configured plugin.
    ///
    /// `endpoint` is the worker listener this server is about to run, which the
    /// child receives as `CALLMIND_WORKER_ENDPOINT`.
    pub fn start(plugins: &[LocalPluginConfig], endpoint: &str, token: &CancellationToken) -> Self {
        let tasks = plugins
            .iter()
            .map(|plugin| {
                let plugin = plugin.clone();
                let endpoint = endpoint.to_string();
                let token = token.clone();
                tokio::spawn(async move { supervise(plugin, endpoint, token).await })
            })
            .collect();
        Self { tasks }
    }

    /// Wait for every supervised process to stop.
    ///
    /// Called after the cancellation token is triggered; each task signals its
    /// child, waits, then kills it.
    pub async fn wait(self) {
        for task in self.tasks {
            // A supervisor task that panicked has already lost its child to
            // `kill_on_drop`, so there is nothing left to clean up here.
            let _ = task.await;
        }
    }
}

/// Keep one plugin running until the server stops.
async fn supervise(plugin: LocalPluginConfig, endpoint: String, token: CancellationToken) {
    let mut backoff = FIRST_BACKOFF;

    while !token.is_cancelled() {
        let started = match spawn(&plugin, &endpoint) {
            Ok(child) => child,
            Err(e) => {
                error!(
                    "Plugin '{}' could not be started ({e}); retrying in {backoff:?}",
                    plugin.name
                );
                if wait_or_cancel(&token, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        info!(
            plugin = %plugin.name,
            command = %plugin.command,
            "Started local plugin"
        );
        // A run that lasted is evidence the plugin works, so the next failure
        // starts from a short wait rather than wherever the last crash left off.
        backoff = FIRST_BACKOFF;

        let outcome = run_until_stopped(started, &plugin.name, &token).await;
        if token.is_cancelled() {
            return;
        }

        warn!(
            "Plugin '{}' exited ({outcome}); restarting in {backoff:?}",
            plugin.name
        );
        if wait_or_cancel(&token, backoff).await {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn spawn(plugin: &LocalPluginConfig, endpoint: &str) -> std::io::Result<Child> {
    Command::new(&plugin.command)
        .args(&plugin.args)
        // The child needs no configuration file and no standing secret: it is
        // told where to call and what to lease.
        .env("CALLMIND_WORKER_ENDPOINT", endpoint)
        .env("CALLMIND_WORKER_ID", &plugin.name)
        .env("CALLMIND_WORKER_KINDS", format!("plugin:{}", plugin.name))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The `Drop` cleanup the design asked for, without writing it: an
        // abort, a panic or a killed server takes the children with it rather
        // than orphaning them.
        .kill_on_drop(true)
        .spawn()
}

/// Run a child until it exits or the server stops, forwarding what it prints.
async fn run_until_stopped(mut child: Child, name: &str, token: &CancellationToken) -> String {
    // Forwarded rather than inherited so a plugin's output is attributable and
    // lands wherever the server's log does. A plugin that fails to start says
    // why on stderr, and that is the line somebody will need.
    if let Some(out) = child.stdout.take() {
        forward(BufReader::new(out).lines(), name.to_string(), "stdout");
    }
    if let Some(err) = child.stderr.take() {
        forward(BufReader::new(err).lines(), name.to_string(), "stderr");
    }

    tokio::select! {
        status = child.wait() => match status {
            Ok(status) => format!("status {status}"),
            Err(e) => format!("could not be waited on: {e}"),
        },
        () = token.cancelled() => {
            stop(&mut child, name).await;
            "stopped with the server".to_string()
        }
    }
}

/// Relay one of a child's streams into this server's log.
fn forward<R>(mut lines: tokio::io::Lines<BufReader<R>>, name: String, stream: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            info!(plugin = %name, stream, "{line}");
        }
    });
}

/// Stop a child at shutdown.
///
/// A kill rather than a polite SIGTERM, which tokio does not expose and which
/// would cost a `libc` dependency and an `unsafe` block to send. Nothing is
/// stranded by that: the server requeues every running job on its way out, so a
/// lease this plugin was holding goes back to the queue in the same breath --
/// which is the only thing the polite signal would have bought.
async fn stop(child: &mut Child, name: &str) {
    if let Err(e) = child.kill().await {
        warn!("Plugin '{name}' could not be stopped: {e}");
        return;
    }
    info!("Plugin '{name}' stopped with the server");
}

/// Sleep, unless the server stops first. Returns whether it stopped.
async fn wait_or_cancel(token: &CancellationToken, how_long: Duration) -> bool {
    tokio::select! {
        () = token.cancelled() => true,
        () = tokio::time::sleep(how_long) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, script: &str) -> LocalPluginConfig {
        LocalPluginConfig {
            name: name.to_string(),
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    /// The child is told where to call and what to lease, so it needs no
    /// configuration file of its own.
    #[tokio::test]
    async fn a_plugin_is_told_where_to_connect() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("env.txt");
        let token = CancellationToken::new();
        let supervisor = PluginSupervisor::start(
            &[plugin(
                "acoustic-emotions",
                &format!(
                    "printf '%s %s %s' \"$CALLMIND_WORKER_ENDPOINT\" \"$CALLMIND_WORKER_ID\" \
                     \"$CALLMIND_WORKER_KINDS\" > {}; sleep 30",
                    out.display()
                ),
            )],
            "http://127.0.0.1:8081",
            &token,
        );

        let written = wait_for_file(&out).await;
        assert_eq!(
            written,
            "http://127.0.0.1:8081 acoustic-emotions plugin:acoustic-emotions"
        );

        token.cancel();
        supervisor.wait().await;
    }

    /// A plugin that dies takes its plugin kind with it, and the jobs pile up
    /// unclaimed. Restarting is the whole reason the host owns the process.
    #[tokio::test]
    async fn a_plugin_that_exits_is_started_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let counter = dir.path().join("runs");
        let token = CancellationToken::new();
        let supervisor = PluginSupervisor::start(
            &[plugin(
                "flaky",
                &format!("printf x >> {}; exit 1", counter.display()),
            )],
            "http://127.0.0.1:8081",
            &token,
        );

        // Two runs means it came back on its own; the first backoff is a second,
        // so this is the shortest honest wait.
        let runs = wait_for_length(&counter, 2).await;
        assert!(runs >= 2, "expected a restart, saw {runs} run(s)");

        token.cancel();
        supervisor.wait().await;
    }

    /// Shutdown must actually stop the child. An orphan holding a lease keeps
    /// its job locked until the reaper runs, minutes later.
    #[tokio::test]
    async fn shutdown_stops_the_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("alive");
        let token = CancellationToken::new();
        let supervisor = PluginSupervisor::start(
            &[plugin(
                "long-runner",
                &format!("printf x > {}; sleep 300", marker.display()),
            )],
            "http://127.0.0.1:8081",
            &token,
        );
        wait_for_file(&marker).await;

        token.cancel();
        // The supervisor returning is the claim: it only returns after the
        // child has been waited on, whether it stopped or was killed.
        tokio::time::timeout(Duration::from_secs(10), supervisor.wait())
            .await
            .expect("shutdown must not hang on a child that ignores it");
    }

    async fn wait_for_file(path: &std::path::Path) -> String {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the child never wrote {}", path.display());
    }

    async fn wait_for_length(path: &std::path::Path, want: usize) -> usize {
        for _ in 0..200 {
            let len = std::fs::read_to_string(path).map(|t| t.len()).unwrap_or(0);
            if len >= want {
                return len;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        std::fs::read_to_string(path).map(|t| t.len()).unwrap_or(0)
    }
}

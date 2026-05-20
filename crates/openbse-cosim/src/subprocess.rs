//! Subprocess transport: newline-delimited JSON over stdin/stdout.
//!
//! One `SubprocessTransport` manages the lifecycle of a single child process.
//! Each call to `exchange()` writes one JSON line to the child's stdin and
//! reads one JSON line back from its stdout. The process is stopped cleanly
//! when the transport is dropped.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

// ─── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct CosimRequest<'a> {
    time_s: f64,
    dt_s: f64,
    inputs: &'a HashMap<String, f64>,
}

#[derive(Deserialize)]
struct CosimResponse {
    outputs: Option<HashMap<String, f64>>,
    error: Option<String>,
}

// ─── Transport ───────────────────────────────────────────────────────────────

pub struct SubprocessTransport {
    child: Child,
    stdin: std::io::BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl SubprocessTransport {
    /// Spawn the child process with piped stdin/stdout.
    pub fn spawn(command: &[String]) -> Result<Self, String> {
        if command.is_empty() {
            return Err("cosim command list is empty".to_string());
        }
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn '{}': {}", command[0], e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or("failed to capture subprocess stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("failed to capture subprocess stdout")?;

        Ok(Self {
            child,
            stdin: std::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    /// Send one timestep request and return the outputs map.
    pub fn exchange(
        &mut self,
        time_s: f64,
        dt_s: f64,
        inputs: &HashMap<String, f64>,
    ) -> Result<HashMap<String, f64>, String> {
        let req = CosimRequest {
            time_s,
            dt_s,
            inputs,
        };
        let json =
            serde_json::to_string(&req).map_err(|e| format!("serialization error: {}", e))?;

        writeln!(self.stdin, "{}", json)
            .map_err(|e| format!("write to subprocess failed: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("flush to subprocess failed: {}", e))?;

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|e| format!("read from subprocess failed: {}", e))?;

        if line.is_empty() {
            return Err("subprocess closed stdout unexpectedly".to_string());
        }

        let resp: CosimResponse = serde_json::from_str(line.trim()).map_err(|e| {
            format!(
                "failed to parse subprocess response '{}': {}",
                line.trim(),
                e
            )
        })?;

        if let Some(err) = resp.error {
            return Err(format!("subprocess reported error: {}", err));
        }

        resp.outputs
            .ok_or_else(|| "subprocess returned no outputs field".to_string())
    }

    fn send_stop(&mut self) {
        let _ = self.stdin.write_all(b"{\"command\":\"stop\"}\n");
        let _ = self.stdin.flush();
    }
}

impl Drop for SubprocessTransport {
    fn drop(&mut self) {
        self.send_stop();
        let _ = self.child.wait();
    }
}

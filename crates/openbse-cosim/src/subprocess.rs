//! Subprocess transport: newline-delimited JSON over stdin/stdout.
//!
//! One `SubprocessTransport` manages the lifecycle of a single child process.
//! Each call to `exchange()` writes one JSON line to the child's stdin and
//! reads one JSON line back from its stdout, waiting at most the transport's
//! timeout. The process is stopped when the transport is dropped.
//!
//! `CosimLink` wraps the transport with what both proxy components share:
//! starting the process on first use, counting the exchanges within a
//! timestep, the warmup flag, checking the declared outputs, and keeping the
//! first failure for the driver, which stops the run on it.

use openbse_core::ports::SimulationContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// Default time to wait for one response before the run stops [s].
pub const DEFAULT_TIMEOUT_S: f64 = 60.0;

// ─── Wire types ──────────────────────────────────────────────────────────────

/// One request. `time_s` is the same for every exchange within a timestep;
/// `iteration` counts them from 0. The engine keeps the outputs of the last
/// exchange at a given `time_s`.
#[derive(Serialize)]
struct CosimRequest<'a> {
    time_s: f64,
    dt_s: f64,
    iteration: u32,
    warmup: bool,
    sizing: bool,
    inputs: &'a HashMap<String, f64>,
}

#[derive(Deserialize)]
struct CosimResponse {
    outputs: Option<HashMap<String, f64>>,
    error: Option<String>,
}

/// Request fields other than `inputs`.
#[derive(Debug, Clone, Copy)]
pub struct RequestHeader {
    pub time_s: f64,
    pub dt_s: f64,
    pub iteration: u32,
    pub warmup: bool,
    pub sizing: bool,
}

// ─── Transport ───────────────────────────────────────────────────────────────

pub struct SubprocessTransport {
    child: Child,
    stdin: std::io::BufWriter<ChildStdin>,
    /// Lines read from the child's stdout by a reader thread, so a response
    /// can be awaited with a timeout.
    lines: Receiver<std::io::Result<String>>,
    timeout: Duration,
    /// Set after a timeout: the child is killed rather than asked to stop.
    hung: bool,
}

impl SubprocessTransport {
    /// Spawn the child process with piped stdin/stdout, in `working_dir` when
    /// given (the model's directory, so its relative paths resolve there).
    pub fn spawn(
        command: &[String],
        working_dir: Option<&std::path::Path>,
        timeout_s: f64,
    ) -> Result<Self, String> {
        if command.is_empty() {
            return Err("cosim command list is empty".to_string());
        }
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd
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

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                let res = reader.read_line(&mut line).map(|_| line);
                let done = !matches!(&res, Ok(l) if !l.is_empty());
                if tx.send(res).is_err() || done {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin: std::io::BufWriter::new(stdin),
            lines: rx,
            timeout: Duration::from_secs_f64(timeout_s.max(0.001)),
            hung: false,
        })
    }

    /// Send one request and return the outputs map.
    pub fn exchange(
        &mut self,
        header: RequestHeader,
        inputs: &HashMap<String, f64>,
    ) -> Result<HashMap<String, f64>, String> {
        let req = CosimRequest {
            time_s: header.time_s,
            dt_s: header.dt_s,
            iteration: header.iteration,
            warmup: header.warmup,
            sizing: header.sizing,
            inputs,
        };
        let json =
            serde_json::to_string(&req).map_err(|e| format!("serialization error: {}", e))?;

        writeln!(self.stdin, "{}", json)
            .map_err(|e| format!("write to subprocess failed: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("flush to subprocess failed: {}", e))?;

        let line = match self.lines.recv_timeout(self.timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(e)) => return Err(format!("read from subprocess failed: {}", e)),
            Err(RecvTimeoutError::Timeout) => {
                self.hung = true;
                return Err(format!(
                    "no response within {:.1} s",
                    self.timeout.as_secs_f64()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => String::new(),
        };

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
        if self.hung {
            let _ = self.child.kill();
        } else {
            self.send_stop();
            // Give the child a moment to exit on its own, then kill it so a
            // child that ignores the stop message can't hang the engine.
            for _ in 0..50 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

// ─── Link shared by the proxy components ─────────────────────────────────────

pub struct CosimLink {
    name: String,
    command: Vec<String>,
    output_vars: Vec<String>,
    working_dir: Option<std::path::PathBuf>,
    timeout_s: f64,
    transport: Option<SubprocessTransport>,
    warmup: bool,
    /// `(time_s, warmup)` of the previous exchange and how many were made at it.
    last_time: Option<(f64, bool)>,
    iteration: u32,
    /// The first spawn or exchange failure; the driver stops the run on it.
    failure: Option<String>,
}

impl CosimLink {
    pub fn new(name: &str, command: Vec<String>, output_vars: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            command,
            output_vars,
            working_dir: None,
            timeout_s: DEFAULT_TIMEOUT_S,
            transport: None,
            warmup: false,
            last_time: None,
            iteration: 0,
            failure: None,
        }
    }

    pub fn set_working_dir(&mut self, dir: Option<&std::path::Path>) {
        self.working_dir = dir.map(|d| d.to_path_buf());
    }

    pub fn set_timeout(&mut self, timeout_s: f64) {
        self.timeout_s = timeout_s;
    }

    pub fn set_warmup(&mut self, warmup: bool) {
        self.warmup = warmup;
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.clone()
    }

    /// Record a failure (the first one is kept) and log it.
    pub fn fail(&mut self, msg: String) {
        let msg = format!("cosim '{}': {}", self.name, msg);
        log::error!("{msg}");
        self.failure.get_or_insert(msg);
    }

    /// Exchange once. Returns `None` after recording a failure: a failed
    /// start, a transport error, an error reply, or a response missing any
    /// declared output.
    pub fn exchange(
        &mut self,
        ctx: &SimulationContext,
        inputs: &HashMap<String, f64>,
    ) -> Option<HashMap<String, f64>> {
        if self.failure.is_some() {
            return None;
        }
        if self.transport.is_none() {
            match SubprocessTransport::spawn(
                &self.command,
                self.working_dir.as_deref(),
                self.timeout_s,
            ) {
                Ok(t) => self.transport = Some(t),
                Err(e) => {
                    self.fail(format!("failed to start subprocess: {e}"));
                    return None;
                }
            }
        }
        let time = (ctx.timestep.sim_time_s, self.warmup);
        self.iteration = match self.last_time {
            Some(last) if last == time => self.iteration + 1,
            _ => 0,
        };
        self.last_time = Some(time);
        let header = RequestHeader {
            time_s: ctx.timestep.sim_time_s,
            dt_s: ctx.timestep.dt,
            iteration: self.iteration,
            warmup: self.warmup,
            sizing: ctx.is_sizing,
        };
        let result = self
            .transport
            .as_mut()
            .expect("transport started above")
            .exchange(header, inputs);
        let outputs = match result {
            Ok(o) => o,
            Err(e) => {
                self.fail(format!("exchange failed: {e}"));
                return None;
            }
        };
        let missing: Vec<&str> = self
            .output_vars
            .iter()
            .filter(|v| !outputs.contains_key(v.as_str()))
            .map(|v| v.as_str())
            .collect();
        if !missing.is_empty() {
            self.fail(format!(
                "response at time {} s lacks declared output(s) {}",
                ctx.timestep.sim_time_s,
                missing.join(", ")
            ));
            return None;
        }
        if let Some((k, v)) = outputs.iter().find(|(_, v)| !v.is_finite()) {
            self.fail(format!("output '{k}' is not finite ({v})"));
            return None;
        }
        Some(outputs)
    }

    /// Check a reported `thermal_output_w` against the heat the outlet state
    /// carries; a model whose two answers disagree has a bug.
    pub fn check_thermal(&mut self, outputs: &HashMap<String, f64>, from_states_w: f64) {
        if let Some(&reported) = outputs.get("thermal_output_w") {
            let tol = (0.02 * from_states_w.abs()).max(10.0);
            if (reported - from_states_w).abs() > tol {
                self.fail(format!(
                    "thermal_output_w {reported:.1} W disagrees with the {from_states_w:.1} W \
                     the returned outlet state carries"
                ));
            }
        }
    }
}

/// Names in `given` that aren't in `known`.
pub fn unknown_names<'a>(given: &'a [String], known: &[&str]) -> Vec<&'a str> {
    given
        .iter()
        .filter(|v| !known.contains(&v.as_str()))
        .map(|v| v.as_str())
        .collect()
}

//! Getting a debugger websocket for a process that is already running.
//!
//! Node opens its inspector when it receives SIGUSR1, which is what lets us
//! profile a process nobody planned to profile. Bun and Deno do not do this,
//! so for those the user starts with `--inspect` and we just discover the
//! endpoint that is already listening.

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::thread::sleep;
use std::time::Duration;

/// Ask the inspector which targets it has, and take the first debuggable one.
pub fn discover(port: u16) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let body = ureq::get(&url)
        .timeout(Duration::from_millis(800))
        .call()
        .map_err(|e| format!("inspector not reachable on port {port}: {e}"))?
        .into_string()
        .map_err(|e| format!("could not read inspector response: {e}"))?;

    let targets: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("inspector sent invalid JSON: {e}"))?;

    targets
        .as_array()
        .and_then(|list| {
            list.iter()
                .find_map(|t| t.get("webSocketDebuggerUrl")?.as_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| "inspector is listening but exposed no debuggable target".into())
}

/// Open the inspector on a running Node process, then connect to it.
pub fn attach(pid: i32, port: u16) -> Result<String, String> {
    // If an inspector is already open we should not signal the process at all —
    // a second SIGUSR1 is harmless on Node but pointless, and on a non-Node
    // process SIGUSR1 is very much not harmless (default disposition is fatal).
    if let Ok(url) = discover(port) {
        return Ok(url);
    }

    kill(Pid::from_raw(pid), Signal::SIGUSR1).map_err(|e| match e {
        nix::errno::Errno::ESRCH => format!("no process with pid {pid}"),
        nix::errno::Errno::EPERM => format!("not allowed to signal pid {pid} (try sudo)"),
        other => format!("could not signal pid {pid}: {other}"),
    })?;

    // The inspector binds asynchronously; poll rather than guess a sleep.
    for _ in 0..40 {
        sleep(Duration::from_millis(50));
        if let Ok(url) = discover(port) {
            return Ok(url);
        }
    }

    Err(format!(
        "signalled pid {pid} but no inspector appeared on port {port} within 2s.\n\
         If this is Bun or Deno, restart it with --inspect (SIGUSR1 only works on Node).\n\
         If the process already uses another port, pass --port."
    ))
}

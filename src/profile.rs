//! Driving V8's sampling profiler over the Chrome DevTools Protocol.
//!
//! The protocol is the same on Node, Bun and Deno, which is why one client
//! covers all three.

use serde_json::{json, Value};
use std::net::TcpStream;
use std::thread::sleep;
use std::time::Duration;
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

struct Cdp {
    ws: Socket,
    next_id: u64,
}

impl Cdp {
    fn connect(url: &str) -> Result<Self, String> {
        let (ws, _) = tungstenite::connect(url)
            .map_err(|e| format!("could not open debugger socket: {e}"))?;
        Ok(Self { ws, next_id: 1 })
    }

    /// Send a command and pump messages until its matching reply arrives.
    /// Events and replies to other commands are interleaved, so we filter by id.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let req = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(Message::Text(req.to_string()))
            .map_err(|e| format!("{method}: could not send: {e}"))?;

        loop {
            let msg = self
                .ws
                .read()
                .map_err(|e| format!("{method}: connection lost: {e}"))?;
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => {
                    return Err(format!("{method}: debugger closed the connection"))
                }
                _ => continue, // ping/pong/binary — not ours
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // an event, or another command's reply
            }
            if let Some(err) = v.get("error") {
                let m = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("{method}: {m}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// Attach, sample for `duration` seconds, and return the raw .cpuprofile.
pub fn capture(ws_url: &str, duration: u64, interval_us: u32) -> Result<Value, String> {
    let mut cdp = Cdp::connect(ws_url)?;

    // Bun runs on JavaScriptCore, which speaks WebKit's inspector protocol and
    // has no `Profiler` domain — detect that here rather than failing obscurely.
    cdp.call("Profiler.enable", json!({})).map_err(|e| {
        if e.contains("was not found") {
            "this runtime has no CDP Profiler domain.\n\
             Bun (JavaScriptCore) is not supported yet — flare currently profiles Node and Deno."
                .to_string()
        } else {
            e
        }
    })?;
    // Must be set before start; V8 ignores it mid-run.
    cdp.call(
        "Profiler.setSamplingInterval",
        json!({ "interval": interval_us }),
    )?;
    cdp.call("Profiler.start", json!({}))?;

    // Progress on one line so a long sample does not look like a hang.
    for elapsed in 0..duration {
        sleep(Duration::from_secs(1));
        eprint!("\r  {}s / {}s", elapsed + 1, duration);
    }
    eprintln!();

    let result = cdp.call("Profiler.stop", json!({}))?;
    let profile = result
        .get("profile")
        .cloned()
        .ok_or("profiler returned no profile")?;

    // An idle process yields a profile with no samples; say so rather than
    // rendering an empty page the user has to puzzle over.
    let samples = profile
        .get("samples")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if samples == 0 {
        return Err("captured 0 samples — the process was idle the whole time".into());
    }
    eprintln!("→ captured {samples} samples");

    Ok(profile)
}

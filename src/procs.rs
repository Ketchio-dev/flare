//! Finding the process to profile when the caller did not say.
//!
//! An agent invoking `flare` should not have to shell out to `ps` and parse it
//! first — that is our job, and getting it wrong is dangerous, because we
//! signal whatever we pick.

use std::process::Command;

struct Proc {
    pid: i32,
    cmd: String,
}

/// Every process whose executable is a JS runtime we can profile.
fn js_processes() -> Vec<Proc> {
    let out = match Command::new("ps")
        .args(["-A", "-o", "pid=,comm=,args="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let me = std::process::id() as i32;
    let text = String::from_utf8_lossy(&out.stdout);

    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let pid: i32 = pid.parse().ok()?;
            if pid == me {
                return None;
            }
            let rest = rest.trim_start();
            let (comm, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));

            // Match on the executable name only. Matching the whole command line
            // would catch `vim server.js` or a grep for "node" and we would then
            // send SIGUSR1 to it, which for most programs is fatal.
            let exe = comm.rsplit('/').next().unwrap_or(comm);
            let is_runtime = matches!(exe, "node" | "deno" | "node.exe" | "deno.exe");
            if !is_runtime {
                return None;
            }
            // Skip package managers and toolchain wrappers — profiling the
            // supervisor instead of the server is never what was meant.
            let a = args.to_lowercase();
            if ["npm/", "yarn", "pnpm", "npx", "corepack", "eslint", "tsc "]
                .iter()
                .any(|m| a.contains(m))
            {
                return None;
            }
            Some(Proc {
                pid,
                cmd: args.trim().to_string(),
            })
        })
        .collect()
}

/// The single JS process on this machine, or an error that tells the caller
/// exactly what to pass instead. We never guess between candidates: picking
/// wrong means signalling the wrong process.
pub fn sole_js_process() -> Result<i32, String> {
    let found = js_processes();
    match found.len() {
        0 => Err("no running Node or Deno process found — pass a pid explicitly".into()),
        1 => Ok(found[0].pid),
        _ => {
            let mut msg =
                String::from("several JS processes are running — pass the pid you want:\n");
            for p in found.iter().take(12) {
                let cmd: String = p.cmd.chars().take(90).collect();
                msg.push_str(&format!("  {:>7}  {}\n", p.pid, cmd));
            }
            if found.len() > 12 {
                msg.push_str(&format!("  … and {} more\n", found.len() - 12));
            }
            Err(msg.trim_end().to_string())
        }
    }
}

//! flare — attach to any running JavaScript process and get a flamegraph.
//!
//! Unlike existing profilers, flare does not need to launch your program.
//! It attaches to a process that is *already running*: no restart, no code
//! changes, no wrapper command.
//!
//! The output is meant to be read by a person *or* by a coding agent: a ranked
//! text summary goes to stdout, diagnostics go to stderr, and the browser only
//! opens when a human is actually watching.

use std::io::IsTerminal;
use std::process::ExitCode;

mod attach;
mod flame;
mod procs;
mod profile;

const USAGE: &str = "\
flare — attach to a running JS process and get a flamegraph

USAGE:
    flare                    profile the running Node process (if there's just one)
    flare <PID>              profile that process for 10s
    flare <PID> -d 30        profile for 30 seconds
    flare --port 9229        attach to an inspector that is already open
                             (use this for Deno started with --inspect)

OPTIONS:
    -d, --duration <SECS>    how long to sample        [default: 10]
    -i, --interval <US>      sampling interval in us   [default: 100]
    -o, --out <FILE>         flamegraph HTML path      [default: flare.html]
    -n, --top <N>            functions to list         [default: 12]
    -p, --port <PORT>        inspector port            [default: 9229]
    -u, --url <WS_URL>       connect to an exact ws:// URL
        --json               emit JSON instead of a text summary
        --no-html            skip the flamegraph, text only
        --open / --no-open   force the browser open or shut
                             (default: open only when stdout is a terminal)
    -h, --help               print this help

FOR AGENTS:
    flare --json             everything on stdout, nothing interactive
    flare 51234 --no-html    ranked hot functions as text, no file written
";

struct Args {
    pid: Option<i32>,
    duration: u64,
    interval: u32,
    out: String,
    top: usize,
    port: u16,
    url: Option<String>,
    json: bool,
    html: bool,
    open: Option<bool>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        pid: None,
        duration: 10,
        interval: 100,
        out: "flare.html".into(),
        top: 12,
        port: 9229,
        url: None,
        json: false,
        html: true,
        open: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].clone();
        let next = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i).cloned().ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-d" | "--duration" => a.duration = next(&mut i)?.parse().map_err(|_| "bad duration")?,
            "-i" | "--interval" => a.interval = next(&mut i)?.parse().map_err(|_| "bad interval")?,
            "-o" | "--out" => a.out = next(&mut i)?,
            "-n" | "--top" => a.top = next(&mut i)?.parse().map_err(|_| "bad --top")?,
            "-p" | "--port" => a.port = next(&mut i)?.parse().map_err(|_| "bad port")?,
            "-u" | "--url" => a.url = Some(next(&mut i)?),
            "--json" => a.json = true,
            "--no-html" => a.html = false,
            "--open" => a.open = Some(true),
            "--no-open" => a.open = Some(false),
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}")),
            s => a.pid = Some(s.parse().map_err(|_| format!("bad pid: {s}"))?),
        }
        i += 1;
    }
    Ok(a)
}

/// Human-readable summary. Goes to stdout so it can be piped or captured.
fn print_text(s: &flame::Summary, html: Option<&str>) {
    println!("\n{:.0} ms sampled · {} frames deep\n", s.wall_us / 1000.0, s.depth);
    println!("Hot functions (self time — where the CPU actually was):");
    let wall = s.wall_us.max(1.0);
    for f in &s.top {
        let loc = if f.location.is_empty() { "-" } else { &f.location };
        println!(
            "  {pct:5.1}%  {ms:8.1} ms  {name:<28} {loc}",
            pct = f.self_us / wall * 100.0,
            ms = f.self_us / 1000.0,
            name = f.name,
        );
    }
    if !s.hot_path.is_empty() {
        println!("\nHottest path (widest branch at each level):");
        for (d, f) in s.hot_path.iter().enumerate() {
            println!("  {}{}", "  ".repeat(d.min(12)), f);
        }
    }
    if let Some(path) = html {
        println!("\nFlamegraph: {path}");
    }
}

/// Machine-readable summary for agents and scripts.
fn print_json(s: &flame::Summary, html: Option<&str>) {
    let wall = s.wall_us.max(1.0);
    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    let top: Vec<serde_json::Value> = s
        .top
        .iter()
        .map(|f| {
            serde_json::json!({
                "function": f.name,
                "location": f.location,
                "self_ms": round1(f.self_us / 1000.0),
                "total_ms": round1(f.total_us / 1000.0),
                "self_pct": round1(f.self_us / wall * 100.0),
            })
        })
        .collect();
    let out = serde_json::json!({
        "sampled_ms": round1(s.wall_us / 1000.0),
        "depth": s.depth,
        "hot_functions": top,
        "hot_path": s.hot_path,
        "flamegraph": html,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    // Three ways in, most specific first: an exact URL, a pid to signal, or
    // "work it out yourself" — which is what an agent will reach for.
    let ws_url = match (args.url.clone(), args.pid) {
        (Some(url), _) => url,
        (None, Some(pid)) => attach::attach(pid, args.port)?,
        (None, None) => match attach::discover(args.port) {
            Ok(url) => url,
            Err(_) => {
                let pid = procs::sole_js_process()?;
                eprintln!("→ found one JS process: pid {pid}");
                attach::attach(pid, args.port)?
            }
        },
    };

    eprintln!("→ attached  ({ws_url})");
    eprintln!("→ sampling for {}s at {}µs …", args.duration, args.interval);

    let cpuprofile = profile::capture(&ws_url, args.duration, args.interval)?;

    let written = if args.html {
        let html = flame::render(&cpuprofile).map_err(|e| format!("could not build flamegraph: {e}"))?;
        std::fs::write(&args.out, html).map_err(|e| format!("could not write {}: {e}", args.out))?;
        Some(args.out.clone())
    } else {
        None
    };

    let summary = flame::summarize(&cpuprofile, args.top)?;
    if args.json {
        print_json(&summary, written.as_deref());
    } else {
        print_text(&summary, written.as_deref());
    }

    // Open only for a human at a terminal: an agent capturing stdout does not
    // want a browser window, and CI certainly does not.
    let open = args.open.unwrap_or_else(|| std::io::stdout().is_terminal() && !args.json);
    if open {
        if let Some(path) = &written {
            let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" })
                .arg(path)
                .status();
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flare: {e}");
            ExitCode::FAILURE
        }
    }
}

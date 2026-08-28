//! flare — attach to any running JavaScript process and get a flamegraph.
//!
//! Unlike existing profilers, flare does not need to launch your program.
//! It attaches to a process that is *already running*: no restart, no code
//! changes, no wrapper command.

use std::process::ExitCode;

mod attach;
mod flame;
mod profile;

const USAGE: &str = "\
flare — attach to a running JS process and get a flamegraph

USAGE:
    flare <PID>              profile a running Node process for 10s
    flare <PID> -d 30        profile for 30 seconds
    flare --port 9229        attach to an inspector that is already open
                             (use this for Bun/Deno started with --inspect)

OPTIONS:
    -d, --duration <SECS>    how long to sample        [default: 10]
    -i, --interval <US>      sampling interval in µs   [default: 100]
    -o, --out <FILE>         output HTML path          [default: flare.html]
    -p, --port <PORT>        inspector port            [default: 9229]
    -u, --url <WS_URL>       connect to an exact ws:// URL (Bun prints one)
        --no-open            do not open the browser
    -h, --help               print this help

EXAMPLES:
    flare 51234              # find the pid with `ps` or `lsof -i`
    flare 51234 -d 60 -o slow-request.html
";

struct Args {
    pid: Option<i32>,
    duration: u64,
    interval: u32,
    out: String,
    port: u16,
    url: Option<String>,
    open: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        pid: None,
        duration: 10,
        interval: 100,
        out: "flare.html".into(),
        port: 9229,
        url: None,
        open: true,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        // `next` pulls the value for a flag, erroring if it is missing.
        let next = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-d" | "--duration" => a.duration = next(&mut i)?.parse().map_err(|_| "bad duration")?,
            "-i" | "--interval" => a.interval = next(&mut i)?.parse().map_err(|_| "bad interval")?,
            "-o" | "--out" => a.out = next(&mut i)?,
            "-p" | "--port" => a.port = next(&mut i)?.parse().map_err(|_| "bad port")?,
            "-u" | "--url" => a.url = Some(next(&mut i)?),
            "--no-open" => a.open = false,
            _ if arg.starts_with('-') => return Err(format!("unknown flag: {arg}")),
            _ => a.pid = Some(arg.parse().map_err(|_| format!("bad pid: {arg}"))?),
        }
        i += 1;
    }
    Ok(a)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    // Two ways in: signal a Node process to open its inspector, or connect to
    // an inspector the user already opened (Bun/Deno, or `node --inspect`).
    let ws_url = match (args.url.clone(), args.pid) {
        (Some(url), _) => url,
        (None, Some(pid)) => attach::attach(pid, args.port)?,
        (None, None) => attach::discover(args.port)
            .map_err(|e| format!("{e}\n\nNo pid given and nothing is listening on port {}.\nPass a pid, or start your process with --inspect.", args.port))?,
    };

    eprintln!("→ attached  ({ws_url})");
    eprintln!("→ sampling for {}s at {}µs …", args.duration, args.interval);

    let cpuprofile = profile::capture(&ws_url, args.duration, args.interval)?;
    let html = flame::render(&cpuprofile).map_err(|e| format!("could not build flamegraph: {e}"))?;

    std::fs::write(&args.out, html).map_err(|e| format!("could not write {}: {e}", args.out))?;
    eprintln!("→ wrote {}", args.out);

    if args.open {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" })
            .arg(&args.out)
            .status();
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

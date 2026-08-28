# flare

**Profile a JavaScript process that is already running.** No restart, no code changes, no wrapper command.

```bash
flare 51234
```

That's it. flare attaches to the live process, samples it, and opens a flamegraph in your browser.

![flare flamegraph](assets/demo.png)

*A live Node HTTP server. `priceItem` is 54% of the sampled window — six frames
deep inside a request handler, which is exactly the sort of thing you do not see
until you look.*

---

## Why

Every other Node profiler wants to *launch* your program:

```bash
0x -- node server.js          # relaunch
node --cpu-prof server.js     # relaunch
clinic flame -- node app.js   # relaunch
```

That is fine on your laptop and useless when it matters. The process that is
burning CPU right now is the one you cannot restart — it is serving traffic, it
took forty minutes to reach this state, or the bug only shows up after two days
of uptime. By the time you relaunch it under a profiler, the thing you wanted
to see is gone.

flare attaches to the process as it is.

## Install

```bash
cargo install --git https://github.com/Ketchio-dev/flare
```

Or grab a binary from [releases](../../releases). It is one 1.4 MB file with no
runtime dependencies — no Node module to install, nothing added to your project.

## Use

```bash
flare 51234                        # sample for 10s, open the flamegraph
flare 51234 -d 60                  # sample for a minute
flare 51234 -o slow-checkout.html  # name the output
flare --port 9229                  # attach to an inspector already listening
flare --url ws://127.0.0.1:9229/…  # or to an exact websocket URL
```

Find the pid however you normally would — `ps aux | grep node`, `lsof -i :3000`,
your process manager, `docker top`.

```
→ attached  (ws://127.0.0.1:9229/d1812a8c-…)
→ sampling for 5s at 100µs …
  5s / 5s
→ captured 38404 samples
→ wrote flare.html
```

The output is a **single self-contained HTML file**. No viewer to install, no
server to run, no upload. The example above is 10 KB — small enough to attach to
an issue or drop in a Slack thread, and it still works on the other person's
machine.

## How it works

Node opens its inspector when it receives `SIGUSR1`. That is the whole trick:
flare signals the process, waits for the inspector to bind, then drives V8's
sampling profiler over the Chrome DevTools Protocol and renders the resulting
`.cpuprofile` as an SVG flamegraph.

Self time comes from the sample stream (`samples` + `timeDeltas`) rather than
`hitCount`, because V8's samples are not evenly spaced and the deltas are the
honest measure.

## Runtime support

| Runtime | Attach to a running process | With `--inspect` |
| ------- | --------------------------- | ---------------- |
| Node    | **yes** — `flare <pid>`     | yes              |
| Deno    | no (no signal handler)      | **yes** — `flare --port 9229` |
| Bun     | not yet                     | not yet          |

Bun runs on JavaScriptCore, which speaks WebKit's inspector protocol and has no
CDP `Profiler` domain, so flare cannot profile it yet. Support means
implementing the `ScriptProfiler` domain separately — it is on the list.

## Reading the flamegraph

Width is time. The wider a frame, the more of the sampled window was spent in it
and everything it called. Depth is call stack, not cost — a tall narrow tower is
cheap, a short wide slab is where your time goes.

Type in the search box to highlight matching functions; the status line totals
how much time they account for. Hover any frame for its file, line, total and
self time.

## Caveats

- **Sampling, not tracing.** Work shorter than the sampling interval can be
  missed entirely. Lower `-i` to catch more, at the cost of overhead.
- **Signalling the wrong process is not free.** `SIGUSR1` terminates most
  programs that do not handle it. flare only signals the pid you give it — make
  sure it is the JavaScript one.
- **The inspector is a debugger port.** While it is open, anything that can
  reach it can execute code in your process. It binds to `127.0.0.1` and Node
  keeps it open until the process exits.
- **Native frames are not resolved.** Time inside C++ addons shows up as
  `(program)` rather than a named stack.

## License

MIT

//! Turning a V8 .cpuprofile into a self-contained HTML flamegraph.
//!
//! No CDN, no bundler, no viewer to install — the output is one file you can
//! mail to someone and they can open it.

use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;

struct Frame {
    name: String,
    location: String,
    children: Vec<u64>,
    self_us: f64,
    total_us: f64,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Warm flamegraph palette. Hashing the name keeps a function the same colour
/// across runs, which matters when comparing two profiles side by side.
fn colour(name: &str) -> String {
    let mut h: u32 = 2166136261;
    for b in name.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = 8 + (h % 44); // red → amber
    let sat = 62 + (h >> 8) % 20;
    let light = 46 + (h >> 16) % 14;
    format!("hsl({hue},{sat}%,{light}%)")
}

fn parse(profile: &Value) -> Result<(HashMap<u64, Frame>, u64), String> {
    let nodes = profile
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("profile has no nodes")?;

    let mut frames: HashMap<u64, Frame> = HashMap::with_capacity(nodes.len());
    let mut root_id = None;

    for n in nodes {
        let id = n.get("id").and_then(Value::as_u64).ok_or("node without id")?;
        let cf = n.get("callFrame").ok_or("node without callFrame")?;

        let raw_name = cf.get("functionName").and_then(Value::as_str).unwrap_or("");
        let url = cf.get("url").and_then(Value::as_str).unwrap_or("");
        let line = cf.get("lineNumber").and_then(Value::as_i64).unwrap_or(-1);

        // V8 leaves the name empty for anonymous functions and top-level code.
        let name = if raw_name.is_empty() {
            if url.is_empty() { "(anonymous)".to_string() } else { "(top level)".to_string() }
        } else {
            raw_name.to_string()
        };

        let location = if url.is_empty() {
            String::new()
        } else {
            // Full paths make tooltips unreadable; the tail is what identifies the file.
            let short = url.rsplit('/').next().unwrap_or(url);
            if line >= 0 { format!("{short}:{}", line + 1) } else { short.to_string() }
        };

        let children = n
            .get("children")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();

        if root_id.is_none() {
            root_id = Some(id);
        }
        frames.insert(id, Frame { name, location, children, self_us: 0.0, total_us: 0.0 });
    }

    let root_id = root_id.ok_or("profile has no nodes")?;

    // Self time from the sample stream. This is more faithful than hitCount
    // because samples are not evenly spaced — V8 tells us the real deltas.
    let samples = profile.get("samples").and_then(Value::as_array);
    let deltas = profile.get("timeDeltas").and_then(Value::as_array);
    if let (Some(samples), Some(deltas)) = (samples, deltas) {
        for (i, s) in samples.iter().enumerate() {
            let Some(id) = s.as_u64() else { continue };
            let dt = deltas.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            if let Some(f) = frames.get_mut(&id) {
                // Negative deltas show up occasionally on clock adjustment.
                f.self_us += dt.max(0.0);
            }
        }
    } else {
        // Older profiles only carry hitCount; approximate with the interval.
        for n in nodes {
            let (Some(id), Some(hits)) = (
                n.get("id").and_then(Value::as_u64),
                n.get("hitCount").and_then(Value::as_f64),
            ) else {
                continue;
            };
            if let Some(f) = frames.get_mut(&id) {
                f.self_us += hits * 100.0;
            }
        }
    }

    // Totals, post-order without recursion so a pathological tree cannot
    // blow the stack.
    let mut stack = vec![(root_id, false)];
    while let Some((id, expanded)) = stack.pop() {
        if expanded {
            let kids: Vec<u64> = frames.get(&id).map(|f| f.children.clone()).unwrap_or_default();
            let sum: f64 = kids.iter().filter_map(|k| frames.get(k)).map(|f| f.total_us).sum();
            if let Some(f) = frames.get_mut(&id) {
                f.total_us = f.self_us + sum;
            }
        } else {
            stack.push((id, true));
            if let Some(f) = frames.get(&id) {
                for c in &f.children {
                    stack.push((*c, false));
                }
            }
        }
    }

    Ok((frames, root_id))
}

const WIDTH: f64 = 1200.0;
const ROW: f64 = 17.0;

pub fn render(profile: &Value) -> Result<String, String> {
    let (frames, root_id) = parse(profile)?;
    let root_total = frames.get(&root_id).map_or(0.0, |f| f.total_us);
    if root_total <= 0.0 {
        return Err("profile contains no measurable time".into());
    }

    // Lay out iteratively: each entry is (node, x offset, depth).
    let mut rects = String::new();
    let mut max_depth: usize = 0;
    let mut stack = vec![(root_id, 0.0f64, 0usize)];

    while let Some((id, x, depth)) = stack.pop() {
        let Some(f) = frames.get(&id) else { continue };
        let w = f.total_us / root_total * WIDTH;
        // Sub-pixel frames are invisible and would bloat the file enormously.
        if w < 0.35 {
            continue;
        }
        max_depth = max_depth.max(depth);
        let y = depth as f64 * ROW;
        let pct = f.total_us / root_total * 100.0;
        let ms = f.total_us / 1000.0;
        let self_ms = f.self_us / 1000.0;

        let label = escape(&f.name);
        let loc = escape(&f.location);
        let title = if loc.is_empty() {
            format!("{label}\n{ms:.1} ms  ({pct:.2}%)  self {self_ms:.1} ms")
        } else {
            format!("{label}\n{loc}\n{ms:.1} ms  ({pct:.2}%)  self {self_ms:.1} ms")
        };

        let _ = write!(
            rects,
            r#"<g class="f" data-n="{label}" data-l="{loc}" data-ms="{ms:.1}" data-pct="{pct:.2}"><rect x="{x:.2}" y="{y:.1}" width="{w:.2}" height="{h:.1}" fill="{c}"/><title>{title}</title>"#,
            h = ROW - 1.0,
            c = colour(&f.name),
        );
        // Only label frames wide enough to fit readable text.
        if w > 26.0 {
            let budget = ((w - 6.0) / 6.2) as usize;
            let text: String = if label.chars().count() > budget {
                label.chars().take(budget.saturating_sub(1)).collect::<String>() + "…"
            } else {
                label.clone()
            };
            let _ = write!(
                rects,
                r#"<text x="{tx:.2}" y="{ty:.1}">{text}</text>"#,
                tx = x + 3.0,
                ty = y + ROW - 5.0,
            );
        }
        rects.push_str("</g>");

        // Children laid out left to right, widest first so the eye lands on
        // the expensive path immediately.
        let mut kids: Vec<u64> = f.children.clone();
        kids.sort_by(|a, b| {
            let ta = frames.get(a).map_or(0.0, |f| f.total_us);
            let tb = frames.get(b).map_or(0.0, |f| f.total_us);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut cx = x;
        // Push in reverse so the widest child is popped (and drawn) first.
        let placed: Vec<(u64, f64)> = kids
            .iter()
            .map(|k| {
                let kx = cx;
                cx += frames.get(k).map_or(0.0, |f| f.total_us) / root_total * WIDTH;
                (*k, kx)
            })
            .collect();
        for (k, kx) in placed.into_iter().rev() {
            stack.push((k, kx, depth + 1));
        }
    }

    let height = (max_depth + 1) as f64 * ROW + 4.0;
    let total_ms = root_total / 1000.0;

    Ok(format!(
        r##"<!doctype html>
<meta charset="utf-8">
<title>flare — flamegraph</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin:0; background:#15161a; color:#e6e6e6;
         font:13px/1.45 ui-sans-serif,-apple-system,"Segoe UI",Roboto,sans-serif; }}
  header {{ padding:14px 18px 10px; border-bottom:1px solid #2a2c33; }}
  h1 {{ margin:0; font-size:14px; font-weight:600; letter-spacing:.02em; }}
  h1 span {{ color:#8b8f9a; font-weight:400; }}
  #search {{ margin-top:9px; padding:5px 9px; width:280px; background:#1e2027;
            border:1px solid #34373f; border-radius:5px; color:inherit; font:inherit; }}
  #search:focus {{ outline:none; border-color:#e07b3c; }}
  #wrap {{ overflow-x:auto; padding:12px 18px 22px; }}
  svg {{ display:block; }}
  .f rect {{ stroke:#15161a; stroke-width:.5; cursor:pointer; }}
  .f:hover rect {{ stroke:#fff; stroke-width:1; }}
  .f text {{ fill:#121316; font:10.5px ui-monospace,SFMono-Regular,Menlo,monospace;
            pointer-events:none; }}
  .dim rect {{ opacity:.16; }}
  .dim text {{ opacity:.3; }}
  #status {{ padding:0 18px 16px; color:#8b8f9a; font-size:12px; min-height:1.4em; }}
</style>
<header>
  <h1>flare <span>— {total_ms:.0} ms sampled · {depth} frames deep</span></h1>
  <input id="search" placeholder="highlight functions…" autocomplete="off">
</header>
<div id="wrap">
<svg width="{WIDTH}" height="{height:.0}" viewBox="0 0 {WIDTH} {height:.0}">{rects}</svg>
</div>
<div id="status">Hover a frame for details. Type to highlight matching functions.</div>
<script>
const groups = [...document.querySelectorAll('.f')];
const status = document.getElementById('status');
document.getElementById('search').addEventListener('input', e => {{
  const q = e.target.value.trim().toLowerCase();
  if (!q) {{
    groups.forEach(g => g.classList.remove('dim'));
    status.textContent = 'Hover a frame for details. Type to highlight matching functions.';
    return;
  }}
  let hits = 0, ms = 0;
  for (const g of groups) {{
    const match = (g.dataset.n + ' ' + g.dataset.l).toLowerCase().includes(q);
    g.classList.toggle('dim', !match);
    if (match) {{ hits++; ms += parseFloat(g.dataset.ms); }}
  }}
  status.textContent = hits
    ? hits + ' frame' + (hits === 1 ? '' : 's') + ' matched · ' + ms.toFixed(1) + ' ms total'
    : 'no match';
}});
for (const g of groups) {{
  g.addEventListener('mouseenter', () => {{
    status.textContent = g.dataset.n + (g.dataset.l ? '  ' + g.dataset.l : '')
      + '  —  ' + g.dataset.ms + ' ms (' + g.dataset.pct + '%)';
  }});
}}
</script>
"##,
        depth = max_depth + 1,
    ))
}

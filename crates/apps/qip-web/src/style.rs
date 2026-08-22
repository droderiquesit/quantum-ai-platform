//! The stylesheet.
//!
//! One stylesheet, served from the same origin, referenced by every page. It
//! is inlined into a `<style>` element rather than fetched, because the
//! content-security policy allows `style-src 'self'` and a single small
//! stylesheet is not worth a second round trip.
//!
//! The palette is chosen for a screen someone is watching at three in the
//! morning: a dark ground, restrained colour, and red reserved exclusively for
//! a halt or a breach so that seeing red always means the same thing.

/// The stylesheet, as CSS.
pub const STYLESHEET: &str = r#"
:root {
  --ground: #0f1115;
  --panel: #171a21;
  --line: #262b36;
  --text: #dfe3ea;
  --muted: #8b93a3;
  --accent: #6ea8fe;
  --good: #4ec9a5;
  --warn: #e0b341;
  --bad: #e05c5c;
  --mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  background: var(--ground);
  color: var(--text);
  font: 15px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif;
}
header {
  border-bottom: 1px solid var(--line);
  padding: 0.9rem 1.5rem;
  display: flex;
  align-items: baseline;
  gap: 1.5rem;
  flex-wrap: wrap;
}
header h1 { font-size: 1rem; font-weight: 600; margin: 0; letter-spacing: 0.02em; }
nav { display: flex; gap: 1rem; flex-wrap: wrap; }
nav a { color: var(--muted); text-decoration: none; font-size: 0.9rem; }
nav a:hover, nav a.current { color: var(--accent); }
main { padding: 1.5rem; max-width: 1200px; }
section { margin-bottom: 2rem; }
h2 { font-size: 1.05rem; font-weight: 600; margin: 0 0 0.75rem; }
h3 { font-size: 0.9rem; font-weight: 600; color: var(--muted); margin: 1.25rem 0 0.5rem; }
p { margin: 0 0 0.75rem; }
.muted { color: var(--muted); }
.mono { font-family: var(--mono); font-size: 0.85rem; }
table { width: 100%; border-collapse: collapse; font-size: 0.88rem; }
th, td {
  text-align: left;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--line);
  vertical-align: top;
}
th { color: var(--muted); font-weight: 500; font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.04em; }
tbody tr:hover { background: var(--panel); }
.cards { display: flex; gap: 1rem; flex-wrap: wrap; }
.card {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 0.9rem 1.1rem;
  min-width: 11rem;
}
.card .label { color: var(--muted); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; }
.card .value { font-size: 1.5rem; font-weight: 600; font-family: var(--mono); margin-top: 0.2rem; }
.banner {
  border-radius: 6px;
  padding: 0.75rem 1.1rem;
  margin-bottom: 1.5rem;
  border: 1px solid;
}
.banner.paper { border-color: var(--accent); background: rgba(110,168,254,0.08); }
.banner.halted { border-color: var(--bad); background: rgba(224,92,92,0.12); }
.banner.live { border-color: var(--warn); background: rgba(224,179,65,0.12); }
.pill {
  display: inline-block;
  padding: 0.1rem 0.5rem;
  border-radius: 999px;
  font-size: 0.75rem;
  font-family: var(--mono);
  border: 1px solid var(--line);
}
.pill.good { color: var(--good); border-color: var(--good); }
.pill.warn { color: var(--warn); border-color: var(--warn); }
.pill.bad { color: var(--bad); border-color: var(--bad); }
.empty { color: var(--muted); font-style: italic; padding: 1rem 0; }
footer { color: var(--muted); font-size: 0.8rem; padding: 1.5rem; border-top: 1px solid var(--line); }
"#;

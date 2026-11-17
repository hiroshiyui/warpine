// SPDX-License-Identifier: GPL-3.0-only
//
// gen_arch — generate Warpine's architecture diagram from its layer map.
//
// The diagram has two sources: `doc/architecture/layers.toml`, written by
// hand (zones, layers and the modules in each, external parts, flows), and
// the list of modules under `src/`, which this tool reads itself. It
// refuses a layer map that misses a module, names one twice or names one
// that does not exist, so the diagram cannot silently drift from the code.
//
//   gen_arch check    validate, and compare doc/architecture/warpine.dot
//                     with what the map and src/ generate (exit 1 if not)
//   gen_arch dot      print the generated DOT
//   gen_arch render   write warpine.dot and run Graphviz `dot` for
//                     warpine.svg and warpine.png
//
// The layer map is a subset of TOML: [[zone]], [[layer]], [[external]] and
// [[flow]] tables, `key = "string"` and `modules = ["…", …]` (one line or
// several), `#` comments. See ADR-0021.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The layer map, relative to the crate root.
const LAYER_MAP: &str = "doc/architecture/layers.toml";
/// The generated diagram files, relative to the crate root.
const DOT_FILE: &str = "doc/architecture/warpine.dot";
const SVG_FILE: &str = "doc/architecture/warpine.svg";
const PNG_FILE: &str = "doc/architecture/warpine.png";
/// Raster resolution of the PNG.
const PNG_DPI: &str = "96";
/// Modules listed per line inside a layer node.
const MODULES_PER_LINE: usize = 4;

// ── Model ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct Zone {
    id: String,
    label: String,
    /// `"min"` or `"max"`: put the zone's nodes in the top or bottom rank.
    rank: Option<String>,
}

/// A layer of Warpine (with modules) or an external part (without).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    id: String,
    zone: String,
    label: String,
    modules: Vec<String>,
    external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Flow {
    from: String,
    to: String,
    label: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct LayerMap {
    zones: Vec<Zone>,
    nodes: Vec<Node>,
    flows: Vec<Flow>,
}

// ── Parser (the TOML subset) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Value {
    Str(String),
    List(Vec<String>),
}

/// One `[[kind]]` table: its keys in order, and the line it starts on.
struct Table {
    kind: String,
    line: usize,
    keys: BTreeMap<String, Value>,
}

/// Remove a `#` comment that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match c {
            '\\' if in_str && !escaped => { escaped = true; continue; }
            '"' if !escaped => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
        escaped = false;
    }
    line
}

/// The double-quoted strings in `s`, in order (`\"` and `\\` escapes).
fn quoted_strings(s: &str, line: usize) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut v = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '\\')) => v.push(e),
                            other => return Err(format!("line {line}: unsupported escape \\{}", other.unwrap_or(' '))),
                        },
                        Some(ch) => v.push(ch),
                        None => return Err(format!("line {line}: unterminated string")),
                    }
                }
                out.push(v);
            }
            ',' | '[' | ']' | ' ' | '\t' => {}
            other => return Err(format!("line {line}: unexpected {other:?} (only strings are allowed here)")),
        }
    }
    Ok(out)
}

fn parse_tables(text: &str) -> Result<Vec<Table>, String> {
    let mut tables: Vec<Table> = Vec::new();
    let mut lines = text.lines().enumerate().map(|(i, l)| (i + 1, l));
    while let Some((n, raw)) = lines.next() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(kind) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            tables.push(Table { kind: kind.trim().to_owned(), line: n, keys: BTreeMap::new() });
            continue;
        }
        let (key, rest) = line.split_once('=').ok_or(format!("line {n}: expected `key = value`"))?;
        let key = key.trim();
        let rest = rest.trim();
        let table = tables.last_mut().ok_or(format!("line {n}: `{key}` outside a [[table]]"))?;
        let value = if rest.starts_with('[') {
            // An array, possibly over several lines, up to the closing `]`.
            let mut body = rest.to_owned();
            while !strip_comment(&body).trim_end().ends_with(']') {
                let (_, more) = lines.next().ok_or(format!("line {n}: unterminated array `{key}`"))?;
                body.push(' ');
                body.push_str(strip_comment(more).trim());
            }
            Value::List(quoted_strings(strip_comment(&body), n)?)
        } else {
            let strings = quoted_strings(rest, n)?;
            match strings.as_slice() {
                [s] if rest.starts_with('"') && rest.ends_with('"') => Value::Str(s.clone()),
                _ => return Err(format!("line {n}: `{key}` must be one string")),
            }
        };
        if table.keys.insert(key.to_owned(), value).is_some() {
            return Err(format!("line {n}: `{key}` given twice"));
        }
    }
    Ok(tables)
}

fn parse_layer_map(text: &str) -> Result<LayerMap, String> {
    let mut map = LayerMap::default();
    for t in parse_tables(text)? {
        let at = t.line;
        let get = |k: &str| -> Result<String, String> {
            match t.keys.get(k) {
                Some(Value::Str(s)) => Ok(s.clone()),
                Some(Value::List(_)) => Err(format!("line {at}: `{k}` must be a string")),
                None => Err(format!("line {at}: [[{}]] needs `{k}`", t.kind)),
            }
        };
        let allowed: &[&str] = match t.kind.as_str() {
            "zone" => &["id", "label", "rank"],
            "layer" => &["id", "zone", "label", "modules"],
            "external" => &["id", "zone", "label"],
            "flow" => &["from", "to", "label"],
            other => return Err(format!("line {at}: unknown table [[{other}]]")),
        };
        if let Some(k) = t.keys.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(format!("line {at}: unknown key `{k}` in [[{}]]", t.kind));
        }
        match t.kind.as_str() {
            "zone" => map.zones.push(Zone { id: get("id")?, label: get("label")?, rank: get("rank").ok() }),
            "layer" => {
                let modules = match t.keys.get("modules") {
                    Some(Value::List(m)) => m.clone(),
                    _ => return Err(format!("line {at}: [[layer]] needs `modules = [...]`")),
                };
                map.nodes.push(Node { id: get("id")?, zone: get("zone")?, label: get("label")?, modules, external: false });
            }
            "external" => map.nodes.push(Node {
                id: get("id")?, zone: get("zone")?, label: get("label")?, modules: Vec::new(), external: true,
            }),
            _ => map.flows.push(Flow { from: get("from")?, to: get("to")?, label: get("label").ok() }),
        }
    }
    Ok(map)
}

// ── Validation against src/ ──────────────────────────────────────────────────

/// Every `.rs` file under `src`, as a path relative to it with `/`.
fn source_modules(src: &Path) -> Result<BTreeSet<String>, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                walk(&path, root, out)?;
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
                out.insert(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
            }
        }
        Ok(())
    }
    let mut out = BTreeSet::new();
    walk(src, src, &mut out)?;
    Ok(out)
}

/// All problems of `map` against the modules in `sources`; empty when valid.
fn validate(map: &LayerMap, sources: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ids = BTreeSet::new();
    for id in map.zones.iter().map(|z| &z.id).chain(map.nodes.iter().map(|n| &n.id)) {
        if !ids.insert(id.as_str()) {
            problems.push(format!("id `{id}` is used twice"));
        }
    }
    let zones: BTreeSet<&str> = map.zones.iter().map(|z| z.id.as_str()).collect();
    let nodes: BTreeSet<&str> = map.nodes.iter().map(|n| n.id.as_str()).collect();
    for z in &map.zones {
        if let Some(r) = &z.rank && r != "min" && r != "max" {
            problems.push(format!("zone `{}` has rank `{r}`: use \"min\" or \"max\"", z.id));
        }
    }
    let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
    for n in &map.nodes {
        if !zones.contains(n.zone.as_str()) {
            problems.push(format!("`{}` is in unknown zone `{}`", n.id, n.zone));
        }
        if !n.external && n.modules.is_empty() {
            problems.push(format!("layer `{}` has no modules", n.id));
        }
        for m in &n.modules {
            if !sources.contains(m) {
                problems.push(format!("layer `{}` names `{m}`, which is not a module under src/", n.id));
            }
            if let Some(first) = owner.insert(m.as_str(), n.id.as_str()) {
                problems.push(format!("`{m}` is in two layers: `{first}` and `{}`", n.id));
            }
        }
    }
    for m in sources {
        if !owner.contains_key(m.as_str()) {
            problems.push(format!("`{m}` is in no layer: add it to {LAYER_MAP}"));
        }
    }
    for f in &map.flows {
        for end in [&f.from, &f.to] {
            if !nodes.contains(end.as_str()) {
                problems.push(format!("flow {} -> {} names unknown node `{end}`", f.from, f.to));
            }
        }
    }
    problems
}

// ── DOT ──────────────────────────────────────────────────────────────────────

/// Fill colour of each zone's box and of the nodes in it.
fn zone_colours(zone: &str) -> (&'static str, &'static str) {
    match zone {
        "guest" => ("#fff8e1", "#ffe082"),
        "host" => ("#e3f2fd", "#bbdefb"),
        "build" => ("#f1f8e9", "#dcedc8"),
        _ => ("#f5f5f5", "#e0e0e0"),
    }
}

/// Text for a Graphviz HTML-like label.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Text for a quoted DOT string.
fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The diagram of `map` in Graphviz DOT; deterministic.
fn to_dot(map: &LayerMap) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "// Generated by `cargo run --bin gen_arch -- render` from {LAYER_MAP} and src/.");
    let _ = writeln!(out, "// Do not edit: change the layer map and render again.");
    out.push_str("digraph warpine {\n");
    out.push_str("  graph [rankdir=TB, fontname=\"Helvetica\", fontsize=14, compound=true, newrank=true, nodesep=0.35, ranksep=0.5];\n");
    out.push_str("  node [shape=box, style=\"rounded,filled\", fontname=\"Helvetica\", fontsize=11, margin=\"0.15,0.08\"];\n");
    out.push_str("  edge [fontname=\"Helvetica\", fontsize=9, color=\"#455a64\", fontcolor=\"#37474f\"];\n");
    for zone in &map.zones {
        let (bg, fill) = zone_colours(&zone.id);
        let _ = writeln!(out, "  subgraph cluster_{} {{", zone.id);
        let _ = writeln!(out, "    label=\"{}\"; style=\"rounded,filled\"; fillcolor=\"{bg}\"; color=\"#90a4ae\";", dot_escape(&zone.label));
        for n in map.nodes.iter().filter(|n| n.zone == zone.id) {
            if n.external {
                let _ = writeln!(out, "    {} [label=\"{}\", shape=component, fillcolor=\"#ffffff\"];", n.id, dot_escape(&n.label));
                continue;
            }
            let mut label = format!("<<B>{}</B>", html_escape(&n.label));
            for chunk in n.modules.chunks(MODULES_PER_LINE) {
                let line = chunk.iter().map(|m| html_escape(m)).collect::<Vec<_>>().join(", ");
                let _ = write!(label, "<BR/><FONT POINT-SIZE=\"8\" COLOR=\"#37474f\">{line}</FONT>");
            }
            label.push('>');
            let _ = writeln!(out, "    {} [label={label}, fillcolor=\"{fill}\"];", n.id);
        }
        out.push_str("  }\n");
    }
    for zone in &map.zones {
        let Some(rank) = &zone.rank else { continue };
        let ids: Vec<&str> = map.nodes.iter().filter(|n| n.zone == zone.id).map(|n| n.id.as_str()).collect();
        if !ids.is_empty() {
            let _ = writeln!(out, "  {{ rank={rank}; {}; }}", ids.join("; "));
        }
    }
    for f in &map.flows {
        match &f.label {
            Some(l) => { let _ = writeln!(out, "  {} -> {} [label=\"{}\"];", f.from, f.to, dot_escape(l)); }
            None => { let _ = writeln!(out, "  {} -> {};", f.from, f.to); }
        }
    }
    out.push_str("}\n");
    out
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read, parse and validate the layer map; the generated DOT.
fn generate(root: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join(LAYER_MAP)).map_err(|e| format!("{LAYER_MAP}: {e}"))?;
    let map = parse_layer_map(&text).map_err(|e| format!("{LAYER_MAP}: {e}"))?;
    let problems = validate(&map, &source_modules(&root.join("src"))?);
    if !problems.is_empty() {
        return Err(format!("{LAYER_MAP} does not match src/:\n  {}", problems.join("\n  ")));
    }
    Ok(to_dot(&map))
}

fn run_dot(root: &Path, format: &str, output: &str, extra: &[&str]) -> Result<(), String> {
    let status = Command::new("dot")
        .arg(format!("-T{format}"))
        .args(extra)
        .arg("-o").arg(root.join(output))
        .arg(root.join(DOT_FILE))
        .status()
        .map_err(|e| format!("cannot run Graphviz `dot` ({e}); install graphviz"))?;
    if status.success() { Ok(()) } else { Err(format!("`dot -T{format}` failed: {status}")) }
}

fn main() -> ExitCode {
    let root = root();
    let cmd = std::env::args().nth(1).unwrap_or_default();
    let result = match cmd.as_str() {
        "dot" => generate(&root).map(|dot| print!("{dot}")),
        "check" => generate(&root).and_then(|dot| {
            let committed = std::fs::read_to_string(root.join(DOT_FILE)).unwrap_or_default();
            if committed == dot {
                println!("✓  {LAYER_MAP} matches src/, and {DOT_FILE} is up to date");
                Ok(())
            } else {
                Err(format!("{DOT_FILE} is out of date: run `cargo run --bin gen_arch -- render`"))
            }
        }),
        "render" => generate(&root).and_then(|dot| {
            std::fs::write(root.join(DOT_FILE), dot).map_err(|e| format!("{DOT_FILE}: {e}"))?;
            run_dot(&root, "svg", SVG_FILE, &[])?;
            run_dot(&root, "png", PNG_FILE, &[&format!("-Gdpi={PNG_DPI}")])?;
            println!("wrote {DOT_FILE}, {SVG_FILE}, {PNG_FILE}");
            Ok(())
        }),
        _ => Err("usage: gen_arch check | dot | render".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("error: {e}"); ExitCode::FAILURE }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = r#"
# a comment
[[zone]]
id = "host"   # trailing comment
label = "Host # not a comment"

[[layer]]
id = "a"
zone = "host"
label = "Layer <A> & co"
modules = [
  "main.rs",
  "lib.rs",   # comment in an array
]

[[external]]
id = "x"
zone = "host"
label = "Outside"

[[flow]]
from = "a"
to = "x"
label = "calls \"x\""
"#;

    fn sources(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_toml_subset() {
        let map = parse_layer_map(SMALL).unwrap();
        assert_eq!(map.zones, vec![Zone { id: "host".into(), label: "Host # not a comment".into(), rank: None }]);
        assert_eq!(map.nodes[0].modules, vec!["main.rs", "lib.rs"]);
        assert!(map.nodes[1].external);
        assert_eq!(map.flows[0].label.as_deref(), Some("calls \"x\""));
    }

    #[test]
    fn rejects_malformed_maps() {
        for (text, what) in [
            ("[[zone]]\nid = \"z\"\n", "missing label"),
            ("[[zone]]\nid = z\nlabel = \"L\"\n", "unquoted value"),
            ("[[thing]]\nid = \"t\"\n", "unknown table"),
            ("[[zone]]\nid = \"z\"\nlabel = \"L\"\ncolour = \"red\"\n", "unknown key"),
            ("id = \"z\"\n", "key outside a table"),
            ("[[layer]]\nid = \"a\"\nzone = \"z\"\nlabel = \"L\"\nmodules = [\"x\"\n", "unterminated array"),
            ("[[zone]]\nid = \"z\"\nid = \"y\"\nlabel = \"L\"\n", "duplicate key"),
        ] {
            assert!(parse_layer_map(text).is_err(), "{what}");
        }
    }

    #[test]
    fn validation_finds_every_mismatch_with_src() {
        let map = parse_layer_map(SMALL).unwrap();
        assert!(validate(&map, &sources(&["main.rs", "lib.rs"])).is_empty());
        let problems = validate(&map, &sources(&["main.rs", "new.rs"]));
        assert!(problems.iter().any(|p| p.contains("`lib.rs`, which is not a module")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("`new.rs` is in no layer")), "{problems:?}");

        let mut twice = map.clone();
        twice.nodes[1] = Node { external: false, modules: vec!["main.rs".into()], ..twice.nodes[1].clone() };
        assert!(validate(&twice, &sources(&["main.rs", "lib.rs"])).iter().any(|p| p.contains("in two layers")));

        let mut bad_flow = map.clone();
        bad_flow.flows[0].to = "nowhere".into();
        assert!(validate(&bad_flow, &sources(&["main.rs", "lib.rs"])).iter().any(|p| p.contains("unknown node `nowhere`")));

        let mut bad_rank = map.clone();
        bad_rank.zones[0].rank = Some("middle".into());
        assert!(validate(&bad_rank, &sources(&["main.rs", "lib.rs"])).iter().any(|p| p.contains("use \"min\"")));

        let mut bad_zone = map;
        bad_zone.nodes[0].zone = "mars".into();
        assert!(validate(&bad_zone, &sources(&["main.rs", "lib.rs"])).iter().any(|p| p.contains("unknown zone")));
    }

    #[test]
    fn dot_escapes_labels() {
        let dot = to_dot(&parse_layer_map(SMALL).unwrap());
        assert!(dot.contains("<B>Layer &lt;A&gt; &amp; co</B>"), "{dot}");
        assert!(dot.contains("a -> x [label=\"calls \\\"x\\\"\"];"), "{dot}");
        assert!(dot.contains("subgraph cluster_host"), "{dot}");
    }

    /// The real layer map covers every module under src/ exactly once, and
    /// the committed warpine.dot is what it generates: a new, renamed or
    /// removed module, or an edit of the map without `render`, fails here.
    #[test]
    fn layer_map_matches_src_and_the_committed_diagram() {
        let root = root();
        let dot = generate(&root).unwrap_or_else(|e| panic!("{e}"));
        let committed = std::fs::read_to_string(root.join(DOT_FILE)).unwrap_or_default();
        assert!(committed == dot, "{DOT_FILE} is out of date: run `cargo run --bin gen_arch -- render`");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//
// gen_compat — the API compatibility chart (ADR-0033).
//
// Measures how much of the 32-bit API of OS/2 2.0, Warp 3 (3.0) and Warp 4
// (4.0) Warpine implements, and draws it as a grouped bar chart for the head
// of README.md (a light and a dark SVG).
//
// Sources:
//
// - `doc/compat/os2_api_versions.tsv`: every 32-bit export of the Open
//   Watcom import library (`doc/os2_ordinals.md`), with the OS/2 version it
//   first appeared in and the public source that dates it
//   (`doc/compat/SOURCES.md`). Exports dated after 4.0 ("later") or not
//   dateable ("unknown") are counted in no version.
// - `doc/compat/other_api_versions.tsv`: the same for the modules outside
//   that library (VIO / KBD / MOU, REXX, the SOM kernel, MSG, PMCTLS, UCONV,
//   MMPM/2, TCP/IP): their full documented export lists, the ordinal `-`
//   where no public source gives one.
// - Warpine's source: the static registry (`src/loader/api_registry.rs`,
//   `ApiEntry { ordinal, module }`, stubs from `STUB_ORDINALS`, written
//   `N` for DOSCALLS or `MODULE_BASE + N`) and the PM sub-dispatcher arms
//   (`N => {` followed by `// ApiName(` in `src/loader/*.rs`; an arm whose
//   comment says "(stub)" is a stub). An os2386.lib export counts as
//   implemented when the registry has its module and ordinal, or an arm has
//   its ordinal and name. An export of the other table is matched by module
//   and name against `targets/os2api.def`, Warpine's list of every call it
//   dispatches; it counts unless that ordinal is a stub (`STUB_ORDINALS`, or
//   a VIO / KBD / MOU arm that says "(stub)"). Stubs do not count.
//
// Commands: `render` writes `doc/compat/api_coverage.svg` and
// `api_coverage_dark.svg`; `check` fails when they are stale (a test runs
// the same check), so adding an API means re-rendering the chart.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSIONS_TSV: &str = "doc/compat/os2_api_versions.tsv";
const OTHER_TSV: &str = "doc/compat/other_api_versions.tsv";
const API_DEF: &str = "targets/os2api.def";
const LIGHT_SVG: &str = "doc/compat/api_coverage.svg";
const DARK_SVG: &str = "doc/compat/api_coverage_dark.svg";
const REGISTRY_RS: &str = "src/loader/api_registry.rs";
const LOADER_DIR: &str = "src/loader";

/// The VIO / KBD / MOU sub-dispatchers and their modules; their stub arms
/// are one line (`51 => { debug!("  VioSetState (stub)"); NO_ERROR },`).
const SUB_DISPATCHERS: [(&str, &str); 3] = [
    ("viocalls.rs", "VIOCALLS"),
    ("kbdcalls.rs", "KBDCALLS"),
    ("moucalls.rs", "MOUCALLS"),
];

/// The OS/2 versions on the chart, oldest first, with their legend names.
const VERSIONS: [(&str, &str); 3] = [
    ("2.0", "OS/2 2.0"),
    ("3.0", "OS/2 Warp 3"),
    ("4.0", "OS/2 Warp 4"),
];

/// Modules matched as one pool: no public source says which of the two
/// socket DLLs exports a call (the table splits them by the TCP/IP
/// reference's chapters), so a call Warpine maps in either counts.
const POOLS: &[&[&str]] = &[&["SO32DLL", "TCP32DLL"]];

/// The modules an export of `module` may be mapped in.
fn pool(module: &str) -> Vec<&str> {
    POOLS.iter().find(|p| p.contains(&module)).map_or_else(|| vec![module], |p| p.to_vec())
}

/// Which exports a chart row sums.
#[derive(Debug, Clone, Copy)]
enum Rows {
    /// Every export of both tables.
    All,
    /// The os2386.lib table only.
    Lib,
    /// These modules, from either table.
    Modules(&'static [&'static str]),
}

/// Chart rows: a label and what it sums. The first is the headline.
const GROUPS: &[(&str, Rows)] = &[
    ("All charted APIs", Rows::All),
    ("32-bit os2386.lib APIs", Rows::Lib),
    ("Control Program", Rows::Modules(&["DOSCALLS", "QUECALLS", "NLS", "SESMGR", "MSG"])),
    ("Video, keyboard, mouse", Rows::Modules(&["VIOCALLS", "KBDCALLS", "MOUCALLS"])),
    ("Window manager (PMWIN)", Rows::Modules(&["PMWIN"])),
    ("Graphics (PMGPI)", Rows::Modules(&["PMGPI"])),
    ("Standard dialogs (PMCTLS)", Rows::Modules(&["PMCTLS"])),
    ("Shell, profiles, help", Rows::Modules(&["PMSHAPI", "HELPMGR"])),
    ("Spooler (PMSPL)", Rows::Modules(&["PMSPL"])),
    ("Drag and drop (PMDRAG)", Rows::Modules(&["PMDRAG"])),
    ("Workplace Shell (PMWP)", Rows::Modules(&["PMWP"])),
    ("SOM kernel", Rows::Modules(&["SOM"])),
    ("REXX", Rows::Modules(&["REXX", "REXXAPI"])),
    ("Unicode (UCONV)", Rows::Modules(&["UCONV"])),
    ("Multimedia (MMPM/2)", Rows::Modules(&["MDM"])),
    ("TCP/IP sockets", Rows::Modules(&["SO32DLL", "TCP32DLL"])),
];

impl Rows {
    fn includes(self, e: &Export) -> bool {
        match self {
            Rows::All => true,
            Rows::Lib => e.table == Table::Lib,
            Rows::Modules(m) => m.contains(&e.module.as_str()),
        }
    }
}

// ── Data ─────────────────────────────────────────────────────────────────────

/// The table an export comes from, which decides how it is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Table {
    /// `os2_api_versions.tsv`: matched by module and ordinal (registry) or
    /// ordinal and name (PM arms).
    Lib,
    /// `other_api_versions.tsv`: matched by module and name through the def.
    Other,
}

/// One dated export.
#[derive(Debug, Clone, PartialEq)]
struct Export {
    module: String,
    /// `None` where no public source gives the ordinal (other table only).
    ordinal: Option<u32>,
    name: String,
    /// Index into [`VERSIONS`] of the first version that has it.
    since: usize,
    table: Table,
}

/// The dated exports of the table at `path`, and how many lines were left
/// out ("later", "unknown"). Only the other table may write `-` for an
/// ordinal.
fn parse_versions(text: &str, path: &str, table: Table) -> Result<(Vec<Export>, usize), String> {
    let mut out = Vec::new();
    let mut skipped = 0;
    for (n, line) in text.lines().enumerate() {
        if n == 0 || line.trim().is_empty() {
            continue; // header
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 4 {
            return Err(format!("{path}:{}: expected at least 4 columns", n + 1));
        }
        let ordinal = match cols[1] {
            "-" if table == Table::Other => None,
            o => Some(o.parse::<u32>().map_err(|_| format!("{path}:{}: bad ordinal {o:?}", n + 1))?),
        };
        match VERSIONS.iter().position(|(v, _)| *v == cols[3]) {
            Some(since) => out.push(Export { module: cols[0].to_owned(), ordinal, name: cols[2].to_owned(), since, table }),
            None if matches!(cols[3], "later" | "unknown") => skipped += 1,
            None => return Err(format!("{path}:{}: unknown version {:?}", n + 1, cols[3])),
        }
    }
    Ok((out, skipped))
}

/// Both tables: the exports, how many were left out, and how many dated
/// exports came from each (os2386.lib, other).
struct Dated {
    exports: Vec<Export>,
    skipped: usize,
    lib: usize,
    other: usize,
}

fn dated(root: &Path) -> Result<Dated, String> {
    let (mut exports, lib_skipped) = parse_versions(&read(root, VERSIONS_TSV)?, VERSIONS_TSV, Table::Lib)?;
    let lib = exports.len();
    let (other, other_skipped) = parse_versions(&read(root, OTHER_TSV)?, OTHER_TSV, Table::Other)?;
    let n_other = other.len();
    exports.extend(other);
    Ok(Dated { exports, skipped: lib_skipped + other_skipped, lib, other: n_other })
}

/// What Warpine implements, read from its source.
#[derive(Debug, Default)]
struct Implemented {
    /// (module, ordinal) of non-stub registry entries.
    registry: HashSet<(String, u32)>,
    /// (ordinal, upper-case name) of non-stub sub-dispatcher arms.
    arms: HashSet<(u32, String)>,
    /// (module, upper-case name) of every call the def maps, with its ordinal.
    defined: HashMap<(String, String), u32>,
    /// (module, ordinal) of the stubs: `STUB_ORDINALS` and the VIO / KBD /
    /// MOU arms that say "(stub)".
    stubs: HashSet<(String, u32)>,
}

impl Implemented {
    fn has(&self, e: &Export) -> bool {
        match e.table {
            Table::Lib => e.ordinal.is_some_and(|o| {
                self.registry.contains(&(e.module.clone(), o))
                    || self.arms.contains(&(o, e.name.to_ascii_uppercase()))
            }),
            Table::Other => pool(&e.module).iter().any(|&m| {
                self.defined.get(&(m.to_owned(), e.name.to_ascii_uppercase()))
                    .is_some_and(|&o| !self.stubs.contains(&(m.to_owned(), o)))
            }),
        }
    }
}

/// `(module, ordinal)` of the entries of `const STUB_ORDINALS: &[u32] =
/// &[ … ];`: `N` is a DOSCALLS ordinal, `MODULE_BASE + N` the module's own.
fn stub_ordinals(registry: &str) -> BTreeSet<(String, u32)> {
    let Some(start) = registry.find("const STUB_ORDINALS") else { return BTreeSet::new() };
    let rest = &registry[start..];
    let Some(open) = rest.find("&[") else { return BTreeSet::new() };
    let Some(close) = rest[open..].find("];") else { return BTreeSet::new() };
    rest[open + 2..open + close]
        .lines()
        .filter_map(|l| l.split("//").next())
        .flat_map(|l| l.split(','))
        .filter_map(|t| match t.split_once('+') {
            Some((base, n)) => {
                let module = base.trim().strip_suffix("_BASE")?;
                Some((module.to_owned(), n.trim().parse().ok()?))
            }
            None => Some(("DOSCALLS".to_owned(), t.trim().parse().ok()?)),
        })
        .collect()
}

/// Ordinals of the one-line arms `N => { … (stub) … }` of a VIO / KBD / MOU
/// sub-dispatcher.
fn stub_arms(source: &str) -> Vec<u32> {
    source.lines()
        .filter_map(|l| {
            let (ord, body) = l.trim().split_once("=>")?;
            body.contains("(stub)").then_some(())?;
            ord.trim().parse().ok()
        })
        .collect()
}

/// `(module, upper-case name) → ordinal` of every `MODULE.ORDINAL  Name`
/// line of the def.
fn def_entries(def: &str) -> HashMap<(String, String), u32> {
    def.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let mut cols = l.split_whitespace();
            let (module, ordinal) = cols.next()?.split_once('.')?;
            let name = cols.next()?;
            Some(((module.to_owned(), name.to_ascii_uppercase()), ordinal.parse().ok()?))
        })
        .collect()
}

/// `(module, ordinal)` of every `ApiEntry { ordinal: X, module: "M"` in the
/// registry; `X` is a number or `BASE + number` (the module's own ordinal).
fn registry_entries(registry: &str) -> Vec<(String, u32)> {
    const ENTRY: &str = "ApiEntry { ordinal:";
    const MODULE: &str = "module: \"";
    let mut out = Vec::new();
    let mut rest = registry;
    while let Some(i) = rest.find(ENTRY) {
        rest = &rest[i + ENTRY.len()..];
        let Some(comma) = rest.find(',') else { break };
        let expr = &rest[..comma];
        let number = expr.rsplit('+').next().unwrap_or(expr).trim();
        let Some(m) = rest.find(MODULE) else { break };
        let after = &rest[m + MODULE.len()..];
        let Some(q) = after.find('"') else { break };
        if let Ok(ordinal) = number.parse::<u32>() {
            out.push((after[..q].to_owned(), ordinal));
        }
    }
    out
}

/// `(ordinal, upper-case name, stub)` of every `N => {` arm followed by a
/// `// ApiName(` comment.
fn dispatch_arms(source: &str) -> Vec<(u32, String, bool)> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut out = Vec::new();
    for pair in lines.windows(2) {
        let Some(ord) = pair[0].strip_suffix(" => {").and_then(|o| o.parse::<u32>().ok()) else { continue };
        let Some(rest) = pair[1].strip_prefix("// ") else { continue };
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if name.starts_with(|c: char| c.is_ascii_uppercase()) && rest[name.len()..].starts_with('(') {
            out.push((ord, name.to_ascii_uppercase(), rest.contains("(stub)")));
        }
    }
    out
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))
}

fn implemented(root: &Path) -> Result<Implemented, String> {
    let registry = read(root, REGISTRY_RS)?;
    let mut imp = Implemented {
        defined: def_entries(&read(root, API_DEF)?),
        stubs: stub_ordinals(&registry).into_iter().collect(),
        ..Implemented::default()
    };
    for (file, module) in SUB_DISPATCHERS {
        let src = read(root, &format!("{LOADER_DIR}/{file}"))?;
        imp.stubs.extend(stub_arms(&src).into_iter().map(|o| (module.to_owned(), o)));
    }
    for (module, ordinal) in registry_entries(&registry) {
        if !imp.stubs.contains(&(module.clone(), ordinal)) {
            imp.registry.insert((module, ordinal));
        }
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(LOADER_DIR))
        .map_err(|e| format!("{LOADER_DIR}: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();
    for f in files {
        let src = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        for (ordinal, name, stub) in dispatch_arms(&src) {
            if !stub {
                imp.arms.insert((ordinal, name));
            }
        }
    }
    Ok(imp)
}

/// Covered / total per group and version.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Share {
    covered: usize,
    total: usize,
}

impl Share {
    fn percent(self) -> u32 {
        (self.covered * 1000).checked_div(self.total).map_or(0, |permille| ((permille + 5) / 10) as u32)
    }
    fn fraction(self) -> f64 {
        if self.total == 0 { 0.0 } else { self.covered as f64 / self.total as f64 }
    }
}

fn shares(exports: &[Export], imp: &Implemented) -> Vec<[Share; 3]> {
    GROUPS.iter().map(|&(_, rows)| {
        let mut row = [Share::default(); 3];
        for e in exports.iter().filter(|e| rows.includes(e)) {
            let done = imp.has(e);
            for (v, s) in row.iter_mut().enumerate() {
                if e.since <= v {
                    s.total += 1;
                    s.covered += usize::from(done);
                }
            }
        }
        row
    }).collect()
}

// ── Drawing ──────────────────────────────────────────────────────────────────

/// Colours of one theme: the dataviz reference palette's surfaces, text and
/// categorical slots 1–3 (validated all-pairs in both modes; light slot 3 is
/// below 3:1 on the surface, so every bar carries its value as text).
struct Theme {
    surface: &'static str,
    text: &'static str,
    text_secondary: &'static str,
    muted: &'static str,
    grid: &'static str,
    series: [&'static str; 3],
}

const LIGHT: Theme = Theme {
    surface: "#fcfcfb", text: "#0b0b0b", text_secondary: "#52514e", muted: "#898781", grid: "#e4e3df",
    series: ["#2a78d6", "#eb6834", "#1baf7a"],
};
const DARK: Theme = Theme {
    surface: "#1a1a19", text: "#f0efec", text_secondary: "#c3c2b7", muted: "#898781", grid: "#383835",
    series: ["#3987e5", "#d95926", "#199e70"],
};

const WIDTH: i32 = 760;
const PAD: i32 = 24;
const LABEL_W: i32 = 190;
const PLOT_X: i32 = PAD + LABEL_W;
const PLOT_W: i32 = 400;
const BAR_H: i32 = 12;
const BAR_GAP: i32 = 2;
const GROUP_GAP: i32 = 14;
const RADIUS: i32 = 4;
const TOP: i32 = 104;
const FONT: &str = "system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A horizontal bar from `x` with width `w`, rounded at the data end only.
fn bar_path(x: i32, y: i32, w: i32, h: i32) -> String {
    let r = RADIUS.min(w).min(h / 2);
    if w <= 0 {
        return String::new();
    }
    format!(
        "M{x} {y}h{a}a{r} {r} 0 0 1 {r} {r}v{b}a{r} {r} 0 0 1 -{r} {r}h-{a}z",
        a = w - r, b = h - 2 * r,
    )
}

fn render(rows: &[[Share; 3]], d: &Dated, theme: &Theme, title_suffix: &str) -> String {
    let group_h = 3 * BAR_H + 2 * BAR_GAP;
    let plot_h = rows.len() as i32 * (group_h + GROUP_GAP) - GROUP_GAP;
    let height = TOP + plot_h + 72;
    let mut s = String::new();
    let _ = writeln!(s, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" viewBox=\"0 0 {WIDTH} {height}\" role=\"img\" aria-labelledby=\"t d\" font-family=\"{FONT}\">");
    let overall = rows[0];
    let _ = writeln!(s, "<title id=\"t\">Warpine API coverage{title_suffix}</title>");
    let _ = writeln!(s, "<desc id=\"d\">Share of the charted OS/2 API that Warpine implements: {}.</desc>",
        VERSIONS.iter().zip(overall).map(|((_, n), sh)| format!("{n} {}% ({} of {})", sh.percent(), sh.covered, sh.total))
            .collect::<Vec<_>>().join(", "));
    let _ = writeln!(s, "<rect width=\"{WIDTH}\" height=\"{height}\" rx=\"8\" fill=\"{}\"/>", theme.surface);
    let _ = writeln!(s, "<text x=\"{PAD}\" y=\"34\" font-size=\"17\" font-weight=\"600\" fill=\"{}\">Warpine API coverage of OS/2 2.0, Warp 3 and Warp 4</text>", theme.text);
    let _ = writeln!(s, "<text x=\"{PAD}\" y=\"55\" font-size=\"12.5\" fill=\"{}\">Share of each version's API that Warpine implements (stubs not counted)</text>", theme.text_secondary);
    // Legend.
    let mut lx = PAD;
    for ((_, name), color) in VERSIONS.iter().zip(theme.series) {
        let _ = writeln!(s, "<rect x=\"{lx}\" y=\"72\" width=\"12\" height=\"12\" rx=\"3\" fill=\"{color}\"/>");
        let _ = writeln!(s, "<text x=\"{}\" y=\"82.5\" font-size=\"12.5\" fill=\"{}\">{}</text>", lx + 18, theme.text, esc(name));
        lx += 18 + 7 * name.len() as i32 + 26;
    }
    // Gridlines and the percentage axis.
    let bottom = TOP + plot_h;
    for pct in [0, 25, 50, 75, 100] {
        let x = PLOT_X + PLOT_W * pct / 100;
        let _ = writeln!(s, "<line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{bottom}\" stroke=\"{}\" stroke-width=\"1\"/>", TOP - 6, theme.grid);
        let _ = writeln!(s, "<text x=\"{x}\" y=\"{}\" font-size=\"11\" text-anchor=\"middle\" fill=\"{}\">{pct}%</text>", bottom + 16, theme.muted);
    }
    // Groups.
    for (g, ((label, _), row)) in GROUPS.iter().zip(rows).enumerate() {
        let gy = TOP + g as i32 * (group_h + GROUP_GAP);
        let weight = if g == 0 { "600" } else { "400" };
        let _ = writeln!(s, "<text x=\"{PAD}\" y=\"{}\" font-size=\"12.5\" font-weight=\"{weight}\" fill=\"{}\">{}</text>", gy + group_h / 2 + 4, theme.text, esc(label));
        for (v, sh) in row.iter().enumerate() {
            let y = gy + v as i32 * (BAR_H + BAR_GAP);
            let w = (sh.fraction() * PLOT_W as f64).round() as i32;
            let d = bar_path(PLOT_X, y, w, BAR_H);
            if !d.is_empty() {
                let _ = writeln!(s, "<path d=\"{d}\" fill=\"{}\"><title>{}: {}% ({} of {})</title></path>", theme.series[v], VERSIONS[v].1, sh.percent(), sh.covered, sh.total);
            }
            // A module the version did not have yet gets a dash, not 0%.
            let value = if sh.total == 0 { "—".to_owned() } else { format!("{}% · {}/{}", sh.percent(), sh.covered, sh.total) };
            let _ = writeln!(s, "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{}\">{value}</text>", PLOT_X + w + 6, y + BAR_H - 2, theme.text_secondary);
        }
    }
    let _ = writeln!(s, "<text x=\"{PAD}\" y=\"{}\" font-size=\"10.5\" fill=\"{}\">{} dated exports: {} of the Open Watcom os2386.lib, {} of the other modules; {} dated after 4.0 or undated are left out.</text>", height - 28, theme.muted, d.lib + d.other, d.lib, d.other, d.skipped);
    let _ = writeln!(s, "<text x=\"{PAD}\" y=\"{}\" font-size=\"10.5\" fill=\"{}\">Dating sources: doc/compat/SOURCES.md. Generated from Warpine's source by gen_compat.</text>", height - 13, theme.muted);
    s.push_str("</svg>\n");
    s
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The light and dark charts for the tree at `root`.
fn charts(root: &Path) -> Result<(String, String), String> {
    let d = dated(root)?;
    let imp = implemented(root)?;
    let rows = shares(&d.exports, &imp);
    Ok((render(&rows, &d, &LIGHT, ""), render(&rows, &d, &DARK, " (dark)")))
}

fn main() -> ExitCode {
    let root = root();
    let cmd = std::env::args().nth(1).unwrap_or_default();
    let result = match cmd.as_str() {
        "render" => charts(&root).and_then(|(light, dark)| {
            for (path, svg) in [(LIGHT_SVG, light), (DARK_SVG, dark)] {
                std::fs::write(root.join(path), svg).map_err(|e| format!("{path}: {e}"))?;
            }
            println!("wrote {LIGHT_SVG}, {DARK_SVG}");
            Ok(())
        }),
        "check" => charts(&root).and_then(|(light, dark)| {
            let stale: Vec<&str> = [(LIGHT_SVG, light), (DARK_SVG, dark)].into_iter()
                .filter(|(path, svg)| std::fs::read_to_string(root.join(path)).ok().as_deref() != Some(svg.as_str()))
                .map(|(path, _)| path)
                .collect();
            if stale.is_empty() {
                println!("✓  {LIGHT_SVG} and {DARK_SVG} are up to date");
                Ok(())
            } else {
                Err(format!("{} out of date: run `cargo run --bin gen_compat -- render`", stale.join(", ")))
            }
        }),
        "table" => dated(&root).and_then(|d| {
            let imp = implemented(&root)?;
            for ((label, _), row) in GROUPS.iter().zip(shares(&d.exports, &imp)) {
                let cells: Vec<String> = row.iter().map(|s| format!("{:>3}% {:>4}/{:<4}", s.percent(), s.covered, s.total)).collect();
                println!("{label:<26} {}", cells.join("   "));
            }
            Ok(())
        }),
        _ => Err("usage: gen_compat render | check | table".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("error: {e}"); ExitCode::FAILURE }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_skip_later_and_unknown() {
        let tsv = "module\tordinal\tname\tsince\tsource_id\tnote\n\
                   DOSCALLS\t229\tDosSleep\t2.0\tS1\tx\n\
                   PMWIN\t999\tWinNew\t3.0\tS5\tx\n\
                   DOSCALLS\t981\tDosOpenL\tlater\tS9\tx\n\
                   PMGPI\t700\tGpiOval\tunknown\tH1\tx\n";
        let (e, skipped) = parse_versions(tsv, "t", Table::Lib).unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1], Export { module: "PMWIN".into(), ordinal: Some(999), name: "WinNew".into(), since: 1, table: Table::Lib });
        assert!(parse_versions("h\nDOSCALLS\tx\tDosSleep\t2.0\n", "t", Table::Lib).is_err());
        assert!(parse_versions("h\nDOSCALLS\t1\tDosSleep\t5.0\n", "t", Table::Lib).is_err());
    }

    /// Only the other table may leave the ordinal out.
    #[test]
    fn only_the_other_table_has_unknown_ordinals() {
        let tsv = "h\nSOM\t-\tsomBuildClass\t3.0\tS60\tx\n";
        assert!(parse_versions(tsv, "t", Table::Lib).is_err());
        let (e, _) = parse_versions(tsv, "t", Table::Other).unwrap();
        assert_eq!(e[0].ordinal, None);
        assert_eq!(e[0].table, Table::Other);
    }

    #[test]
    fn registry_entries_and_stubs_are_read() {
        let src = "ApiEntry { ordinal: 229, module: \"DOSCALLS\", name: \"DosSleep\", argc: 1,\n\
                   ApiEntry { ordinal: 1024 + 10, module: \"QUECALLS\", name: \"DosPurgeQueue\",\n\
                   ApiEntry { ordinal: PMSHAPI_BASE + 119, module: \"PMSHAPI\", name: \"WinStartApp\",\n\
                   const STUB_ORDINALS: &[u32] = &[\n    317,  // DosDebug\n    415,  // DosShutdown\n\
                   SOM_BASE + 28,  // somConstructClass\n    REXXAPI_BASE + 22, REXXAPI_BASE + 23,\n];";
        assert_eq!(registry_entries(src), vec![("DOSCALLS".into(), 229), ("QUECALLS".into(), 10), ("PMSHAPI".into(), 119)]);
        let stub = |m: &str, o| (m.to_owned(), o);
        assert_eq!(stub_ordinals(src), BTreeSet::from([
            stub("DOSCALLS", 317), stub("DOSCALLS", 415), stub("SOM", 28), stub("REXXAPI", 22), stub("REXXAPI", 23),
        ]));
    }

    #[test]
    fn one_line_stub_arms_and_def_entries_are_read() {
        let src = "            51 => { debug!(\"  VioSetState (stub)\"); NO_ERROR },\n\
                   19 => self.vio_wrt_tty(a, b),\n\
                   Some(x) => { debug!(\"(stub)\") }\n";
        assert_eq!(stub_arms(src), vec![51]);
        let def = "# header\nVIOCALLS.19   VioWrtTTY\nSOM.3072  somResolveByName  # NOTE: own\nbad line\n";
        let d = def_entries(def);
        assert_eq!(d.len(), 2);
        assert_eq!(d[&("VIOCALLS".to_owned(), "VIOWRTTTY".to_owned())], 19);
        assert_eq!(d[&("SOM".to_owned(), "SOMRESOLVEBYNAME".to_owned())], 3072);
    }

    /// The other table matches by module and name through the def, and a
    /// stub there does not count.
    #[test]
    fn other_exports_match_through_the_def_without_stubs() {
        let ex = |m: &str, n: &str| Export { module: m.into(), ordinal: None, name: n.into(), since: 0, table: Table::Other };
        let mut imp = Implemented::default();
        imp.defined.insert(("VIOCALLS".into(), "VIOWRTTTY".into()), 19);
        imp.defined.insert(("VIOCALLS".into(), "VIOSETSTATE".into()), 51);
        imp.stubs.insert(("VIOCALLS".into(), 51));
        assert!(imp.has(&ex("VIOCALLS", "VioWrtTTY")));
        assert!(!imp.has(&ex("VIOCALLS", "VioSetState")), "a stub");
        assert!(!imp.has(&ex("KBDCALLS", "VioWrtTTY")), "another module");
        assert!(!imp.has(&ex("VIOCALLS", "VioPopUp")), "not in the def");
        // The socket DLLs are one pool.
        imp.defined.insert(("SO32DLL".into(), "GETHOSTNAME".into()), 5);
        assert!(imp.has(&ex("TCP32DLL", "gethostname")));
        assert!(!imp.has(&ex("UCONV", "gethostname")));
        assert_eq!(pool("TCP32DLL"), ["SO32DLL", "TCP32DLL"]);
        assert_eq!(pool("SOM"), ["SOM"]);
    }

    #[test]
    fn dispatch_arms_with_names_and_stubs() {
        let src = "            763 => {\n                // WinInitialize(ULONG flOptions) → HAB\n\
                   400 => {\n    // GpiLoadFonts(HAB hab, PSZ psz) → BOOL  (stub)\n\
                   0 => {\n    if x {\n\
                   5 => {\n    // not an API\n";
        assert_eq!(dispatch_arms(src), vec![(763, "WININITIALIZE".into(), false), (400, "GPILOADFONTS".into(), true)]);
    }

    #[test]
    fn shares_are_cumulative_by_version() {
        let ex = |m: &str, o: u32, n: &str, since| Export { module: m.into(), ordinal: Some(o), name: n.into(), since, table: Table::Lib };
        let mut exports = vec![
            ex("DOSCALLS", 229, "DosSleep", 0),
            ex("DOSCALLS", 900, "DosNew3", 1),
            ex("PMWIN", 763, "WinInitialize", 0),
            ex("PMWIN", 999, "WinNew4", 2),
        ];
        exports.push(Export { module: "UCONV".into(), ordinal: None, name: "UniNew".into(), since: 2, table: Table::Other });
        let mut imp = Implemented::default();
        imp.registry.insert(("DOSCALLS".into(), 229));
        imp.arms.insert((763, "WININITIALIZE".into()));
        imp.defined.insert(("UCONV".into(), "UNINEW".into()), 1);
        let rows = shares(&exports, &imp);
        let row = |label: &str| rows[GROUPS.iter().position(|(l, _)| *l == label).expect(label)].map(|s| (s.covered, s.total));
        // All APIs: 2/2 in 2.0, 2/3 in 3.0, 3/5 in 4.0.
        assert_eq!(row("All charted APIs"), [(2, 2), (2, 3), (3, 5)]);
        // os2386.lib only: 2/2, 2/3, 2/4.
        assert_eq!(row("32-bit os2386.lib APIs"), [(2, 2), (2, 3), (2, 4)]);
        // Control Program: 1/1, 1/2, 1/2.
        assert_eq!(row("Control Program"), [(1, 1), (1, 2), (1, 2)]);
        // UCONV: none before 4.0.
        assert_eq!(row("Unicode (UCONV)"), [(0, 0), (0, 0), (1, 1)]);
        assert_eq!(Share { covered: 2, total: 3 }.percent(), 67);
        assert_eq!(Share::default().percent(), 0);
    }

    /// Every module of either table is in some row, so no dated export is
    /// left off the chart.
    #[test]
    fn every_dated_module_has_a_row() {
        let d = dated(&root()).expect("tables");
        let charted: HashSet<&str> = GROUPS.iter()
            .filter_map(|(_, r)| if let Rows::Modules(m) = r { Some(m.iter().copied()) } else { None })
            .flatten().collect();
        let missing: BTreeSet<&str> = d.exports.iter().map(|e| e.module.as_str()).filter(|m| !charted.contains(m)).collect();
        assert!(missing.is_empty(), "modules without a chart row: {missing:?}");
    }

    /// A row whose module a version did not have shows a dash, not 0%.
    #[test]
    fn an_empty_version_shows_a_dash() {
        let d = Dated { exports: Vec::new(), skipped: 0, lib: 0, other: 0 };
        let rows = vec![[Share::default(), Share::default(), Share { covered: 1, total: 2 }]; GROUPS.len()];
        let svg = render(&rows, &d, &LIGHT, "");
        assert!(svg.contains(">—</text>"));
        assert!(svg.contains(">50% · 1/2</text>"));
    }

    #[test]
    fn bars_are_rounded_at_the_data_end_only() {
        assert_eq!(bar_path(10, 20, 0, 12), "");
        let d = bar_path(10, 20, 100, 12);
        assert!(d.starts_with("M10 20h96a4 4"), "{d}");
        // A bar narrower than the radius stays inside its width.
        assert!(bar_path(10, 20, 2, 12).contains("h0a2 2"));
    }

    /// The committed charts match the data and the source (re-render with
    /// `cargo run --bin gen_compat -- render` after adding an API).
    #[test]
    fn committed_charts_are_current() {
        let root = root();
        let (light, dark) = charts(&root).expect("charts");
        assert_eq!(std::fs::read_to_string(root.join(LIGHT_SVG)).ok().as_deref(), Some(light.as_str()),
                   "{LIGHT_SVG} is stale: run `cargo run --bin gen_compat -- render`");
        assert_eq!(std::fs::read_to_string(root.join(DARK_SVG)).ok().as_deref(), Some(dark.as_str()),
                   "{DARK_SVG} is stale: run `cargo run --bin gen_compat -- render`");
    }

    /// Every dispatcher arm whose comment names a PM call writes it as
    /// `// Name(`: `gen_compat`, `build.rs` (`DosQueryProcAddr` by name)
    /// and the def's PMWIN / PMGPI lines all find arms that way, and 8 arms
    /// written `// WinInitialize` were invisible to all three (ADR-0097).
    #[test]
    fn every_pm_arm_comment_names_its_call_with_parentheses() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut bare = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(LOADER_DIR)).expect("src/loader")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();
        for f in files {
            let src = std::fs::read_to_string(&f).expect("read a loader file");
            let lines: Vec<&str> = src.lines().map(str::trim).collect();
            for pair in lines.windows(2) {
                if !pair[0].strip_suffix(" => {").is_some_and(|o| o.parse::<u32>().is_ok()) { continue; }
                let Some(rest) = pair[1].strip_prefix("// ") else { continue };
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let pm = ["Win", "Gpi", "Dev"].iter().any(|p| name.len() > p.len() && name.starts_with(p));
                if pm && !rest[name.len()..].starts_with('(') {
                    bare.push(format!("{}: {}", f.display(), pair[1]));
                }
            }
        }
        assert!(bare.is_empty(), "write these arm comments as `// Name(params)`:\n{}", bare.join("\n"));
    }
}

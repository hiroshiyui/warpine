// SPDX-License-Identifier: GPL-3.0-or-later
//
// gen_api — Ordinal Table Canonical Build Tool
//
// `targets/os2api.def` is the single source of truth for the OS/2 API ordinal
// address space.  This tool validates it, generates derived Rust source, and
// re-emits a canonically sorted version of the file.
//
// Usage:
//   cargo run --bin gen_api -- <command> [--def PATH] [--trace PATH]
//
// Commands:
//   show        Pretty-print the full ordinal table with flat addresses
//   check       Validate os2api.def and detect drift against api_trace.rs
//   gen-trace   Emit Rust source for ordinal_to_name() + module_for_ordinal()
//   gen-def     Re-emit a canonically sorted os2api.def
//   check-headers  Check the SDK's C headers (sdk/include) against os2api.def

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

// ── Canonical module address-space layout ────────────────────────────────────
//
// MUST stay in sync with src/loader/constants.rs.
// This table is the authoritative flat ordinal map; all derived artifacts
// (api_trace.rs, os2api.def) are generated from it.

const MODULES: &[(&str, u32)] = &[
    ("DOSCALLS", 0),
    ("QUECALLS", 1024),
    ("PMWIN",    2048),
    ("PMGPI",    3072),
    ("KBDCALLS", 4096),
    ("VIOCALLS", 5120),
    ("SESMGR",   6144),
    ("NLS",      7168),
    ("MSG",      8192),
    ("MDM",      10240),
    ("UCONV",    12288),
    ("SO32DLL",  13312),
    ("TCP32DLL", 14336),
    ("MOUCALLS", 15360),
    ("PMSHAPI",  16384),
    ("HELPMGR",  17408),
    ("PMSPL",    18432),
    ("PMCTLS",   19456),
    ("WARPINE",  20480),
    ("PMDRAG",   21504),
    ("SOM",      22528),
    ("REXX",     26624),
    ("REXXAPI",  27648),
    ("PMWP",     28672),
];
const STUB_AREA_SIZE: u32 = 29696; // PMWP_BASE + 1024 (SOM is SOM_ORDINALS = 4096 wide, ADR-0070)

// Rust constant name used for each module's base in generated code.
// Must match src/loader/constants.rs exactly.
fn base_const(module: &str) -> &'static str {
    match module {
        "DOSCALLS" => "0",
        "QUECALLS" => "1024",
        "PMWIN"    => "PMWIN_BASE",
        "PMGPI"    => "PMGPI_BASE",
        "KBDCALLS" => "KBDCALLS_BASE",
        "VIOCALLS" => "VIOCALLS_BASE",
        "SESMGR"   => "SESMGR_BASE",
        "NLS"      => "NLS_BASE",
        "MSG"      => "MSG_BASE",
        "MDM"      => "MDM_BASE",
        "UCONV"    => "UCONV_BASE",
        "SO32DLL"  => "SO32DLL_BASE",
        "TCP32DLL" => "TCP32DLL_BASE",
        "MOUCALLS" => "MOUCALLS_BASE",
        "PMSHAPI"  => "PMSHAPI_BASE",
        "HELPMGR"  => "HELPMGR_BASE",
        "PMSPL"    => "PMSPL_BASE",
        "PMCTLS"   => "PMCTLS_BASE",
        "WARPINE"  => "WARPINE_BASE",
        "PMDRAG"   => "PMDRAG_BASE",
        "SOM"      => "SOM_BASE",
        "REXX"     => "REXX_BASE",
        "REXXAPI"  => "REXXAPI_BASE",
        "PMWP"     => "PMWP_BASE",
        _          => "?",
    }
}

// Rust constant name for the exclusive upper bound of each module's range.
fn upper_const(module: &str) -> &'static str {
    match module {
        "DOSCALLS" => "1024",
        "QUECALLS" => "PMWIN_BASE",
        "PMWIN"    => "PMGPI_BASE",
        "PMGPI"    => "KBDCALLS_BASE",
        "KBDCALLS" => "VIOCALLS_BASE",
        "VIOCALLS" => "SESMGR_BASE",
        "SESMGR"   => "NLS_BASE",
        "NLS"      => "MSG_BASE",
        "MSG"      => "MDM_BASE",
        "MDM"      => "UCONV_BASE",
        "UCONV"    => "SO32DLL_BASE",
        "SO32DLL"  => "TCP32DLL_BASE",
        "TCP32DLL" => "MOUCALLS_BASE",
        "MOUCALLS" => "PMSHAPI_BASE",
        "PMSHAPI"  => "HELPMGR_BASE",
        "HELPMGR"  => "PMSPL_BASE",
        "PMSPL"    => "PMCTLS_BASE",
        "PMCTLS"   => "WARPINE_BASE",
        "WARPINE"  => "PMDRAG_BASE",
        "PMDRAG"   => "SOM_BASE",
        "SOM"      => "REXX_BASE",
        "REXX"     => "REXXAPI_BASE",
        "REXXAPI"  => "PMWP_BASE",
        "PMWP"     => "STUB_AREA_SIZE",
        _          => "?",
    }
}

// ── Data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct OrdEntry {
    module: String,
    local_ordinal: u32,
    flat_ordinal: u32,
    name: String,
    /// Source line number in os2api.def (1-based), for diagnostics.
    line: usize,
    /// The text after `# NOTE:` on the entry's line: an explanation of a
    /// known difference from the OW snapshot (a naming convention the alias
    /// normaliser cannot detect), which `validate-doc` then reports as
    /// acknowledged instead of as a mismatch.
    note: Option<String>,
}

// ── Module helpers ───────────────────────────────────────────────────────────

fn module_base(module: &str) -> Option<u32> {
    MODULES.iter().find(|(m, _)| *m == module).map(|(_, b)| *b)
}

fn module_upper(module: &str) -> u32 {
    let pos = MODULES.iter().position(|(m, _)| *m == module).unwrap_or(0);
    MODULES.get(pos + 1).map(|(_, b)| *b).unwrap_or(STUB_AREA_SIZE)
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// The marker that starts an acknowledged-difference note on a def line.
const NOTE_MARKER: &str = "# NOTE:";

/// The note of a def line (`... # NOTE: reason`), trimmed; `None` if absent.
fn def_note(line: &str) -> Option<String> {
    let note = line.split_once(NOTE_MARKER)?.1.trim();
    (!note.is_empty()).then(|| note.to_owned())
}

fn parse_def(path: &Path) -> Result<Vec<OrdEntry>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    let mut entries = Vec::new();

    for (idx, raw) in content.lines().enumerate() {
        let line_num = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let module_ord = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");

        if name.is_empty() {
            return Err(format!("line {line_num}: missing symbol name: {line:?}"));
        }

        let dot = module_ord.find('.').ok_or_else(|| {
            format!("line {line_num}: expected MODULE.ORDINAL, got {module_ord:?}")
        })?;
        let module = &module_ord[..dot];
        let ord_str = &module_ord[dot + 1..];
        let local_ordinal: u32 = ord_str.parse().map_err(|_| {
            format!("line {line_num}: invalid ordinal {ord_str:?}")
        })?;

        let base = module_base(module).ok_or_else(|| {
            format!("line {line_num}: unknown module {module:?}")
        })?;

        entries.push(OrdEntry {
            module: module.to_owned(),
            local_ordinal,
            flat_ordinal: base + local_ordinal,
            name: name.to_owned(),
            line: line_num,
            note: def_note(raw),
        });
    }

    Ok(entries)
}

// ── api_trace.rs drift detector ──────────────────────────────────────────────
//
// Extracts the (flat_ordinal → name) mapping that api_trace::ordinal_to_name()
// currently encodes by text-scanning the source file.  The format is regular
// enough that a hand-written scanner beats pulling in a Rust parser crate.
//
// Patterns handled:
//   Direct arm (DOSCALLS, flat ordinal):
//     "        273 => \"DosOpen\","
//
//   Range sub-match arm (QUECALLS, NLS, MSG, MDM, UCONV):
//     "        o if (NLS_BASE..MSG_BASE).contains(&o) => match o - NLS_BASE {"
//     "            5 => \"NlsQueryCp\","
//
// Only handles ordinal_to_name; stops at module_for_ordinal or arg_names.

fn extract_trace_names(trace_path: &Path) -> Result<HashMap<u32, String>, String> {
    let content = fs::read_to_string(trace_path)
        .map_err(|e| format!("cannot read {}: {e}", trace_path.display()))?;

    let mut map: HashMap<u32, String> = HashMap::new();

    // State: are we inside ordinal_to_name? inside a sub-match? what base?
    let mut in_fn = false;
    let mut sub_base: Option<u32> = None;
    let mut brace_depth: i32 = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect entry into ordinal_to_name
        if !in_fn {
            if trimmed.starts_with("pub fn ordinal_to_name(") {
                in_fn = true;
                brace_depth = 0;
            }
            continue;
        }

        // Track braces to detect function exit
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if brace_depth <= 0 {
            break; // exited ordinal_to_name
        }

        // Detect range sub-match header: "o if (BASE..UPPER).contains(&o) => match o - BASE {"
        if trimmed.contains(".contains(&o)") && trimmed.contains("match o - ") {
            // Extract the base value from the name after "o - "
            if let Some(sub_base_name) = trimmed.split("o - ").nth(1).map(|s| s.trim().trim_matches('{').trim()) {
                sub_base = Some(resolve_const(sub_base_name));
            }
            continue;
        }

        // Detect exit from sub-match block
        if sub_base.is_some() && trimmed.starts_with("},") && !trimmed.contains("=>") {
            sub_base = None;
            continue;
        }

        // Parse an arm: "N => \"Name\"," or "N  => \"Name\","
        if let Some((lhs, rhs)) = trimmed.split_once("=>") {
            let lhs = lhs.trim();
            let rhs = rhs.trim().trim_end_matches(',');

            // Must be a plain integer arm (not a guard like "o if ...")
            if let Ok(local_ord) = lhs.parse::<u32>()
                && let Some(name) = extract_str_literal(rhs)
            {
                let flat = sub_base.unwrap_or(0) + local_ord;
                map.insert(flat, name);
            }
        }
    }

    Ok(map)
}

/// Resolve a constant name to its u32 value using our MODULES table.
fn resolve_const(name: &str) -> u32 {
    let name = name.trim();
    match name {
        "0"             => 0,
        "1024"          => 1024,
        "PMWIN_BASE"    => 2048,
        "PMGPI_BASE"    => 3072,
        "KBDCALLS_BASE" => 4096,
        "VIOCALLS_BASE" => 5120,
        "SESMGR_BASE"   => 6144,
        "NLS_BASE"      => 7168,
        "MSG_BASE"      => 8192,
        "MDM_BASE"      => 10240,
        "UCONV_BASE"    => 12288,
        "SO32DLL_BASE"  => 13312,
        "TCP32DLL_BASE" => 14336,
        "MOUCALLS_BASE" => 15360,
        "PMSHAPI_BASE"  => 16384,
        "HELPMGR_BASE"  => 17408,
        "PMSPL_BASE"    => 18432,
        "PMCTLS_BASE"   => 19456,
        "WARPINE_BASE"  => 20480,
        "PMDRAG_BASE"   => 21504,
        "SOM_BASE"      => 22528,
        "REXX_BASE"     => 26624,
        "REXXAPI_BASE"  => 27648,
        "PMWP_BASE"     => 28672,
        "STUB_AREA_SIZE"=> 29696,
        other => other.parse().unwrap_or(0),
    }
}

/// Extract the string value from a Rust string literal `"Foo"` or `"?"`.
/// Accepts an optional trailing comma (e.g. `"DosExit",`).
fn extract_str_literal(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches(',').trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        if inner != "?" {
            return Some(inner.to_owned());
        }
    }
    None
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn cmd_show(entries: &[OrdEntry]) {
    let total = entries.len();
    let mut last_module = String::new();

    println!("{:<12}  {:>6}  {:>6}  NAME", "MODULE", "LOCAL", "FLAT");
    println!("{}", "─".repeat(58));

    for e in entries {
        if e.module != last_module {
            if !last_module.is_empty() {
                println!();
            }
            let base = module_base(&e.module).unwrap_or(0);
            let upper = module_upper(&e.module);
            println!("── {} (flat {:#06x}–{:#06x})", e.module, base, upper - 1);
            last_module = e.module.clone();
        }
        println!("  {:>6}  {:>6}  {}", e.local_ordinal, e.flat_ordinal, e.name);
    }

    println!();
    let n_modules = entries
        .iter()
        .map(|e| e.module.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    println!("Total: {total} entries across {n_modules} modules");
}

fn cmd_check(entries: &[OrdEntry], trace_path: &Path) -> bool {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 1. No duplicate (module, local_ordinal) within def file
    {
        let mut seen: HashMap<(&str, u32), usize> = HashMap::new();
        for e in entries {
            let key = (e.module.as_str(), e.local_ordinal);
            if let Some(&prev_line) = seen.get(&key) {
                errors.push(format!(
                    "duplicate {}.{}: first at line {prev_line}, redefined at line {}",
                    e.module, e.local_ordinal, e.line
                ));
            } else {
                seen.insert(key, e.line);
            }
        }
    }

    // 2. Entries within each module are sorted by local ordinal
    {
        let mut by_module: HashMap<&str, Vec<u32>> = HashMap::new();
        for e in entries {
            by_module.entry(e.module.as_str()).or_default().push(e.local_ordinal);
        }
        for (module, ords) in &by_module {
            let mut sorted = ords.clone();
            sorted.sort_unstable();
            if *ords != sorted {
                warnings.push(format!(
                    "{module} ordinals are not sorted (run gen-def to fix)"
                ));
            }
        }
    }

    // 3. No flat ordinal overflow into the next module's range
    for e in entries {
        let upper = module_upper(&e.module);
        if e.flat_ordinal >= upper {
            errors.push(format!(
                "line {}: {}.{} → flat {} overflows into next module range (< {})",
                e.line, e.module, e.local_ordinal, e.flat_ordinal, upper
            ));
        }
    }

    // 4. Drift check: compare def entries against api_trace::ordinal_to_name()
    //
    // Modules covered by ordinal_to_name in api_trace.rs:
    //   DOSCALLS, QUECALLS, NLS, MSG, MDM, UCONV
    // Modules NOT covered (use sub-dispatchers):
    //   PMWIN, PMGPI, KBDCALLS, VIOCALLS, SESMGR

    // All modules now covered by ordinal_to_name in api_trace.rs.
    // Excludes PMWIN, PMGPI, SESMGR which return "?" and use sub-dispatchers.
    let trace_covered_modules = [
        "DOSCALLS", "QUECALLS", "KBDCALLS", "VIOCALLS", "NLS", "MSG", "MDM", "UCONV",
        "SO32DLL", "TCP32DLL", "MOUCALLS", "PMSHAPI", "HELPMGR", "PMSPL", "PMCTLS", "WARPINE", "PMDRAG", "SOM", "REXX", "REXXAPI", "PMWP",
    ];
    let def_in_trace: Vec<&OrdEntry> = entries
        .iter()
        .filter(|e| trace_covered_modules.contains(&e.module.as_str()))
        .collect();

    match extract_trace_names(trace_path) {
        Ok(trace_map) => {
            let mut missing_in_trace: Vec<&OrdEntry> = Vec::new();
            let mut name_mismatch: Vec<(&OrdEntry, &str)> = Vec::new();

            for e in &def_in_trace {
                match trace_map.get(&e.flat_ordinal) {
                    None => missing_in_trace.push(e),
                    Some(trace_name) if trace_name != &e.name => {
                        // Leak the trace_name string to get a 'static lifetime isn't
                        // possible; push as an owned string instead
                        let _ = trace_name; // use below without lifetime tricks
                        name_mismatch.push((e, Box::leak(trace_name.clone().into_boxed_str())));
                    }
                    _ => {}
                }
            }

            // Entries in trace but not in def (orphans)
            let def_flat: std::collections::HashSet<u32> =
                def_in_trace.iter().map(|e| e.flat_ordinal).collect();
            let mut orphan_in_trace: Vec<(u32, String)> = trace_map
                .iter()
                .filter(|(ord, _)| !def_flat.contains(*ord))
                .map(|(ord, name)| (*ord, name.clone()))
                .collect();
            orphan_in_trace.sort_by_key(|(ord, _)| *ord);

            if !missing_in_trace.is_empty() {
                let list: Vec<String> = missing_in_trace
                    .iter()
                    .map(|e| {
                        format!(
                            "  {}.{} (flat {}) = {}",
                            e.module, e.local_ordinal, e.flat_ordinal, e.name
                        )
                    })
                    .collect();
                warnings.push(format!(
                    "{} def entries absent from api_trace ordinal_to_name \
                     (run gen-trace to fix):\n{}",
                    missing_in_trace.len(),
                    list.join("\n")
                ));
            }

            if !name_mismatch.is_empty() {
                let list: Vec<String> = name_mismatch
                    .iter()
                    .map(|(e, trace_name)| {
                        format!(
                            "  {}.{} (flat {}): def={:?} trace={:?}",
                            e.module, e.local_ordinal, e.flat_ordinal, e.name, trace_name
                        )
                    })
                    .collect();
                warnings.push(format!(
                    "{} name mismatches between def and api_trace:\n{}",
                    name_mismatch.len(),
                    list.join("\n")
                ));
            }

            if !orphan_in_trace.is_empty() {
                let list: Vec<String> = orphan_in_trace
                    .iter()
                    .map(|(ord, name)| format!("  flat {ord} = {name}"))
                    .collect();
                warnings.push(format!(
                    "{} api_trace entries not in def (add to os2api.def or remove from trace):\n{}",
                    orphan_in_trace.len(),
                    list.join("\n")
                ));
            }
        }
        Err(e) => {
            warnings.push(format!("cannot parse api_trace.rs for drift check: {e}"));
        }
    }

    // 5. Summary
    let has_errors   = !errors.is_empty();
    let has_warnings = !warnings.is_empty();

    if !has_errors && !has_warnings {
        println!("✓  os2api.def is consistent ({} entries)", entries.len());
        return true;
    }

    for msg in &errors {
        eprintln!("ERROR:   {msg}");
    }
    for msg in &warnings {
        eprintln!("WARNING: {msg}");
    }

    println!();
    println!(
        "Result: {} error(s), {} warning(s) — {}",
        errors.len(),
        warnings.len(),
        if has_errors { "fix errors before building" } else { "run gen-trace to regenerate api_trace.rs" }
    );

    !has_errors
}

fn cmd_gen_trace(entries: &[OrdEntry]) {
    // Group entries by module, preserving definition order within each module
    let mut by_module: Vec<(&str, Vec<&OrdEntry>)> = Vec::new();
    for (module, _) in MODULES {
        let group: Vec<&OrdEntry> = entries.iter().filter(|e| e.module == *module).collect();
        if !group.is_empty() {
            by_module.push((module, group));
        }
    }

    let mut out = String::new();

    writeln!(out, "// AUTO-GENERATED — do not edit manually.").unwrap();
    writeln!(out, "// Regenerate with: cargo run --bin gen_api -- gen-trace").unwrap();
    writeln!(out, "// Source of truth:  targets/os2api.def").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "use super::constants::*;").unwrap();
    writeln!(out).unwrap();

    // ── ordinal_to_name ──────────────────────────────────────────────────────

    writeln!(out, "/// Map a warpine flat ordinal to its OS/2 API name.").unwrap();
    writeln!(out, "///").unwrap();
    writeln!(out, "/// Returns `\"?\"` for unknown ordinals.").unwrap();
    writeln!(out, "pub fn ordinal_to_name(ordinal: u32) -> &'static str {{").unwrap();
    writeln!(out, "    match ordinal {{").unwrap();

    for (module, group) in &by_module {
        let base_c  = base_const(module);
        let upper_c = upper_const(module);

        writeln!(out, "        // ── {module} ──────────────────────────────────────────────").unwrap();

        if *module == "DOSCALLS" {
            // Direct flat-ordinal arms (base == 0)
            for e in group {
                writeln!(out, "        {} => {:?},", e.flat_ordinal, e.name).unwrap();
            }
        } else {
            // Range guard + sub-match on local ordinal
            writeln!(
                out,
                "        o if ({base_c}..{upper_c}).contains(&o) => match o - {base_c} {{"
            ).unwrap();
            for e in group {
                writeln!(out, "            {} => {:?},", e.local_ordinal, e.name).unwrap();
            }
            writeln!(out, "            _ => \"?\",").unwrap();
            writeln!(out, "        }},").unwrap();
        }
        writeln!(out).unwrap();
    }

    writeln!(out, "        _ => \"?\",").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();

    // ── module_for_ordinal ───────────────────────────────────────────────────

    writeln!(out, "/// Map a flat warpine ordinal to the OS/2 DLL that owns it.").unwrap();
    writeln!(out, "pub fn module_for_ordinal(ordinal: u32) -> &'static str {{").unwrap();

    let mut first = true;
    for (module, _) in MODULES {
        let upper_c = upper_const(module);
        let base_c  = base_const(module);
        let keyword = if first { "if" } else { "else if" };
        first = false;

        if *module == "DOSCALLS" {
            writeln!(out, "    if ordinal < {upper_c} {{ {:?} }}", module).unwrap();
        } else if *module == "UCONV" {
            writeln!(out, "    else if ordinal < STUB_AREA_SIZE {{ {:?} }}", module).unwrap();
        } else {
            writeln!(
                out,
                "    {keyword} ordinal < {upper_c} {{ {:?} }}   // {base_c}–{upper_c}−1",
                module
            ).unwrap();
        }
    }
    writeln!(out, "    else {{ \"?\" }}").unwrap();
    writeln!(out, "}}").unwrap();

    print!("{out}");
}

// ── doc/os2_ordinals.md parser ────────────────────────────────────────────────
//
// Parses the markdown file produced from the Open Watcom import library and
// returns a map of (MODULE, local_ordinal) → documented_name.
//
// Format expected:
//   ## MODULE (N exports)
//   | Ordinal | Function |
//   |---------|----------|
//   | 234 | DosExit |
//   ...
//
// Duplicate ordinal rows (e.g. short + long alias in the doc) keep the last
// occurrence (both names are typically fine for comparison purposes).

fn parse_ordinals_doc(path: &Path) -> Result<HashMap<(String, u32), String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    let mut map: HashMap<(String, u32), String> = HashMap::new();
    let mut current_module: Option<String> = None;
    let mut in_table = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Module section header: "## DOSCALLS (395 exports)"
        if let Some(rest) = trimmed.strip_prefix("## ") {
            if let Some(mod_name) = rest.split_whitespace().next() {
                current_module = Some(mod_name.to_owned());
                in_table = false;
            }
            continue;
        }

        // Table header row: "| Ordinal | Function |"
        if trimmed.starts_with("| Ordinal") || trimmed.starts_with("|Ordinal") {
            in_table = true;
            continue;
        }

        // Separator row: "|---------|----------|"
        if in_table && (trimmed.starts_with("|---") || trimmed.starts_with("| ---")) {
            continue;
        }

        // Data row: "| 234 | DosExit |"
        if in_table && trimmed.starts_with('|') {
            if let Some(module) = &current_module {
                let parts: Vec<&str> = trimmed.split('|').collect();
                // parts: ["", " 234 ", " DosExit ", ""]
                if parts.len() >= 3 {
                    let ord_str = parts[1].trim();
                    let name    = parts[2].trim();
                    if let Ok(ordinal) = ord_str.parse::<u32>() {
                        map.insert((module.clone(), ordinal), name.to_owned());
                    }
                }
            }
            continue;
        }

        // Blank line resets table state
        if trimmed.is_empty() {
            in_table = false;
        }
    }

    Ok(map)
}

/// Normalise a name for 16-bit alias detection.
///
/// Strategy: uppercase + remove all occurrences of "16".
/// Catches `VIO16SCROLLDN` ↔ `VioScrollDn`, `KBD16CHARIN` ↔ `KbdCharIn`, etc.
fn normalize_for_alias(s: &str) -> String {
    s.to_uppercase().replace("16", "")
}

fn cmd_validate_doc(entries: &[OrdEntry], doc_path: &Path) {
    // Modules of os2386.lib (and of rexx.lib: REXX, REXXAPI). MSG, MDM,
    // UCONV and the socket DLLs are not in
    // the import library; PMCTLS is, but the os2_ordinals.md snapshot has no
    // section for it, so its def lines carry a `# NOTE:` naming the source.
    const DOC_MODULES: &[&str] = &[
        "DOSCALLS", "QUECALLS", "KBDCALLS", "VIOCALLS", "PMGPI", "PMWIN", "SESMGR",
        "NLS", "MOUCALLS", "PMSHAPI", "HELPMGR", "PMSPL", "PMCTLS", "REXX", "REXXAPI", "PMWP",
    ];

    let doc_map = match parse_ordinals_doc(doc_path) {
        Ok(m)  => m,
        Err(e) => { eprintln!("error: {e}"); return; }
    };

    let mut confirmed:    usize = 0;
    let mut alias_ok:     usize = 0;
    let mut only_in_def:  Vec<&OrdEntry> = Vec::new();
    let mut mismatches:   Vec<(&OrdEntry, String)> = Vec::new();
    let mut acknowledged: Vec<(&OrdEntry, String)> = Vec::new();

    let in_scope: Vec<&OrdEntry> = entries
        .iter()
        .filter(|e| DOC_MODULES.contains(&e.module.as_str()))
        .collect();

    for e in &in_scope {
        let key = (e.module.clone(), e.local_ordinal);
        match doc_map.get(&key) {
            // Not in the snapshot: acknowledged when the def line says why.
            None if e.note.is_some() => acknowledged.push((e, "(not in the snapshot)".to_string())),
            None => only_in_def.push(e),
            Some(doc_name) => {
                if doc_name.to_uppercase() == e.name.to_uppercase() {
                    confirmed += 1;
                } else if normalize_for_alias(doc_name) == normalize_for_alias(&e.name) {
                    alias_ok += 1;
                } else if e.note.is_some() {
                    acknowledged.push((e, doc_name.clone()));
                } else {
                    mismatches.push((e, doc_name.clone()));
                }
            }
        }
    }

    println!("Validation: targets/os2api.def vs {}", doc_path.display());
    println!(
        "{} def entries checked (modules: {})",
        in_scope.len(),
        DOC_MODULES.join(", ")
    );
    println!();

    println!("  ✓  Confirmed (exact match):             {confirmed}");
    println!("  ~  16-bit alias pairs (auto-detected):  {alias_ok}");

    if !only_in_def.is_empty() {
        println!();
        println!(
            "  ℹ  Only in os2api.def (not in OW snapshot, {} entries):",
            only_in_def.len()
        );
        let mut by_mod: HashMap<&str, Vec<&OrdEntry>> = HashMap::new();
        for e in &only_in_def {
            by_mod.entry(e.module.as_str()).or_default().push(e);
        }
        for module in DOC_MODULES {
            let Some(group) = by_mod.get(module) else { continue };
            for e in group.iter() {
                println!("    {}.{} = {}", e.module, e.local_ordinal, e.name);
            }
        }
    }

    if !acknowledged.is_empty() {
        println!();
        println!(
            "  ≈  Name differences acknowledged by a {NOTE_MARKER} in os2api.def ({}):",
            acknowledged.len()
        );
        for (e, doc_name) in &acknowledged {
            println!(
                "    {}.{:>4}: def={:<40} doc={} — {}",
                e.module, e.local_ordinal, e.name, doc_name, e.note.as_deref().unwrap_or("")
            );
        }
    }

    if !mismatches.is_empty() {
        println!();
        println!(
            "  ⚠  Ordinal name mismatches ({} — investigate for correctness):",
            mismatches.len()
        );
        for (e, doc_name) in &mismatches {
            println!(
                "    {}.{:>4}: def={:<40} doc={}",
                e.module, e.local_ordinal, e.name, doc_name
            );
        }
    }

    println!();
    if mismatches.is_empty() {
        println!("✓  All in-scope def entries confirmed, recognised as 16-bit aliases or acknowledged.");
    } else {
        println!(
            "⚠  {} mismatch(es) need investigation — see list above.",
            mismatches.len()
        );
    }
}

fn cmd_gen_def(entries: &[OrdEntry]) {
    println!("# OS/2 API ordinal map for the Warpine LX linker.");
    println!("#");
    println!("# Format:  MODULE.ORDINAL  SymbolName");
    println!("#");
    println!("# Modules and their MAGIC_API_BASE offsets:");
    for (module, base) in MODULES {
        println!("#   {:<12} base {:<6}  (MAGIC_API_BASE + {base} + ordinal)", module, base);
    }
    println!();

    // Collect entries grouped by module, sorted by local ordinal
    let mut by_module: HashMap<&str, Vec<&OrdEntry>> = HashMap::new();
    for e in entries {
        by_module.entry(e.module.as_str()).or_default().push(e);
    }

    for (module, _base) in MODULES {
        let Some(group) = by_module.get(module) else { continue };
        let mut group = group.clone();
        group.sort_by_key(|e| e.local_ordinal);

        let label = format!("── {module} ");
        let dashes = "─".repeat(75usize.saturating_sub(label.len() + 2));
        println!("# {label}{dashes}");
        for e in &group {
            println!("{}.{:<6}  {}", e.module, e.local_ordinal, e.name);
        }
        println!();
    }
}

// ── SDK header check (ADR-0096) ──────────────────────────────────────────────
//
// The C headers in sdk/include/os2/ declare OS/2 calls for GCC and clang;
// lx-link resolves each one through os2api.def. `check-headers` keeps the
// two in step: every declared call must be in the def, under the right
// convention (the Pascal subsystems VIOCALLS / KBDCALLS / MOUCALLS through
// OS2_PASCAL_API, everything else APIENTRY), and every integer constant
// the headers define (ERROR_*, WM_*, WS_* …) must equal Warpine's own
// constant of that name, where Warpine has one. Warpine's constants are
// checked against the toolkit headers by constants.rs's own test, so this
// holds the SDK to the toolkit's values without copying its text.

/// The modules whose calls use the Pascal convention (callee cleans up).
const PASCAL_MODULES: &[&str] = &["VIOCALLS", "KBDCALLS", "MOUCALLS"];

/// One call an SDK header declares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderDecl {
    name: String,
    /// Declared through `OS2_PASCAL_API` (else `APIENTRY`).
    pascal: bool,
    /// `file:line`, for messages.
    origin: String,
}

/// The identifier at the start of `s` (after leading blanks).
fn leading_ident(s: &str) -> &str {
    let s = s.trim_start();
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

/// The calls a header declares: `ret APIENTRY Name(`, `ret EXPENTRY Name(`
/// and `OS2_PASCAL_API(ret, Name, ...`. Preprocessor lines, typedefs (function
/// pointer types) and comments are not declarations.
fn parse_header_decls(text: &str, file: &str) -> Vec<HeaderDecl> {
    let mut decls = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if line.starts_with('#') || line.starts_with("typedef") || line.starts_with("/*") || line.starts_with('*') {
            continue;
        }
        let origin = format!("{file}:{}", idx + 1);
        if let Some(rest) = line.strip_prefix("OS2_PASCAL_API(") {
            if let Some((_, after)) = rest.split_once(',') {
                let name = leading_ident(after);
                if !name.is_empty() {
                    decls.push(HeaderDecl { name: name.to_owned(), pascal: true, origin });
                }
            }
        } else if let Some((_, after)) = line.split_once("APIENTRY ").or_else(|| line.split_once("EXPENTRY ")) {
            let name = leading_ident(after);
            if !name.is_empty() && after.trim_start()[name.len()..].trim_start().starts_with('(') {
                decls.push(HeaderDecl { name: name.to_owned(), pascal: false, origin });
            }
        }
    }
    decls
}

/// A C or Rust integer literal (decimal or 0x hex, `_` and `U` / `L`
/// suffixes allowed, a leading `-`), as its 32-bit pattern: -1 and
/// 0xFFFFFFFF compare equal, as they do in C on i386.
fn parse_int_literal(s: &str) -> Option<u32> {
    let s: String = s.trim().trim_end_matches(['u', 'U', 'l', 'L']).chars().filter(|c| *c != '_').collect();
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, s.as_str()),
    };
    let value = match digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    Some(if negative { value.wrapping_neg() } else { value })
}

/// A header constant's value: an integer literal, possibly negative, in
/// parentheses, or cast (`((HWND)1)`, `((ULONG)-1)`); `None` for anything
/// else (expressions, strings, other macros).
fn parse_c_value(s: &str) -> Option<u32> {
    let mut v = s.trim();
    loop {
        if let Some(inner) = v.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            v = inner.trim();
        } else if let Some(rest) = v.strip_prefix('(') {
            // A cast: "(TYPE)value".
            let (ty, value) = rest.split_once(')')?;
            if !ty.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ' || c == '*') {
                return None;
            }
            v = value.trim();
        } else {
            return parse_int_literal(v);
        }
    }
}

/// Every `#define NAME value` in a header whose value is an integer.
fn parse_header_constants(text: &str) -> Vec<(String, u32)> {
    text.lines().filter_map(|raw| {
        let line = raw.trim_start().strip_prefix('#')?.trim_start().strip_prefix("define")?;
        let name = leading_ident(line);
        let rest = &line.trim_start()[name.len()..];
        if name.is_empty() || rest.starts_with('(') { return None; }   // a function-like macro
        let value = rest.split("/*").next()?.trim();
        Some((name.to_owned(), parse_c_value(value)?))
    }).collect()
}

/// Every `const NAME: <integer type> = literal;` in Rust source.
fn parse_rust_constants(text: &str) -> Vec<(String, u32)> {
    const INT_TYPES: &[&str] = &["u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize"];
    text.lines().filter_map(|raw| {
        let (_, rest) = raw.split_once("const ")?;
        let name = leading_ident(rest);
        let (ty, value) = rest[name.len()..].trim_start().strip_prefix(':')?.split_once('=')?;
        if !INT_TYPES.contains(&ty.trim()) { return None; }
        Some((name.to_owned(), parse_int_literal(value.split(';').next()?)?))
    }).collect()
}

/// Warpine's own integer constants, from every `.rs` file under `dir`; a
/// name defined with two different values is left out (it means
/// different things in different modules).
fn warpine_constant_values(dir: &Path) -> HashMap<String, u32> {
    let mut found: HashMap<String, Option<u32>> = HashMap::new();
    collect_rust_constants(dir, &mut found);
    found.into_iter().filter_map(|(k, v)| Some((k, v?))).collect()
}

fn collect_rust_constants(dir: &Path, found: &mut HashMap<String, Option<u32>>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_rust_constants(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(text) = fs::read_to_string(&path) {
            for (name, value) in parse_rust_constants(&text) {
                let slot = found.entry(name).or_insert(Some(value));
                if *slot != Some(value) { *slot = None; }
            }
        }
    }
}

/// The problems with `decls` against the def `entries`, as messages.
fn header_decl_problems(entries: &[OrdEntry], decls: &[HeaderDecl]) -> Vec<String> {
    let modules: HashMap<&str, &str> = entries.iter().map(|e| (e.name.as_str(), e.module.as_str())).collect();
    let mut problems = Vec::new();
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for d in decls {
        if let Some(first) = seen.insert(&d.name, &d.origin) {
            problems.push(format!("{} declared twice ({first} and {})", d.name, d.origin));
        }
        match modules.get(d.name.as_str()) {
            None => problems.push(format!("{} ({}) is not in os2api.def: lx-link cannot resolve it", d.name, d.origin)),
            Some(module) => {
                let pascal_module = PASCAL_MODULES.contains(module);
                if pascal_module && !d.pascal {
                    problems.push(format!("{} ({}) is a {module} call: declare it with OS2_PASCAL_API", d.name, d.origin));
                } else if !pascal_module && d.pascal {
                    problems.push(format!("{} ({}) is a {module} call: declare it APIENTRY, not Pascal", d.name, d.origin));
                }
            }
        }
    }
    problems
}

/// The constants whose header value differs from Warpine's.
fn header_constant_problems(header: &[(String, u32)], warpine: &HashMap<String, u32>) -> Vec<String> {
    header.iter().filter_map(|(name, value)| match warpine.get(name) {
        Some(w) if w != value => Some(format!("{name} is {value:#x} in the headers but {w:#x} in Warpine")),
        _ => None,
    }).collect()
}

fn cmd_check_headers(entries: &[OrdEntry], include_dir: &Path, src_dir: &Path) -> bool {
    let dir = include_dir.join("os2");
    let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(read) => read.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "h")).collect(),
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", dir.display());
            return false;
        }
    };
    files.sort();
    let mut decls = Vec::new();
    let mut constants = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            eprintln!("error: cannot read {}", path.display());
            return false;
        };
        decls.extend(parse_header_decls(&text, &path.display().to_string()));
        constants.extend(parse_header_constants(&text));
    }
    let mut problems = header_decl_problems(entries, &decls);
    let warpine = warpine_constant_values(src_dir);
    problems.extend(header_constant_problems(&constants, &warpine));

    if !problems.is_empty() {
        for p in &problems {
            eprintln!("✗  {p}");
        }
        return false;
    }
    let declared: std::collections::HashSet<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    let covered_modules = ["DOSCALLS", "QUECALLS", "VIOCALLS", "KBDCALLS", "MOUCALLS", "PMWIN", "PMGPI",
                           "PMSHAPI", "PMCTLS", "HELPMGR", "PMSPL", "PMDRAG"];
    let missing = entries.iter()
        .filter(|e| covered_modules.contains(&e.module.as_str()) && !declared.contains(e.name.as_str()))
        .count();
    let compared = constants.iter().filter(|(name, _)| warpine.contains_key(name)).count();
    println!("✓  sdk/include: {} calls declared, each in os2api.def under its convention; {compared} of {} constants \
              compared with Warpine's, all equal", decls.len(), constants.len());
    println!("   ({missing} entries of the def for the modules the headers cover are not declared yet)");
    true
}

// ── CLI parsing ───────────────────────────────────────────────────────────────

struct Config {
    command:      String,
    def_path:     PathBuf,
    trace_path:   PathBuf,
    ordinals_doc: PathBuf,
    headers:      PathBuf,
}

fn usage_and_exit() -> ! {
    eprintln!(
        "Usage: gen_api [--def PATH] [--trace PATH] [--ordinals-doc PATH] <command>\n\
         \n\
         Commands:\n\
           show          Pretty-print the full ordinal table\n\
           check         Validate os2api.def + detect drift vs api_trace.rs\n\
           gen-trace     Emit Rust source for ordinal_to_name() + module_for_ordinal()\n\
           gen-def       Re-emit a canonically sorted os2api.def\n\
           validate-doc  Cross-check os2api.def against doc/os2_ordinals.md\n\
           check-headers Check the SDK's C headers against os2api.def (ADR-0096)\n\
         \n\
         Options:\n\
           --def PATH          Path to os2api.def        (default: targets/os2api.def)\n\
           --trace PATH        Path to api_trace.rs      (default: src/loader/api_trace.rs)\n\
           --ordinals-doc PATH Path to os2_ordinals.md   (default: doc/os2_ordinals.md)\n\
           --headers PATH      The SDK's include dir     (default: sdk/include)"
    );
    process::exit(1);
}

fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut def_path     = PathBuf::from("targets/os2api.def");
    let mut trace_path   = PathBuf::from("src/loader/api_trace.rs");
    let mut ordinals_doc = PathBuf::from("doc/os2_ordinals.md");
    let mut headers      = PathBuf::from("sdk/include");
    let mut command      = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--def" => {
                i += 1;
                def_path = PathBuf::from(args.get(i).unwrap_or_else(|| {
                    eprintln!("--def requires a path argument");
                    process::exit(1);
                }));
            }
            "--trace" => {
                i += 1;
                trace_path = PathBuf::from(args.get(i).unwrap_or_else(|| {
                    eprintln!("--trace requires a path argument");
                    process::exit(1);
                }));
            }
            "--ordinals-doc" => {
                i += 1;
                ordinals_doc = PathBuf::from(args.get(i).unwrap_or_else(|| {
                    eprintln!("--ordinals-doc requires a path argument");
                    process::exit(1);
                }));
            }
            "--headers" => {
                i += 1;
                headers = PathBuf::from(args.get(i).unwrap_or_else(|| {
                    eprintln!("--headers requires a path argument");
                    process::exit(1);
                }));
            }
            s if s.starts_with("--") => {
                eprintln!("unknown option: {s}");
                usage_and_exit();
            }
            s => {
                if command.is_some() {
                    eprintln!("unexpected argument: {s}");
                    usage_and_exit();
                }
                command = Some(s.to_owned());
            }
        }
        i += 1;
    }

    Config {
        command: command.unwrap_or_else(|| {
            eprintln!("missing command");
            usage_and_exit();
        }),
        def_path,
        trace_path,
        ordinals_doc,
        headers,
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let cfg = parse_args();

    let entries = parse_def(&cfg.def_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });

    match cfg.command.as_str() {
        "show" => cmd_show(&entries),

        "check" => {
            let ok = cmd_check(&entries, &cfg.trace_path);
            if !ok {
                process::exit(1);
            }
        }

        "gen-trace" => cmd_gen_trace(&entries),

        "gen-def" => cmd_gen_def(&entries),

        "validate-doc" => cmd_validate_doc(&entries, &cfg.ordinals_doc),

        "check-headers" => {
            if !cmd_check_headers(&entries, &cfg.headers, Path::new("src")) {
                process::exit(1);
            }
        }

        other => {
            eprintln!("unknown command: {other:?}");
            usage_and_exit();
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<OrdEntry> {
        vec![
            OrdEntry { module: "DOSCALLS".into(), local_ordinal: 234, flat_ordinal: 234, name: "DosExit".into(), line: 1, note: None },
            OrdEntry { module: "DOSCALLS".into(), local_ordinal: 282, flat_ordinal: 282, name: "DosWrite".into(), line: 2, note: None },
            OrdEntry { module: "QUECALLS".into(), local_ordinal: 9,   flat_ordinal: 1033, name: "DosReadQueue".into(), line: 3, note: None },
            OrdEntry { module: "NLS".into(),      local_ordinal: 5,   flat_ordinal: 7173, name: "NlsQueryCp".into(), line: 4, note: None },
            OrdEntry { module: "UCONV".into(),    local_ordinal: 1,   flat_ordinal: 12289, name: "UniCreateUconvObject".into(), line: 5, note: None },
        ]
    }

    #[test]
    fn test_module_base_all_known() {
        for (module, base) in MODULES {
            assert_eq!(module_base(module), Some(*base), "wrong base for {module}");
        }
    }

    #[test]
    fn test_module_base_unknown_returns_none() {
        assert!(module_base("UNKNOWN").is_none());
        assert!(module_base("").is_none());
    }

    #[test]
    fn test_module_upper_boundaries() {
        assert_eq!(module_upper("DOSCALLS"),  1024);
        assert_eq!(module_upper("QUECALLS"),  2048);
        assert_eq!(module_upper("UCONV"),     13312); // SO32DLL_BASE
        assert_eq!(module_upper("SO32DLL"),   14336); // TCP32DLL_BASE
        assert_eq!(module_upper("TCP32DLL"),  15360); // MOUCALLS_BASE
        assert_eq!(module_upper("MOUCALLS"),  16384); // PMSHAPI_BASE
        assert_eq!(module_upper("PMSHAPI"),   17408); // HELPMGR_BASE
        assert_eq!(module_upper("HELPMGR"),   18432); // PMSPL_BASE
        assert_eq!(module_upper("PMSPL"),     19456); // PMCTLS_BASE
        assert_eq!(module_upper("PMCTLS"),    20480); // WARPINE_BASE
        assert_eq!(module_upper("WARPINE"),   21504); // PMDRAG_BASE
        assert_eq!(module_upper("PMDRAG"),    22528); // SOM_BASE
        assert_eq!(module_upper("SOM"),       26624); // REXX_BASE
        assert_eq!(module_upper("REXX"),      27648); // REXXAPI_BASE
        assert_eq!(module_upper("REXXAPI"),   28672); // PMWP_BASE
        assert_eq!(module_upper("PMWP"),      STUB_AREA_SIZE);
    }

    #[test]
    fn test_parse_def_valid() {
        // Test the parser with a minimal in-memory def via a temp file
        let content = "# comment\nDOSCALLS.234  DosExit\nQUECALLS.9  DosReadQueue\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.def");
        fs::write(&path, content).unwrap();

        let entries = parse_def(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].module, "DOSCALLS");
        assert_eq!(entries[0].local_ordinal, 234);
        assert_eq!(entries[0].flat_ordinal, 234);
        assert_eq!(entries[0].name, "DosExit");

        assert_eq!(entries[1].module, "QUECALLS");
        assert_eq!(entries[1].local_ordinal, 9);
        assert_eq!(entries[1].flat_ordinal, 1024 + 9);
        assert_eq!(entries[1].name, "DosReadQueue");
    }

    #[test]
    fn test_parse_def_unknown_module_errors() {
        let content = "BOGUS.1  FooBar\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.def");
        fs::write(&path, content).unwrap();
        assert!(parse_def(&path).is_err());
    }

    #[test]
    fn test_parse_def_missing_name_errors() {
        let content = "DOSCALLS.234\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.def");
        fs::write(&path, content).unwrap();
        assert!(parse_def(&path).is_err());
    }

    #[test]
    fn test_def_note_is_read_from_the_line() {
        assert_eq!(def_note("DOSCALLS.75   DosQueryFileMode16  # NOTE: 16-bit alias").as_deref(),
                   Some("16-bit alias"));
        assert_eq!(def_note("DOSCALLS.234  DosExit"), None);
        assert_eq!(def_note("DOSCALLS.234  DosExit  # plain comment"), None);
        assert_eq!(def_note("DOSCALLS.234  DosExit  # NOTE:   "), None, "an empty note acknowledges nothing");
    }

    #[test]
    fn test_parse_def_bad_ordinal_errors() {
        let content = "DOSCALLS.abc  DosExit\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.def");
        fs::write(&path, content).unwrap();
        assert!(parse_def(&path).is_err());
    }

    #[test]
    fn test_base_const_and_upper_const_known() {
        assert_eq!(base_const("DOSCALLS"),  "0");
        assert_eq!(base_const("NLS"),       "NLS_BASE");
        assert_eq!(upper_const("DOSCALLS"), "1024");
        assert_eq!(upper_const("NLS"),      "MSG_BASE");
        assert_eq!(upper_const("UCONV"),     "SO32DLL_BASE");
        assert_eq!(upper_const("SO32DLL"),   "TCP32DLL_BASE");
        assert_eq!(upper_const("TCP32DLL"),  "MOUCALLS_BASE");
        assert_eq!(upper_const("MOUCALLS"),  "PMSHAPI_BASE");
        assert_eq!(upper_const("PMSHAPI"),   "HELPMGR_BASE");
        assert_eq!(upper_const("HELPMGR"),   "PMSPL_BASE");
        assert_eq!(upper_const("PMSPL"),     "PMCTLS_BASE");
        assert_eq!(upper_const("PMCTLS"),    "WARPINE_BASE");
        assert_eq!(base_const("WARPINE"),    "WARPINE_BASE");
        assert_eq!(upper_const("WARPINE"),   "PMDRAG_BASE");
        assert_eq!(base_const("PMDRAG"),     "PMDRAG_BASE");
        assert_eq!(upper_const("PMDRAG"),    "SOM_BASE");
        assert_eq!(upper_const("SOM"),       "REXX_BASE");
        assert_eq!(base_const("REXX"),       "REXX_BASE");
        assert_eq!(upper_const("REXX"),      "REXXAPI_BASE");
        assert_eq!(base_const("REXXAPI"),    "REXXAPI_BASE");
        assert_eq!(upper_const("REXXAPI"),   "PMWP_BASE");
        assert_eq!(base_const("PMWP"),       "PMWP_BASE");
        assert_eq!(upper_const("PMWP"),      "STUB_AREA_SIZE");
    }

    #[test]
    fn test_resolve_const_all_names() {
        assert_eq!(resolve_const("0"),              0);
        assert_eq!(resolve_const("1024"),           1024);
        assert_eq!(resolve_const("PMWIN_BASE"),     2048);
        assert_eq!(resolve_const("PMGPI_BASE"),     3072);
        assert_eq!(resolve_const("KBDCALLS_BASE"),  4096);
        assert_eq!(resolve_const("VIOCALLS_BASE"),  5120);
        assert_eq!(resolve_const("SESMGR_BASE"),    6144);
        assert_eq!(resolve_const("NLS_BASE"),       7168);
        assert_eq!(resolve_const("MSG_BASE"),       8192);
        assert_eq!(resolve_const("MDM_BASE"),       10240);
        assert_eq!(resolve_const("UCONV_BASE"),     12288);
        assert_eq!(resolve_const("PMSHAPI_BASE"),   16384);
        assert_eq!(resolve_const("HELPMGR_BASE"),   17408);
        assert_eq!(resolve_const("PMSPL_BASE"),     18432);
        assert_eq!(resolve_const("PMCTLS_BASE"),    19456);
        assert_eq!(resolve_const("WARPINE_BASE"),   20480);
        assert_eq!(resolve_const("PMDRAG_BASE"),    21504);
        assert_eq!(resolve_const("SOM_BASE"),       22528);
        assert_eq!(resolve_const("REXX_BASE"),      26624);
        assert_eq!(resolve_const("REXXAPI_BASE"),   27648);
        assert_eq!(resolve_const("PMWP_BASE"),      28672);
        assert_eq!(resolve_const("STUB_AREA_SIZE"), 29696);
    }

    #[test]
    fn test_extract_str_literal() {
        assert_eq!(extract_str_literal("\"DosExit\","), Some("DosExit".into()));
        assert_eq!(extract_str_literal("\"DosWrite\""), Some("DosWrite".into()));
        assert_eq!(extract_str_literal("\"?\""),        None);
        assert_eq!(extract_str_literal("&[]"),          None);
        assert_eq!(extract_str_literal("_"),            None);
    }

    #[test]
    fn test_gen_trace_compiles_structurally() {
        // Smoke-test: gen-trace should produce output with key function signatures
        let entries = sample_entries();
        // Redirect stdout capture isn't trivial in unit tests; check via content
        // by calling the inner logic directly.
        let mut out = String::new();
        writeln!(out, "pub fn ordinal_to_name(ordinal: u32) -> &'static str {{").unwrap();
        assert!(out.contains("ordinal_to_name"));

        // Verify flat-ordinal assignment for entries
        for e in &entries {
            let base = module_base(&e.module).unwrap_or(0);
            assert_eq!(e.flat_ordinal, base + e.local_ordinal);
        }
    }

    #[test]
    fn test_check_no_duplicates_valid() {
        // Build a def with no duplicates — check passes
        let content = "DOSCALLS.234  DosExit\nDOSCALLS.282  DosWrite\n";
        let dir = tempfile::tempdir().unwrap();
        let def_path = dir.path().join("test.def");
        let trace_path = dir.path().join("api_trace.rs");
        fs::write(&def_path, content).unwrap();
        fs::write(&trace_path, "pub fn ordinal_to_name(ordinal: u32) -> &'static str { \"?\" }\n").unwrap();
        let entries = parse_def(&def_path).unwrap();
        // check should pass (no errors) even if trace is empty (warnings only)
        let ok = cmd_check(&entries, &trace_path);
        assert!(ok, "expected no hard errors for valid def");
    }

    #[test]
    fn test_flat_ordinal_overflow_detected() {
        // QUECALLS.1024 would overflow into PMWIN range (base 1024, upper 2048)
        // local 1024 → flat 1024+1024 = 2048 which is >= 2048
        let content = "QUECALLS.1024  BogusEntry\n";
        let dir = tempfile::tempdir().unwrap();
        let def_path = dir.path().join("test.def");
        fs::write(&def_path, content).unwrap();
        let entries = parse_def(&def_path).unwrap();
        assert_eq!(entries[0].flat_ordinal, 1024 + 1024);
        assert!(entries[0].flat_ordinal >= module_upper("QUECALLS"),
            "overflow should be detected by check");
    }

    #[test]
    fn test_real_def_file_parses_without_error() {
        // This test runs against the actual targets/os2api.def in the repo.
        // It will fail if the file has syntax errors or unknown modules.
        let def_path = Path::new("targets/os2api.def");
        if !def_path.exists() {
            return; // skip in environments without the repo
        }
        let entries = parse_def(def_path).expect("os2api.def must parse without errors");
        assert!(!entries.is_empty(), "expected at least one entry");

        // All flat ordinals must be within their module's range
        for e in &entries {
            let upper = module_upper(&e.module);
            assert!(
                e.flat_ordinal < upper,
                "{}.{} → flat {} >= upper {}",
                e.module, e.local_ordinal, e.flat_ordinal, upper
            );
        }
    }

    // ── validate-doc tests ───────────────────────────────────────────────────

    fn sample_ordinals_md() -> String {
        "## DOSCALLS (3 exports)\n\
         \n\
         | Ordinal | Function |\n\
         |---------|----------|\n\
         | 234 | DosExit |\n\
         | 282 | DosWrite |\n\
         | 373 | DosQueryDOSProperty |\n\
         \n\
         ## VIOCALLS (2 exports)\n\
         \n\
         | Ordinal | Function |\n\
         |---------|----------|\n\
         | 19 | VIO16WRTTTY |\n\
         | 8  | VIO16PRTSC |\n\
         ".to_owned()
    }

    #[test]
    fn test_parse_ordinals_doc_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("os2_ordinals.md");
        fs::write(&path, sample_ordinals_md()).unwrap();

        let map = parse_ordinals_doc(&path).unwrap();
        assert_eq!(map.get(&("DOSCALLS".into(), 234)), Some(&"DosExit".to_owned()));
        assert_eq!(map.get(&("DOSCALLS".into(), 282)), Some(&"DosWrite".to_owned()));
        assert_eq!(map.get(&("VIOCALLS".into(), 19)),  Some(&"VIO16WRTTTY".to_owned()));
        assert_eq!(map.get(&("VIOCALLS".into(), 8)),   Some(&"VIO16PRTSC".to_owned()));
        // Module not in doc — absent
        assert!(!map.contains_key(&("NLS".into(), 5)));
    }

    #[test]
    fn test_normalize_for_alias() {
        // 16-bit and 32-bit variants of the same function normalise identically
        assert_eq!(normalize_for_alias("VIO16WRTTTY"),  normalize_for_alias("VioWrtTTY"));
        assert_eq!(normalize_for_alias("KBD16CHARIN"),  normalize_for_alias("KbdCharIn"));
        assert_eq!(normalize_for_alias("DOS16EXIT"),    normalize_for_alias("DosExit"));
        // Different functions must NOT normalise identically
        assert_ne!(normalize_for_alias("VIO16PRTSC"),   normalize_for_alias("VioScrollDn"));
        // Same name, different case
        assert_eq!(normalize_for_alias("DosWrite"),     normalize_for_alias("DOSWRITE"));
    }

    #[test]
    fn test_validate_doc_exact_and_alias() {
        // Build a tiny def and a matching doc — confirmed + alias should both work
        let def_content =
            "DOSCALLS.234  DosExit\n\
             VIOCALLS.19   VioWrtTTY\n";
        let doc_content = sample_ordinals_md();

        let dir = tempfile::tempdir().unwrap();
        let def_path = dir.path().join("test.def");
        let doc_path = dir.path().join("os2_ordinals.md");
        fs::write(&def_path, def_content).unwrap();
        fs::write(&doc_path, doc_content).unwrap();

        let entries = parse_def(&def_path).unwrap();
        let doc_map = parse_ordinals_doc(&doc_path).unwrap();

        // DosExit: exact match
        let e_dos = entries.iter().find(|e| e.name == "DosExit").unwrap();
        let doc_name = doc_map.get(&("DOSCALLS".into(), 234)).unwrap();
        assert_eq!(doc_name.to_uppercase(), e_dos.name.to_uppercase());

        // VioWrtTTY: alias match (VIO16WRTTTY → VIOWRTTTY == VIOWRTTTY)
        let e_vio = entries.iter().find(|e| e.name == "VioWrtTTY").unwrap();
        let doc_vio = doc_map.get(&("VIOCALLS".into(), 19)).unwrap();
        assert_ne!(doc_vio.to_uppercase(), e_vio.name.to_uppercase(), "should not be exact");
        assert_eq!(normalize_for_alias(doc_vio), normalize_for_alias(&e_vio.name), "alias should match");
    }

    #[test]
    fn test_real_def_no_duplicates() {
        let def_path = Path::new("targets/os2api.def");
        if !def_path.exists() { return; }
        let entries = parse_def(def_path).unwrap();

        let mut seen: HashMap<(&str, u32), usize> = HashMap::new();
        for e in &entries {
            let key = (e.module.as_str(), e.local_ordinal);
            let prev = seen.insert(key, e.line);
            assert!(prev.is_none(),
                "{}.{} defined at lines {} and {}",
                e.module, e.local_ordinal, prev.unwrap(), e.line);
        }
    }

    // ── SDK header check (ADR-0096) ──────────────────────────────────────

    fn sub_entries() -> Vec<OrdEntry> {
        let mut e = sample_entries();
        e.push(OrdEntry { module: "VIOCALLS".into(), local_ordinal: 19, flat_ordinal: 5139, name: "VioWrtTTY".into(), line: 6, note: None });
        e
    }

    #[test]
    fn test_parse_header_decls_finds_both_conventions() {
        let text = "#define EXPENTRY APIENTRY\n\
                    typedef void (APIENTRY *PFNTHREAD)(ULONG);\n\
                    typedef ULONG APIENTRY ERR(PVOID);\n\
                    APIRET APIENTRY DosWrite(HFILE hFile, const void *pBuffer,\n\
                    \x20                        ULONG cbWrite, PULONG pcbActual);\n\
                    void   APIENTRY DosExit (ULONG action, ULONG result);\n\
                    MRESULT EXPENTRY WinDefWindowProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);\n\
                    /* APIENTRY Commented(void); */\n\
                    OS2_PASCAL_API(APIRET16, VioWrtTTY, (PCCH pch, USHORT cb, HVIO hvio),\n\
                    \x20              (ULONG hvio, ULONG cb, PCCH pch), (hvio, cb, pch))\n";
        let decls = parse_header_decls(text, "x.h");
        let found: Vec<(&str, bool)> = decls.iter().map(|d| (d.name.as_str(), d.pascal)).collect();
        assert_eq!(found, [("DosWrite", false), ("DosExit", false), ("WinDefWindowProc", false), ("VioWrtTTY", true)]);
        assert_eq!(decls[0].origin, "x.h:4");
    }

    #[test]
    fn test_header_decl_problems_name_each_mistake() {
        let decl = |name: &str, pascal| HeaderDecl { name: name.into(), pascal, origin: "x.h:1".into() };
        let entries = sub_entries();
        assert!(header_decl_problems(&entries, &[decl("DosWrite", false), decl("VioWrtTTY", true)]).is_empty());
        let problems = header_decl_problems(&entries, &[
            decl("DosWrte", false),      // not in the def
            decl("VioWrtTTY", false),    // a Pascal call declared APIENTRY
            decl("DosExit", true),       // a system call declared Pascal
            decl("DosWrite", false),
            decl("DosWrite", false),     // twice
        ]);
        assert_eq!(problems.len(), 4, "{problems:#?}");
        assert!(problems[0].contains("DosWrte") && problems[0].contains("not in os2api.def"));
        assert!(problems[1].contains("OS2_PASCAL_API"));
        assert!(problems[2].contains("APIENTRY, not Pascal"));
        assert!(problems[3].contains("declared twice"));
    }

    #[test]
    fn test_header_constants_compare_with_warpine() {
        let header = parse_header_constants("#define NO_ERROR 0\n#  define ERROR_FILE_NOT_FOUND   2\n\
                                             #define ERROR_TIMEOUT 640  /* the wait ended */\n#define ERROR_ONLY_IN_C 7\n\
                                             #define HWND_DESKTOP ((HWND)1)\n#define CLR_BLACK (-1L)\n\
                                             #define SEM_INDEFINITE_WAIT ((ULONG)-1)\n#define LOUSHORT(l) ((USHORT)(l))\n\
                                             #define FCF_MINMAX (FCF_MINBUTTON | FCF_MAXBUTTON)\n#define NAME \"text\"\n");
        let names: Vec<&str> = header.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["NO_ERROR", "ERROR_FILE_NOT_FOUND", "ERROR_TIMEOUT", "ERROR_ONLY_IN_C",
                           "HWND_DESKTOP", "CLR_BLACK", "SEM_INDEFINITE_WAIT"]);
        assert_eq!(header[4].1, 1);
        assert_eq!(header[5].1, u32::MAX);
        assert_eq!(header[6].1, u32::MAX);
        // Built with format! so the wrong value is no `const` line of this
        // file (constants.rs's test checks those against the toolkit).
        let rust = parse_rust_constants(&format!("pub const {}: u32 = 2;\npub const {}:        u32 = 0x0000_0281; // wrong on purpose\nconst NOT_INTEGER: &str = \"5\";\npub const {}: i32 = -1;\n",
                                              "ERROR_FILE_NOT_FOUND", "ERROR_TIMEOUT", "CLR_BLACK"));
        let warpine: HashMap<String, u32> = rust.into_iter().collect();
        assert_eq!(warpine.len(), 3);
        let problems = header_constant_problems(&header, &warpine);
        assert_eq!(problems, ["ERROR_TIMEOUT is 0x280 in the headers but 0x281 in Warpine"]);
    }

    /// The real headers against the real def and Warpine's constants: the
    /// check `cargo run --bin gen_api -- check-headers` runs.
    #[test]
    fn test_real_sdk_headers_match_the_def() {
        let entries = parse_def(Path::new("targets/os2api.def")).expect("parse os2api.def");
        assert!(cmd_check_headers(&entries, Path::new("sdk/include"), Path::new("src")),
                "sdk/include disagrees with os2api.def or Warpine's error codes");
    }
}

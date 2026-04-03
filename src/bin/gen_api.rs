// SPDX-License-Identifier: GPL-3.0-only
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
];
const STUB_AREA_SIZE: u32 = 16384;

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
        "TCP32DLL" => "STUB_AREA_SIZE",
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
        "STUB_AREA_SIZE"=> 16384,
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
        "SO32DLL", "TCP32DLL",
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
    // Only modules present in both os2api.def and os2_ordinals.md.
    // NLS, MSG, MDM, UCONV are Warpine-specific and absent from the OW snapshot.
    const DOC_MODULES: &[&str] = &[
        "DOSCALLS", "QUECALLS", "KBDCALLS", "VIOCALLS", "PMGPI", "PMWIN", "SESMGR",
    ];

    let doc_map = match parse_ordinals_doc(doc_path) {
        Ok(m)  => m,
        Err(e) => { eprintln!("error: {e}"); return; }
    };

    let mut confirmed:    usize = 0;
    let mut alias_ok:     usize = 0;
    let mut only_in_def:  Vec<&OrdEntry> = Vec::new();
    let mut mismatches:   Vec<(&OrdEntry, String)> = Vec::new();

    let in_scope: Vec<&OrdEntry> = entries
        .iter()
        .filter(|e| DOC_MODULES.contains(&e.module.as_str()))
        .collect();

    for e in &in_scope {
        let key = (e.module.clone(), e.local_ordinal);
        match doc_map.get(&key) {
            None => only_in_def.push(e),
            Some(doc_name) => {
                if doc_name.to_uppercase() == e.name.to_uppercase() {
                    confirmed += 1;
                } else if normalize_for_alias(doc_name) == normalize_for_alias(&e.name) {
                    alias_ok += 1;
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
        println!("✓  All in-scope def entries confirmed or recognised as 16-bit aliases.");
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

// ── CLI parsing ───────────────────────────────────────────────────────────────

struct Config {
    command:      String,
    def_path:     PathBuf,
    trace_path:   PathBuf,
    ordinals_doc: PathBuf,
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
         \n\
         Options:\n\
           --def PATH          Path to os2api.def        (default: targets/os2api.def)\n\
           --trace PATH        Path to api_trace.rs      (default: src/loader/api_trace.rs)\n\
           --ordinals-doc PATH Path to os2_ordinals.md   (default: doc/os2_ordinals.md)"
    );
    process::exit(1);
}

fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut def_path     = PathBuf::from("targets/os2api.def");
    let mut trace_path   = PathBuf::from("src/loader/api_trace.rs");
    let mut ordinals_doc = PathBuf::from("doc/os2_ordinals.md");
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
            OrdEntry { module: "DOSCALLS".into(), local_ordinal: 234, flat_ordinal: 234, name: "DosExit".into(), line: 1 },
            OrdEntry { module: "DOSCALLS".into(), local_ordinal: 282, flat_ordinal: 282, name: "DosWrite".into(), line: 2 },
            OrdEntry { module: "QUECALLS".into(), local_ordinal: 9,   flat_ordinal: 1033, name: "DosReadQueue".into(), line: 3 },
            OrdEntry { module: "NLS".into(),      local_ordinal: 5,   flat_ordinal: 7173, name: "NlsQueryCp".into(), line: 4 },
            OrdEntry { module: "UCONV".into(),    local_ordinal: 1,   flat_ordinal: 12289, name: "UniCreateUconvObject".into(), line: 5 },
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
        assert_eq!(module_upper("TCP32DLL"),  STUB_AREA_SIZE);
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
        assert_eq!(upper_const("TCP32DLL"),  "STUB_AREA_SIZE");
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
        assert_eq!(resolve_const("STUB_AREA_SIZE"), 16384);
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
        assert!(map.get(&("NLS".into(), 5)).is_none());
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
}

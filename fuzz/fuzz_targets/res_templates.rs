// SPDX-License-Identifier: GPL-3.0-or-later
//! The resource parsers (src/loader/res_templates.rs): dialog and menu
//! templates, accelerator tables, string bundles and PRESPARAMS blocks come
//! from a running program's resources or its calls, so they are untrusted.
//! The first byte picks the parser: 0 dialog, 1 menu, 2 accelerators,
//! 3 string bundle, 4 PRESPARAMS (others wrap around).
#![no_main]

use libfuzzer_sys::fuzz_target;

/// What res_templates.rs takes from src/loader/constants.rs: the same
/// items, with the same values. The parser is included at the root, so its
/// `super::` is the root too.
mod constants {
    pub const MIS_SEPARATOR: u16 = 0x0004;
    pub const MIS_SUBMENU: u16 = 0x0010;
    pub const MIS_END: u16 = 0x8000;
}

#[path = "../../src/loader/res_templates.rs"]
#[allow(dead_code)]
mod res_templates;

/// Latin-1, as a stand-in for Warpine's code page decoder.
fn decode(bytes: &[u8], _cp: u32) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// How many items a menu tree holds and how deep it goes.
fn menu_shape(items: &[res_templates::MenuItem], depth: usize) -> (usize, usize) {
    items.iter().fold((0, depth), |(n, d), i| {
        let (cn, cd) = menu_shape(&i.children, depth + 1);
        (n + 1 + cn, d.max(cd))
    })
}

fuzz_target!(|data: &[u8]| {
    use res_templates::*;
    let Some((&kind, rest)) = data.split_first() else { return };
    match kind % 5 {
        0 => {
            if let Some(t) = parse_dlg_template(rest, 437, &decode, &|atom| format!("#{atom}")) {
                assert!(t.items.len() <= rest.len() / 30, "no more items than fit");
                assert!(t.focus.is_none_or(|f| f < t.items.len()));
                assert!(t.items.iter().all(|i| i.parent.is_none_or(|p| p < t.items.len())));
                assert!(t.items.iter().all(|i| i.ctl_data.len() <= MAX_DLG_CTL_DATA));
            }
        }
        1 => {
            let (_, depth) = menu_shape(&parse_menu_template(rest, 437, &decode), 0);
            assert!(depth <= MAX_MENU_DEPTH, "the nesting bound holds");
        }
        2 => assert!(parse_accel_table(rest).len() <= rest.len() / ACCEL_SIZE),
        3 => {
            for i in 0..STRINGS_PER_BUNDLE as usize {
                if let Some(s) = bundle_string(rest, i) {
                    assert!(s.len() < 256, "a length byte bounds each string");
                }
            }
        }
        _ => {
            let p = parse_presparams(rest);
            assert!(p.len() <= MAX_PRESPARAMS);
            assert!(p.iter().all(|(_, v)| v.len() <= MAX_PRESPARAM_LEN));
        }
    }
});

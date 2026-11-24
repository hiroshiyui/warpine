// SPDX-License-Identifier: GPL-3.0-or-later
//! The CONFIG.SYS parser (src/loader/config_sys_parse.rs): `C:\CONFIG.SYS`
//! lives on a drive a program can write, so its text is untrusted. It
//! becomes the THREADS= limit, the LIBPATH directories and the SET
//! statements of the starting environment.
#![no_main]

use libfuzzer_sys::fuzz_target;

/// What config_sys_parse.rs takes from src/loader/inherit.rs. The parser
/// is included at the root, so its `super::` is the root.
mod inherit {
    pub const MAX_ENV_BLOCK: usize = 64 * 1024;
}

#[path = "../../src/loader/config_sys_parse.rs"]
#[allow(dead_code)]
mod config_sys_parse;

use config_sys_parse::*;

fuzz_target!(|data: &[u8]| {
    let cfg = ConfigSys::parse(data);
    let dirs = cfg.libpath_dirs();
    assert!(dirs.len() <= MAX_LIBPATH_ENTRIES);
    assert!(dirs.iter().all(|d| !d.is_empty() && d.len() <= MAX_LIBPATH_DIR));
    let env = cfg.environment();
    assert!(env.iter().map(|s| s.len() + 1).sum::<usize>() <= inherit::MAX_ENV_BLOCK, "one environment block");
    let limit = threads_limit(Some(data), None);
    assert!((MIN_THREADS..=MAX_THREADS).contains(&limit));
    assert_eq!(parse_threads(data), cfg.threads);
});

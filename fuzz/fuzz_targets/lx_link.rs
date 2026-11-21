// SPDX-License-Identifier: GPL-3.0-or-later
//! lx-link's readers and linker (ADR-0095): ELF objects, archives,
//! resource files, DLLs and message files come from compilers and
//! libraries a program is built with. The first byte picks the input's
//! kind (by its name, as lx-link does) and --gc-sections; the rest is the
//! input, linked in memory, with its debug companion when it has DWARF.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/bin/lx_link.rs"]
#[allow(dead_code, clippy::all)]
mod lx_link;

const NAMES: [&str; 4] = ["input.o", "input.res", "input.dll", "input.msg"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, input)) = data.split_first() else { return };
    let name = NAMES[usize::from(pick) % NAMES.len()];
    if let Ok(linked) = lx_link::link_in_memory(name, input.to_vec(), pick & 0x80 != 0)
        && linked.has_debug_info() {
        let _ = linked.debug_file();
    }
});

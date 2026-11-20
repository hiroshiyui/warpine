/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * noinit.c - a DLL with exports and no initialization routine, for the
 * loader's unit tests (src/loader/lx_loader.rs, src/lx.rs).
 *
 * Built without the C runtime, so the module has no entry point
 * (eip_object 0). NoInitAdd and NoInitTwice are exported by ordinal (the
 * non-resident names table), NoInitAnswer by ordinal and resident.
 */
int __syscall NoInitAdd( int a, int b ) { return a + b; }
int __syscall NoInitTwice( int a ) { return a * 2; }
int __syscall NoInitAnswer( void ) { return 42; }

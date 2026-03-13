// SPDX-License-Identifier: GPL-3.0-only

pub const MAGIC_API_BASE: u64 = 0x01000000;
pub const EXIT_TRAP_ADDR: u32 = 0x010003FF;
pub const CALLBACK_RET_TRAP: u32 = 0x010003FE;
pub const DYNAMIC_ALLOC_BASE: u32 = 0x02000000; // 32MB

pub const PMWIN_BASE: u32 = 2048;
pub const PMGPI_BASE: u32 = 3072;

// OS/2 WM_ message constants
pub const WM_SIZE: u32 = 0x0007;
pub const WM_PAINT: u32 = 0x0023;
pub const WM_TIMER: u32 = 0x0024;
pub const WM_CLOSE: u32 = 0x0029;
pub const WM_QUIT: u32 = 0x002A;
pub const WM_MOUSEMOVE: u32 = 0x0070;
pub const WM_BUTTON1DOWN: u32 = 0x0071;
pub const WM_BUTTON1UP: u32 = 0x0072;
pub const WM_COMMAND: u32 = 0x0020;
pub const WM_CHAR: u32 = 0x007A;

// Guest memory layout constants
pub const TIB_BASE: u32 = 0x70000;
pub const PIB_BASE: u32 = 0x71000;
pub const ENV_ADDR: u32 = 0x60000;

// SWP flags for WinSetWindowPos
pub const SWP_SIZE: u32 = 0x0001;
pub const SWP_MOVE: u32 = 0x0002;
pub const SWP_ZORDER: u32 = 0x0004;
pub const SWP_SHOW: u32 = 0x0008;
pub const SWP_HIDE: u32 = 0x0010;
pub const SWP_ACTIVATE: u32 = 0x0020;
pub const SWP_MINIMIZE: u32 = 0x0100;
pub const SWP_MAXIMIZE: u32 = 0x0200;
pub const SWP_RESTORE: u32 = 0x0400;

// Mock handle constants
pub const MOCK_HAB: u32 = 0x1234;
pub const MOCK_HPOINTER: u32 = 0x5000;

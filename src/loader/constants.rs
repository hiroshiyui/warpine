// SPDX-License-Identifier: GPL-3.0-only

pub const MAGIC_API_BASE: u64 = 0x01000000;
pub const EXIT_TRAP_ADDR: u32 = 0x010003FF;
pub const CALLBACK_RET_TRAP: u32 = 0x010003FE;
pub const DYNAMIC_ALLOC_BASE: u32 = 0x02000000; // 32MB

pub const PMWIN_BASE: u32 = 2048;
pub const PMGPI_BASE: u32 = 3072;
pub const KBDCALLS_BASE: u32 = 4096;
pub const VIOCALLS_BASE: u32 = 5120;
pub const SESMGR_BASE: u32 = 6144;
pub const NLS_BASE: u32 = 7168;
pub const MSG_BASE: u32 = 8192;
pub const STUB_AREA_SIZE: u32 = 10240;

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

// Guest memory layout constants — placed after loaded objects but below 1MB
// to keep 16-bit segment arithmetic working. LX objects typically end below 0x80000.
pub const TIB_BASE: u32 = 0x00090000;
pub const PIB_BASE: u32 = 0x00091000;
pub const ENV_ADDR: u32 = 0x00092000;

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

// OS/2 error codes
pub const NO_ERROR: u32 = 0;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_INVALID_FUNCTION: u32 = 87;
pub const ERROR_BUFFER_OVERFLOW: u32 = 111;
pub const ERROR_INVALID_LEVEL: u32 = 124;
pub const ERROR_MOD_NOT_FOUND: u32 = 126;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;

// DosQuerySysInfo QSV_* index constants (1-based)
pub const QSV_MAX_PATH_LENGTH: u32 = 1;
pub const QSV_MAX_TEXT_SESSIONS: u32 = 2;
pub const QSV_MAX_PM_SESSIONS: u32 = 3;
pub const QSV_MAX_VDM_SESSIONS: u32 = 4;
pub const QSV_BOOT_DRIVE: u32 = 5;
pub const QSV_DYN_PRI_VARIATION: u32 = 6;
pub const QSV_MAX_WAIT: u32 = 7;
pub const QSV_MIN_SLICE: u32 = 8;
pub const QSV_MAX_SLICE: u32 = 9;
pub const QSV_PAGE_SIZE: u32 = 10;
pub const QSV_VERSION_MAJOR: u32 = 11;
pub const QSV_VERSION_MINOR: u32 = 12;
pub const QSV_VERSION_REVISION: u32 = 13;
pub const QSV_TOTPHYSMEM: u32 = 17;
pub const QSV_TOTRESMEM: u32 = 18;
pub const QSV_TOTAVAILMEM: u32 = 19;
pub const QSV_MAXPRMEM: u32 = 20;
pub const QSV_MAXSHMEM: u32 = 21;
pub const QSV_TIMER_INTERVAL: u32 = 22;
pub const QSV_MAX_COMP_LENGTH: u32 = 23;

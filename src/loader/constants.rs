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
pub const MDM_BASE: u32 = 10240;
pub const UCONV_BASE: u32 = 12288;
pub const STUB_AREA_SIZE: u32 = 16384;

// GDT tiling: 16-bit segment descriptors covering the guest address space
// so that 16:16 (selector:offset) addressing works for OS/2 16-bit thunks.
pub const GDT_BASE: u32 = 0x00080000;
// GDT layout:
//   [0] null, [1] 32-bit code (0x08), [2] 32-bit data (0x10), [3] FS data (0x18),
//   [4] 16-bit data alias (0x20, base=0, limit=0xFFFF) — used for SS in 16-bit mode,
//   [5] 16-bit code alias (0x28, base=0, limit=0xFFFF) — Far16 thunk entry (JMP FAR 0x0028:xxxx),
//   [6..4101] 16-bit data tiles (0x30, 0x38, …) — one per 64KB, read/write,
//   [4102..6149] 16-bit code tiles — matching bases, execute/read, for Far16 JMP/CALL fixups.
pub const TILED_SEL_START_INDEX: u32 = 6;       // data tiles start at GDT[6] (selector 0x30)
pub const TILED_SEL_START: u32 = TILED_SEL_START_INDEX * 8; // selector 0x30
pub const TILE_SIZE: u32 = 0x10000;             // 64KB per tile
pub const NUM_TILES: u32 = 4096;                // 256MB / 64KB (data tiles)
// Code tiles: matching base/limit as data tiles but with execute/read access.
// Used for 16:16 far JMP/CALL fixups targeting executable LX objects.
pub const NUM_CODE_TILES: u32 = 2048;           // 128MB / 64KB (guest memory size)
pub const TILED_CODE_START_INDEX: u32 = TILED_SEL_START_INDEX + NUM_TILES; // GDT[4102]
pub const TILED_CODE_START: u32 = TILED_CODE_START_INDEX * 8;
pub const GDT_ENTRY_COUNT: u32 = 6 + NUM_TILES + NUM_CODE_TILES; // 6 fixed + 4096 data + 2048 code = 6150
pub const GDT_SIZE: u32 = GDT_ENTRY_COUNT * 8;  // 49200 bytes
// IDT relocated after GDT (GDT ends at ~0x8C030)
pub const IDT_BASE: u32 = 0x0008D000;
pub const IDT_HANDLER_BASE: u32 = 0x0008D800;

// NE (16-bit) loader constants
pub const NE_SEGMENT_BASE: u32 = 0x00100000;   // 1MB — NE segments start here
pub const NE_THUNK_BASE: u32 = 0x00F00000;     // 16-bit API thunk stubs at 15MB
pub const NE_THUNK_TILE_INDEX: u32 = NE_THUNK_BASE / TILE_SIZE; // tile 240
// Data tile selector for NE_THUNK_BASE (cannot be used for CALL FAR — data, not code)
pub const NE_THUNK_GDT_INDEX: u32 = TILED_SEL_START_INDEX + NE_THUNK_TILE_INDEX; // GDT[246]
pub const NE_THUNK_SELECTOR: u16 = (NE_THUNK_GDT_INDEX as u16) * 8; // 0x07B0 (data tile)
// Code tile selector for NE_THUNK_BASE — this is the one used in CALL FAR fixups,
// because x86 CALL FAR requires a code (execute) segment descriptor.
pub const NE_THUNK_CODE_GDT_INDEX: u32 = TILED_CODE_START_INDEX + NE_THUNK_TILE_INDEX; // GDT[4342]
pub const NE_THUNK_CODE_SELECTOR: u16 = (NE_THUNK_CODE_GDT_INDEX as u16) * 8; // 0x87B0

// OS/2 WM_ message constants
pub const WM_CREATE: u32 = 0x0001;
pub const WM_SIZE: u32 = 0x0007;
pub const WM_PAINT: u32 = 0x0023;
pub const WM_TIMER: u32 = 0x0024;
pub const WM_CLOSE: u32 = 0x0029;
pub const WM_QUIT: u32 = 0x002A;
pub const WM_MOUSEMOVE: u32 = 0x0070;
pub const WM_BUTTON1DOWN: u32 = 0x0071;
pub const WM_BUTTON1UP: u32 = 0x0072;
pub const WM_BUTTON2DOWN: u32 = 0x0073;
pub const WM_BUTTON2UP: u32 = 0x0074;
pub const WM_BUTTON3DOWN: u32 = 0x0075;
pub const WM_BUTTON3UP: u32 = 0x0076;
pub const WM_COMMAND: u32 = 0x0020;
pub const WM_CHAR: u32 = 0x007A;
pub const WM_ENABLE: u32 = 0x0002;

// OS/2 WC_LISTBOX messages (LM_*)
pub const LM_INSERTITEM:     u32 = 0x0161;
pub const LM_QUERYITEMCOUNT: u32 = 0x0160;
pub const LIT_END:           u32 = 0xFFFF_FFFF; // insert at end of list

// KC_* flags for WM_CHAR message (MP1 high word)
pub const KC_CHAR:       u32 = 0x0001; // character code in MP2 low word is valid
pub const KC_VIRTUALKEY: u32 = 0x0002; // virtual key code in MP2 high word is valid
pub const KC_SCANCODE:   u32 = 0x0004; // hardware scan code in MP1 byte 8-15 is valid
pub const KC_SHIFT:      u32 = 0x0008; // shift key held
pub const KC_CTRL:       u32 = 0x0010; // Ctrl key held
pub const KC_ALT:        u32 = 0x0020; // Alt key held
pub const KC_KEYUP:      u32 = 0x0040; // key-release event
pub const KC_PREVDOWN:   u32 = 0x0080; // key was already down (auto-repeat)
pub const KC_LONEKEY:    u32 = 0x0100; // key pressed without any other key

// VK_* virtual key codes for WM_CHAR MP2 high word
pub const VK_BACKSPACE:  u32 = 0x05;
pub const VK_TAB:        u32 = 0x06;
pub const VK_NEWLINE:    u32 = 0x08; // main Enter key
pub const VK_ESC:        u32 = 0x0f;
pub const VK_SPACE:      u32 = 0x10;
pub const VK_PAGEUP:     u32 = 0x11;
pub const VK_PAGEDOWN:   u32 = 0x12;
pub const VK_END:        u32 = 0x13;
pub const VK_HOME:       u32 = 0x14;
pub const VK_LEFT:       u32 = 0x15;
pub const VK_UP:         u32 = 0x16;
pub const VK_RIGHT:      u32 = 0x17;
pub const VK_DOWN:       u32 = 0x18;
pub const VK_INSERT:     u32 = 0x1a;
pub const VK_DELETE:     u32 = 0x1b;
pub const VK_SCRLLOCK:   u32 = 0x1c;
pub const VK_NUMLOCK:    u32 = 0x1d;
pub const VK_ENTER:      u32 = 0x1e; // keypad Enter
pub const VK_F1:         u32 = 0x20;
pub const VK_F2:         u32 = 0x21;
pub const VK_F3:         u32 = 0x22;
pub const VK_F4:         u32 = 0x23;
pub const VK_F5:         u32 = 0x24;
pub const VK_F6:         u32 = 0x25;
pub const VK_F7:         u32 = 0x26;
pub const VK_F8:         u32 = 0x27;
pub const VK_F9:         u32 = 0x28;
pub const VK_F10:        u32 = 0x29;
pub const VK_F11:        u32 = 0x2a;
pub const VK_F12:        u32 = 0x2b;

// Clipboard format constants (CF_*)
pub const CF_TEXT:   u32 = 1;
pub const CF_BITMAP: u32 = 2;

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

// OS/2 HWND Z-order sentinels used with SWP_ZORDER in WinSetWindowPos.
pub const HWND_TOP: u32          = 1; // Place window at the top of Z-order
pub const HWND_BOTTOM: u32       = 2; // Place window at the bottom of Z-order
pub const HWND_FLOAT: u32        = 3; // Float above all normal windows (mapped to HWND_TOP)
pub const HWND_DESKTOP: u32      = 4; // PM desktop pseudo-handle (origin of coordinate space)

// ── FCF_* frame creation flags (flCreateFlags in WinCreateStdWindow) ─────────
// Values match OS/2 Warp 4 Toolkit <pmwin.h>.
pub const FCF_TITLEBAR:      u32 = 0x0000_0001; // draw title bar chrome
pub const FCF_SYSMENU:       u32 = 0x0000_0002; // system menu icon in title bar
pub const FCF_MENU:          u32 = 0x0000_0004; // load/attach a menu bar
pub const FCF_SIZEBORDER:    u32 = 0x0000_0008; // resize border (SB_*)
pub const FCF_MINBUTTON:     u32 = 0x0000_0010; // minimize button
pub const FCF_MAXBUTTON:     u32 = 0x0000_0020; // maximize button
pub const FCF_MINMAX:        u32 = 0x0000_0030; // min + max buttons
pub const FCF_VERTSCROLL:    u32 = 0x0000_0040; // vertical scroll bar
pub const FCF_HORZSCROLL:    u32 = 0x0000_0080; // horizontal scroll bar
pub const FCF_DLGBORDER:     u32 = 0x0000_0100; // dialog border (thicker, no resize)
pub const FCF_BORDER:        u32 = 0x0000_0200; // thin single-pixel border
pub const FCF_SHELLPOSITION: u32 = 0x0000_0400; // let shell determine initial position
pub const FCF_TASKLIST:      u32 = 0x0000_0800; // appear in task list / window list
pub const FCF_NOBYTEALIGN:   u32 = 0x0000_1000; // don't byte-align window position
pub const FCF_ICON:          u32 = 0x0000_4000; // title bar icon (requires hmod resource)
pub const FCF_ACCELTABLE:    u32 = 0x0000_8000; // load accelerator table from hmod
pub const FCF_SYSMODAL:      u32 = 0x0001_0000; // system-modal frame
pub const FCF_STANDARD:      u32 = 0x0000_0CC3; // FCF_TITLEBAR|FCF_SYSMENU|FCF_MINMAX|FCF_SIZEBORDER|FCF_TASKLIST

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
pub const ERROR_INVALID_DRIVE: u32 = 15;
pub const ERROR_INVALID_FUNCTION: u32 = 87;
pub const ERROR_BUFFER_OVERFLOW: u32 = 111;
pub const ERROR_INVALID_LEVEL: u32 = 124;
pub const ERROR_MOD_NOT_FOUND: u32 = 126;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;
pub const ERROR_ENVVAR_NOT_FOUND: u32 = 204;
pub const ERROR_INIT_ROUTINE_FAILED: u32 = 199;
pub const ERROR_INVALID_PARAMETER:    u32 = 87;
pub const ERROR_INVALID_CODE_PAGE:    u32 = 470;
pub const ERROR_NESTING_NOT_ALLOWED:  u32 = 215;

// ── VIO error codes ───────────────────────────────────────────────────────────
pub const ERROR_VIO_ROW: u32 = 426; // Row parameter out of range
pub const ERROR_VIO_COL: u32 = 427; // Column parameter out of range

// ── WC_* built-in window class atoms ─────────────────────────────────────────
// These are the numeric atom values passed as the pszClass pointer to
// WinCreateWindow when creating built-in PM controls (OS/2 pmwin.h).
// OS/2 built-in window-class atoms.  PM encodes them as MAKEINTATOM(n)
// = 0xFFFF0000 | n, following the same pattern as Win16 class atoms.
pub const WC_FRAME_ATOM:      u32 = 0xFFFF_0001;
pub const WC_COMBOBOX_ATOM:   u32 = 0xFFFF_0002;
pub const WC_BUTTON_ATOM:     u32 = 0xFFFF_0003;
pub const WC_MENU_ATOM:       u32 = 0xFFFF_0004;
pub const WC_STATIC_ATOM:     u32 = 0xFFFF_0005;
pub const WC_ENTRYFIELD_ATOM: u32 = 0xFFFF_0006;
pub const WC_LISTBOX_ATOM:    u32 = 0xFFFF_0007;
pub const WC_SCROLLBAR_ATOM:  u32 = 0xFFFF_0008;
pub const WC_TITLEBAR_ATOM:   u32 = 0xFFFF_0009;
pub const WC_MLE_ATOM:        u32 = 0xFFFF_000A;
pub const WC_SPINBUTTON_ATOM: u32 = 0xFFFF_000F;
pub const WC_CONTAINER_ATOM:  u32 = 0xFFFF_0025;
pub const WC_NOTEBOOK_ATOM:   u32 = 0xFFFF_0028;

/// Height of the PM menu bar in pixels (OS/2-space units).
pub const MENU_BAR_HEIGHT: i16 = 20;

// Dialog and menu message constants
pub const WM_INITDLG:    u32 = 0x0021; // First message to a dialog box
pub const WM_INITMENU:   u32 = 0x00E5; // Sent before a menu is displayed
pub const WM_MENUSELECT: u32 = 0x0078; // Menu item selected (non-activating)
pub const WM_MENUEND:    u32 = 0x0079; // Menu dismissed

// Dialog result IDs returned by WinDlgBox / WinProcessDlg
pub const DID_OK:     u32 = 1;         // OK button
pub const DID_CANCEL: u32 = 2;         // Cancel button
pub const DID_ERROR:  u32 = 0xFFFF;    // Error return

// ── MIS_* menu item style flags (afStyle in MENUITEM) ───────────────────────
// Values match OS/2 Warp 4 Toolkit <pmwin.h> exactly.
pub const MIS_TEXT:            u16 = 0x0001;
pub const MIS_BITMAP:          u16 = 0x0002;
pub const MIS_SEPARATOR:       u16 = 0x0004;
pub const MIS_OWNERDRAW:       u16 = 0x0008;
pub const MIS_SUBMENU:         u16 = 0x0010;
pub const MIS_MULTMENU:        u16 = 0x0020;
pub const MIS_SYSCOMMAND:      u16 = 0x0040;
pub const MIS_HELP:            u16 = 0x0080;
pub const MIS_STATIC:          u16 = 0x0100;
pub const MIS_BUTTONSEPARATOR: u16 = 0x0200;
pub const MIS_BREAK:           u16 = 0x0400;
pub const MIS_BREAKSEPARATOR:  u16 = 0x0800;
pub const MIS_GROUP:           u16 = 0x1000;
pub const MIS_SINGLE:          u16 = 0x2000;
/// Binary resource end-of-level marker: bit 15 of afStyle.
/// Present in some hand-built templates; wrc uses count-based format instead.
pub const MIS_END:             u16 = 0x8000;

// ── MIA_* menu item attribute flags (afAttribute in MENUITEM) ────────────────
pub const MIA_NODISMISS: u16 = 0x0020;
pub const MIA_FRAMED:    u16 = 0x1000;
pub const MIA_CHECKED:   u16 = 0x2000;
pub const MIA_DISABLED:  u16 = 0x4000;
pub const MIA_HILITED:   u16 = 0x8000;

// WM_SYSCOMMAND (0x000A) — system command sent by frame chrome to the frame.
// mp1 = SC_* command code, mp2 = unused.
pub const WM_SYSCOMMAND: u32 = 0x000A;

// SC_* system command codes (sent in mp1 of WM_SYSCOMMAND).
pub const SC_CLOSE:    u32 = 0x8004; // close the window
pub const SC_MINIMIZE: u32 = 0x8002; // minimize the window
pub const SC_MAXIMIZE: u32 = 0x8003; // maximize the window
pub const SC_RESTORE:  u32 = 0x8080; // restore from min/max

// WM_ACTIVATE (0x0042) — sent to a frame when it gains or loses activation.
// mp1 = fActivate (TRUE = gaining focus, FALSE = losing); mp2 = other frame HWND.
pub const WM_ACTIVATE: u32 = 0x0042;

// WM_CONTROL — notification message from a child control to its owner.
// mp1 = MPFROM2SHORT(usID, usNotifyCode); mp2 = control-specific data.
pub const WM_CONTROL: u32 = 0x0030;

// Button notification codes (high word of WM_CONTROL mp1)
pub const BN_CLICKED: u32 = 0;
pub const BN_DBLCLICKED: u32 = 1;

// VDR chrome dimensions — must match CHROME_TITLE_H in sdl2_renderer.rs
pub const CHROME_TITLE_H: i32 = 20;

// Common window style bits (flStyle in WinCreateWindow)
pub const WS_VISIBLE:  u32 = 0x8000_0000;
pub const WS_DISABLED: u32 = 0x4000_0000;
pub const WS_TABSTOP:  u32 = 0x0002_0000;
pub const WS_GROUP:    u32 = 0x0001_0000;

// Button styles (low 16 bits of flStyle for WC_BUTTON windows)
pub const BS_PUSHBUTTON:     u32 = 0x0000;
pub const BS_CHECKBOX:       u32 = 0x0001;
pub const BS_AUTOCHECKBOX:   u32 = 0x0003;
pub const BS_RADIOBUTTON:    u32 = 0x0005;
pub const BS_AUTORADIOBUTTON: u32 = 0x0007;

// DosSetExtLIBPATH / DosQueryExtLIBPATH flags
pub const BEGIN_LIBPATH: u32 = 1;
pub const END_LIBPATH: u32 = 2;

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

// WinMessageBox style flags (flStyle) — low 4 bits select the button set
pub const MB_OK:               u32 = 0x0000;
pub const MB_OKCANCEL:         u32 = 0x0001;
pub const MB_RETRYCANCEL:      u32 = 0x0002;
pub const MB_ABORTRETRYIGNORE: u32 = 0x0003;
pub const MB_YESNO:            u32 = 0x0004;
pub const MB_YESNOCANCEL:      u32 = 0x0005;
pub const MB_CANCEL:           u32 = 0x0006;
pub const MB_ENTER:            u32 = 0x0007;
pub const MB_ENTERCANCEL:      u32 = 0x0008;

// Icon bits (bits 4–7 of flStyle)
pub const MB_NOICON:           u32 = 0x0000;
pub const MB_ICONHAND:         u32 = 0x0010; // error
pub const MB_ICONQUESTION:     u32 = 0x0020; // question
pub const MB_ICONEXCLAMATION:  u32 = 0x0030; // warning
pub const MB_ICONASTERISK:     u32 = 0x0040; // information

// WinMessageBox return values (MBID_*)
pub const MBID_OK:     u32 = 1;
pub const MBID_CANCEL: u32 = 2;
pub const MBID_ABORT:  u32 = 3;
pub const MBID_RETRY:  u32 = 4;
pub const MBID_IGNORE: u32 = 5;
pub const MBID_YES:    u32 = 6;
pub const MBID_NO:     u32 = 7;
pub const MBID_ENTER:  u32 = 8;

// ── OS/2 Structured Exception Handling (SEH) ─────────────────────────────────

/// TIB byte offset of `tib_pexchain` (exception handler chain head pointer).
pub const TIB_EXCHAIN_OFFSET: u32 = 0x00;
/// Sentinel stored at the tail of the exception handler chain (no further handlers).
pub const XCPT_CHAIN_END: u32 = 0xFFFF_FFFF;

// Exception handler return values (from the handler function)
/// Handler fixed the exception — resume at the fault address.
pub const XCPT_CONTINUE_EXECUTION: u32 = 0xFFFF_FFFF;
/// Handler did not handle the exception — try the next one in the chain.
pub const XCPT_CONTINUE_SEARCH:    u32 = 0x0000_0001;

// Exception report flags (fHandlerFlags in EXCEPTIONREPORTRECORD)
pub const EH_NONCONTINUABLE: u32 = 0x0000_0001;
pub const EH_UNWINDING:      u32 = 0x0000_0002;
pub const EH_EXIT_UNWIND:    u32 = 0x0000_0004;
pub const EH_NESTED_CALL:    u32 = 0x0000_0010;

// CONTEXTRECORD.ContextFlags bits
pub const CONTEXT_CONTROL:  u32 = 0x0001; // CS, EIP, EFLAGS, SS, ESP
pub const CONTEXT_INTEGER:  u32 = 0x0002; // EAX, EBX, ECX, EDX, ESI, EDI, EBP
pub const CONTEXT_SEGMENTS: u32 = 0x0004; // DS, ES, FS, GS
pub const CONTEXT_FULL: u32 = CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_SEGMENTS;

// CONTEXTRECORD guest layout — byte offsets from record base
// Offsets 0x04–0x8B are FPU env/save area (zeroed; not emulated here).
pub const CTX_FLAGS:  u32 = 0x00;
pub const CTX_GS:     u32 = 0x8C;
pub const CTX_FS:     u32 = 0x90;
pub const CTX_ES:     u32 = 0x94;
pub const CTX_DS:     u32 = 0x98;
pub const CTX_EDI:    u32 = 0x9C;
pub const CTX_ESI:    u32 = 0xA0;
pub const CTX_EAX:    u32 = 0xA4;
pub const CTX_EBX:    u32 = 0xA8;
pub const CTX_ECX:    u32 = 0xAC;
pub const CTX_EDX:    u32 = 0xB0;
pub const CTX_EBP:    u32 = 0xB4;
pub const CTX_EIP:    u32 = 0xB8;
pub const CTX_CS:     u32 = 0xBC;
pub const CTX_EFLAGS: u32 = 0xC0;
pub const CTX_ESP:    u32 = 0xC4;
pub const CTX_SS:     u32 = 0xC8;
/// Total size of the CONTEXTRECORD guest structure (0xCC bytes).
pub const CONTEXT_RECORD_SIZE: u32 = 0xCC;

// EXCEPTIONREPORTRECORD guest layout — byte offsets
pub const ERR_NUM:    u32 = 0x00; // ExceptionNum (XCPT_* code)
pub const ERR_FLAGS:  u32 = 0x04; // fHandlerFlags (EH_* bitmask)
pub const ERR_NESTED: u32 = 0x08; // NestedExceptionReportRecord ptr
pub const ERR_ADDR:   u32 = 0x0C; // ExceptionAddress
pub const ERR_CPARMS: u32 = 0x10; // cParameters
pub const ERR_PARAMS: u32 = 0x14; // ExceptionInfo[9] (9 × u32 = 36 bytes)
/// Total size of the EXCEPTIONREPORTRECORD (0x38 bytes).
pub const EXCEPTION_REPORT_SIZE: u32 = 0x38;

// EXCEPTIONREGISTRATIONRECORD guest layout — byte offsets
pub const XERREC_PREV:    u32 = 0x00; // prev_structure (next node toward chain end)
pub const XERREC_HANDLER: u32 = 0x04; // ExceptionHandler function pointer

// OS/2 exception codes (XCPT_*) — from IBM bseexpt.h / OS/2 Warp toolkit
pub const XCPT_GUARD_PAGE_VIOLATION:    u32 = 0xC000_0001;
pub const XCPT_UNABLE_TO_GROW_STACK:    u32 = 0xC000_0003;
pub const XCPT_ACCESS_VIOLATION:        u32 = 0xC000_0005;
pub const XCPT_IN_PAGE_ERROR:           u32 = 0xC000_0006;
pub const XCPT_ILLEGAL_INSTRUCTION:     u32 = 0xC000_001C;
pub const XCPT_INVALID_LOCK_SEQUENCE:   u32 = 0xC000_001E;
pub const XCPT_FLOAT_DENORMAL_OPERAND:  u32 = 0xC000_0048;
pub const XCPT_FLOAT_DIVIDE_BY_ZERO:    u32 = 0xC000_0049;
pub const XCPT_FLOAT_INEXACT_RESULT:    u32 = 0xC000_004A;
pub const XCPT_FLOAT_INVALID_OPERATION: u32 = 0xC000_004B;
pub const XCPT_FLOAT_OVERFLOW:          u32 = 0xC000_004C;
pub const XCPT_FLOAT_STACK_CHECK:       u32 = 0xC000_004D;
pub const XCPT_FLOAT_UNDERFLOW:         u32 = 0xC000_004E;
pub const XCPT_INTEGER_DIVIDE_BY_ZERO:  u32 = 0xC000_009B;
pub const XCPT_INTEGER_OVERFLOW:        u32 = 0xC000_0097;
pub const XCPT_PRIVILEGED_INSTRUCTION:  u32 = 0xC000_0096;
pub const XCPT_DATATYPE_MISALIGNMENT:   u32 = 0xC000_009E;
pub const XCPT_BREAKPOINT:              u32 = 0xC000_0009;
pub const XCPT_SINGLE_STEP:             u32 = 0xC000_000A;
pub const XCPT_SIGNAL:                  u32 = 0xC001_0003;
/// Catch-all for unclassified hardware faults.
pub const XCPT_FATAL_EXCEPTION:         u32 = 0xC000_FFFF;

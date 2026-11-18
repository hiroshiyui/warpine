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
pub const UCONV_BASE: u32    = 12288;
pub const SO32DLL_BASE: u32  = 13312; // 13 * 1024
pub const TCP32DLL_BASE: u32 = 14336; // 14 * 1024
pub const MOUCALLS_BASE: u32 = 15360; // 15 * 1024
pub const PMSHAPI_BASE: u32  = 16384; // 16 * 1024 — profiles (Prf*)
pub const HELPMGR_BASE: u32  = 17408; // 17 * 1024 — Help Manager
pub const PMSPL_BASE: u32    = 18432; // 18 * 1024 — spooler (Spl*), no printers
pub const PMCTLS_BASE: u32   = 19456; // 19 * 1024 — standard file / font dialogs
pub const STUB_AREA_SIZE: u32 = 20480;

// Compile-time bounds checks: each module range is 1024 ordinals wide and
// PMCTLS, the last one, must fit within STUB_AREA_SIZE.
const _: () = assert!(TCP32DLL_BASE + 1024 <= MOUCALLS_BASE);
const _: () = assert!(MOUCALLS_BASE + 1024 <= PMSHAPI_BASE);
const _: () = assert!(PMSHAPI_BASE + 1024 <= HELPMGR_BASE);
const _: () = assert!(HELPMGR_BASE + 1024 <= PMSPL_BASE);
const _: () = assert!(PMSPL_BASE + 1024 <= PMCTLS_BASE);
const _: () = assert!(PMCTLS_BASE + 1024 <= STUB_AREA_SIZE);
// The INT 3 stub area must not reach the dynamic allocation pool.
const _: () = assert!(MAGIC_API_BASE + STUB_AREA_SIZE as u64 <= DYNAMIC_ALLOC_BASE as u64);
// The NE loader addresses stubs with 16-bit offsets.
const _: () = assert!(STUB_AREA_SIZE <= u16::MAX as u32);

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
/// Sent to a window being destroyed (`WinDestroyWindow`), after it has been
/// hidden and before its children are destroyed.
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0007;
/// First message number free for applications.
pub const WM_USER: u32 = 0x1000;
/// Sent to a frame after frame controls were added or removed; mp1 = the
/// `FCF_*` flags of those controls.
pub const WM_UPDATEFRAME: u32 = 0x0042;
/// Identity of a frame's menu-bar window (`WinLoadMenu`, `WinCreateMenu`).
pub const FID_MENU: u32 = 0x8005;
/// Identity of a frame's client window (`<pmwin.h>`).
pub const FID_CLIENT: u32 = 0x8008;
/// Frame formatting messages (`<pmwin.h>`, pm_frame.rs): mp1 = `SWP`
/// array, mp2 = client `RECTL`; the count of frame-control `SWP`s; mp1 =
/// `RECTL`, mp2 = TRUE when it is the frame rectangle.
pub const WM_FORMATFRAME: u32 = 0x0041;
pub const WM_QUERYFRAMECTLCOUNT: u32 = 0x0059;
pub const WM_CALCFRAMERECT: u32 = 0x0053;
pub const WM_PAINT: u32 = 0x0023;
pub const WM_TIMER: u32 = 0x0024;
pub const WM_CLOSE: u32 = 0x0029;
pub const WM_QUIT: u32 = 0x002A;
pub const WM_MOUSEMOVE: u32 = 0x0070;
pub const WM_BUTTON1DOWN: u32 = 0x0071;
pub const WM_BUTTON1UP: u32 = 0x0072;
/// The second press of a double click (instead of a second `WM_BUTTON1DOWN`).
pub const WM_BUTTON1DBLCLK: u32 = 0x0073;
pub const WM_BUTTON2DOWN: u32 = 0x0074;
pub const WM_BUTTON2UP: u32 = 0x0075;
pub const WM_BUTTON2DBLCLK: u32 = 0x0076;
pub const WM_BUTTON3DOWN: u32 = 0x0077;
pub const WM_BUTTON3UP: u32 = 0x0078;
pub const WM_BUTTON3DBLCLK: u32 = 0x0079;
pub const WM_COMMAND: u32 = 0x0020;
pub const WM_CHAR: u32 = 0x007A;
pub const WM_ENABLE: u32 = 0x0004;

// OS/2 WC_LISTBOX messages (LM_*)
pub const LM_INSERTITEM:     u32 = 0x0161;
pub const LM_QUERYITEMCOUNT: u32 = 0x0160;
pub const LIT_END:           u32 = 0xFFFF_FFFF; // insert at end of list
/// mp1 = item index (`LIT_NONE` deselects everything), mp2 = TRUE to select.
pub const LM_SELECTITEM:     u32 = 0x0164;
/// mp1 = index to search after (`LIT_FIRST` from the top); returns the
/// selected index or `LIT_NONE`.
pub const LM_QUERYSELECTION: u32 = 0x0165;
/// mp1 = item index; returns the number of items left.
pub const LM_DELETEITEM:     u32 = 0x0163;
/// Removes every item; returns TRUE.
pub const LM_DELETEALL:      u32 = 0x016e;
/// `LM_QUERYSELECTION` index asking for the item with the cursor.
pub const LIT_CURSOR: i32 = -4;
/// List box style: a click or Space toggles one item, keeping the others.
pub const LS_MULTIPLESEL: u32 = 0x0001;
/// List box style: Shift selects ranges and Ctrl+click toggles items.
pub const LS_EXTENDEDSEL: u32 = 0x0010;
/// "No item": `LM_QUERYSELECTION` result and `LM_SELECTITEM` deselect-all index.
pub const LIT_NONE:  i32 = -1;
/// `LM_QUERYSELECTION` start index meaning "search from the first item".
pub const LIT_FIRST: i32 = -1;

// List box notification codes (high word of WM_CONTROL mp1); mp2 = list box HWND.
pub const LN_SELECT:    u32 = 1;
pub const LN_SETFOCUS:  u32 = 2;
pub const LN_KILLFOCUS: u32 = 3;
/// Enter pressed in, or an item double-clicked.
pub const LN_ENTER:     u32 = 5;

// Entry field notification codes (high word of WM_CONTROL mp1); mp2 = field HWND.
pub const EN_SETFOCUS:  u32 = 0x0001;
pub const EN_KILLFOCUS: u32 = 0x0002;
pub const EN_CHANGE:    u32 = 0x0004;
pub const EN_SCROLL:    u32 = 0x0008;
pub const EN_MEMERROR:  u32 = 0x0010;

/// Entry field style: the text can be selected and copied but not changed.
pub const ES_READONLY: u32 = 0x0000_0020;
/// MLE style: the text can be selected and copied but not changed.
pub const MLS_READONLY: u32 = 0x0000_0010;
/// MLE style: lines wider than the control wrap at word boundaries.
pub const MLS_WORDWRAP: u32 = 0x0000_0001;

// WC_ENTRYFIELD messages (EM_*, <pmwin.h>)
pub const EM_QUERYSEL: u32 = 0x0141;
pub const EM_SETSEL:   u32 = 0x0142;
pub const EM_CUT:      u32 = 0x0144;
pub const EM_COPY:     u32 = 0x0145;
pub const EM_CLEAR:    u32 = 0x0146;
pub const EM_PASTE:    u32 = 0x0147;
/// Returns TRUE when the field is read-only.
pub const EM_QUERYREADONLY: u32 = 0x014a;
/// mp1 = TRUE for read-only; returns the previous state.
pub const EM_SETREADONLY:   u32 = 0x014b;

/// The first and last `EM_*` message.
pub const EM_QUERYCHANGED:  u32 = 0x0140;
pub const EM_SETINSERTMODE: u32 = 0x014c;
/// mp1 = maximum number of characters (`EM_SETTEXTLIMIT`); returns TRUE.
pub const EM_SETTEXTLIMIT: u32 = 0x0143;
/// Entry field styles: text alignment (`ES_LEFT` is 0).
pub const ES_CENTER: u32 = 0x0000_0001;
pub const ES_RIGHT:  u32 = 0x0000_0002;

// More WC_LISTBOX messages (LM_*, <pmwin.h>).
pub const LM_SETTOPINDEX:         u32 = 0x0162;
pub const LM_SETITEMTEXT:         u32 = 0x0166;
pub const LM_QUERYITEMTEXTLENGTH: u32 = 0x0167;
/// mp1 = MPFROM2SHORT(index, cchMax), mp2 = buffer; returns the length.
pub const LM_QUERYITEMTEXT:       u32 = 0x0168;
pub const LM_SETITEMHANDLE:       u32 = 0x0169;
pub const LM_QUERYITEMHANDLE:     u32 = 0x016a;
/// mp1 = MPFROM2SHORT(LSS_* flags, index to start after), mp2 = string.
pub const LM_SEARCHSTRING:        u32 = 0x016b;
pub const LM_SETITEMHEIGHT:       u32 = 0x016c;
pub const LM_QUERYTOPINDEX:       u32 = 0x016d;
/// `LM_*` error results.
pub const LIT_ERROR: i32 = -3;
pub const LIT_MEMERROR: i32 = -2;
/// `LM_INSERTITEM` positions: in ascending / descending order.
pub const LIT_SORTASCENDING: i32 = -2;
pub const LIT_SORTDESCENDING: i32 = -3;
/// `LM_SEARCHSTRING` flags.
pub const LSS_SUBSTRING:     u32 = 1;
pub const LSS_PREFIX:        u32 = 2;
pub const LSS_CASESENSITIVE: u32 = 4;
/// List box style: the owner draws the items (`WM_MEASUREITEM`, `WM_DRAWITEM`).
pub const LS_OWNERDRAW: u32 = 0x0002;
/// List box notification: the list is about to scroll.
pub const LN_SCROLL: u32 = 4;

// Owner drawing (<pmwin.h>).
/// Sent to the owner to draw an item: mp1 = control id, mp2 = `POWNERITEM`.
pub const WM_DRAWITEM:    u32 = 0x0036;
/// Sent to the owner for an item's size: mp1 = control id, mp2 = item
/// index (list box) or `POWNERITEM` (menu); returns the height.
pub const WM_MEASUREITEM: u32 = 0x0037;
/// `WM_CONTROL` code of a `BS_USERBUTTON` that needs painting; mp2 =
/// `PUSERBUTTON`.
pub const BN_PAINT:     u32 = 3;
/// `USERBUTTON.fsState` bits.
pub const BDS_HILITED:  u32 = 0x0100;
pub const BDS_DISABLED: u32 = 0x0200;
pub const BDS_DEFAULT:  u32 = 0x0400;
/// Size of `OWNERITEM` (hwnd, hps, fsState, fsAttribute, fsStateOld,
/// fsAttributeOld, rclItem, idItem, hItem) and `USERBUTTON` (hwnd, hps,
/// fsState, fsStateOld) in bytes.
pub const OWNERITEM_SIZE:  u32 = 44;
pub const USERBUTTON_SIZE: u32 = 16;

// Combination boxes (WC_COMBOBOX, <pmwin.h>).
pub const CBS_SIMPLE:       u32 = 1;
pub const CBS_DROPDOWN:     u32 = 2;
pub const CBS_DROPDOWNLIST: u32 = 4;
/// mp1 = TRUE to show the list; returns TRUE when its state changed.
pub const CBM_SHOWLIST:      u32 = 0x0170;
/// mp1 = TRUE to highlight the entry field; returns TRUE when changed.
pub const CBM_HILITE:        u32 = 0x0171;
pub const CBM_ISLISTSHOWING: u32 = 0x0172;
/// Ids of a combination box's list box and entry field.
pub const CBID_LIST: u32 = 0x029A;
pub const CBID_EDIT: u32 = 0x029B;
/// Combination box notifications (high word of `WM_CONTROL` mp1; mp2 =
/// the combination box).
pub const CBN_EFCHANGE: u32 = 1;
pub const CBN_EFSCROLL: u32 = 2;
pub const CBN_MEMERROR: u32 = 3;
pub const CBN_LBSELECT: u32 = 4;
pub const CBN_LBSCROLL: u32 = 5;
pub const CBN_SHOWLIST: u32 = 6;
pub const CBN_ENTER:    u32 = 7;

// Spin buttons (WC_SPINBUTTON, <pmstddlg.h>).
pub const SPBS_NUMERICONLY:  u32 = 0x0000_0001;
pub const SPBS_READONLY:     u32 = 0x0000_0002;
pub const SPBS_JUSTRIGHT:    u32 = 0x0000_0004;
pub const SPBS_JUSTLEFT:     u32 = 0x0000_0008;
pub const SPBS_JUSTCENTER:   u32 = 0x0000_000C;
pub const SPBS_MASTER:       u32 = 0x0000_0010;
pub const SPBS_NOBORDER:     u32 = 0x0000_0020;
pub const SPBS_PADWITHZEROS: u32 = 0x0000_0080;
pub const SPBS_FASTSPIN:     u32 = 0x0000_0100;
pub const SPBN_UPARROW:   u32 = 0x20A;
pub const SPBN_DOWNARROW: u32 = 0x20B;
pub const SPBN_ENDSPIN:   u32 = 0x20C;
pub const SPBN_CHANGE:    u32 = 0x20D;
pub const SPBN_SETFOCUS:  u32 = 0x20E;
pub const SPBN_KILLFOCUS: u32 = 0x20F;
pub const SPBM_OVERRIDESETLIMITS: u32 = 0x200;
pub const SPBM_QUERYLIMITS:       u32 = 0x201;
pub const SPBM_SETTEXTLIMIT:      u32 = 0x202;
pub const SPBM_SPINUP:            u32 = 0x203;
pub const SPBM_SPINDOWN:          u32 = 0x204;
pub const SPBM_QUERYVALUE:        u32 = 0x205;
pub const SPBM_SETARRAY:          u32 = 0x206;
pub const SPBM_SETLIMITS:         u32 = 0x207;
pub const SPBM_SETCURRENTVALUE:   u32 = 0x208;
pub const SPBM_SETMASTER:         u32 = 0x209;
/// `SPBM_QUERYVALUE` update modes (high word of mp2).
pub const SPBQ_UPDATEIFVALID: u32 = 0;
pub const SPBQ_ALWAYSUPDATE:  u32 = 1;
pub const SPBQ_DONOTUPDATE:   u32 = 3;

// Sliders (WC_SLIDER, <pmstddlg.h> INCL_WINSTDSLIDER).
pub const SLS_HORIZONTAL:      u32 = 0x0000_0000;
pub const SLS_VERTICAL:        u32 = 0x0000_0001;
pub const SLS_CENTER:          u32 = 0x0000_0000;
pub const SLS_BOTTOM:          u32 = 0x0000_0002;
pub const SLS_TOP:             u32 = 0x0000_0004;
pub const SLS_LEFT:            u32 = 0x0000_0002;
pub const SLS_RIGHT:           u32 = 0x0000_0004;
pub const SLS_SNAPTOINCREMENT: u32 = 0x0000_0008;
pub const SLS_BUTTONSBOTTOM:   u32 = 0x0000_0010;
pub const SLS_BUTTONSTOP:      u32 = 0x0000_0020;
pub const SLS_BUTTONSLEFT:     u32 = 0x0000_0010;
pub const SLS_BUTTONSRIGHT:    u32 = 0x0000_0020;
pub const SLS_OWNERDRAW:       u32 = 0x0000_0040;
pub const SLS_READONLY:        u32 = 0x0000_0080;
pub const SLS_RIBBONSTRIP:     u32 = 0x0000_0100;
pub const SLS_HOMEBOTTOM:      u32 = 0x0000_0000;
pub const SLS_HOMETOP:         u32 = 0x0000_0200;
pub const SLS_HOMELEFT:        u32 = 0x0000_0000;
pub const SLS_HOMERIGHT:       u32 = 0x0000_0200;
pub const SLS_PRIMARYSCALE1:   u32 = 0x0000_0000;
pub const SLS_PRIMARYSCALE2:   u32 = 0x0000_0400;
/// `SLM_*SLIDERINFO` attributes (mp1 = MPFROM2SHORT(type, arm position type)).
pub const SMA_SCALE1:              u32 = 0x0001;
pub const SMA_SCALE2:              u32 = 0x0002;
pub const SMA_SHAFTDIMENSIONS:     u32 = 0x0000;
pub const SMA_SHAFTPOSITION:       u32 = 0x0001;
pub const SMA_SLIDERARMDIMENSIONS: u32 = 0x0002;
pub const SMA_SLIDERARMPOSITION:   u32 = 0x0003;
pub const SMA_RANGEVALUE:          u32 = 0x0000;
pub const SMA_INCREMENTVALUE:      u32 = 0x0001;
pub const SMA_SETALLTICKS:         u32 = 0xFFFF;
/// Owner-drawn slider parts (`OWNERITEM.idItem` of `WM_DRAWITEM`).
pub const SDA_RIBBONSTRIP: u32 = 0x0001;
pub const SDA_SLIDERSHAFT: u32 = 0x0002;
pub const SDA_BACKGROUND:  u32 = 0x0003;
pub const SDA_SLIDERARM:   u32 = 0x0004;
pub const SLDERR_INVALID_PARAMETERS: i32 = -1;
pub const SLM_ADDDETENT:       u32 = 0x0369;
pub const SLM_QUERYDETENTPOS:  u32 = 0x036a;
pub const SLM_QUERYSCALETEXT:  u32 = 0x036b;
pub const SLM_QUERYSLIDERINFO: u32 = 0x036c;
pub const SLM_QUERYTICKPOS:    u32 = 0x036d;
pub const SLM_QUERYTICKSIZE:   u32 = 0x036e;
pub const SLM_REMOVEDETENT:    u32 = 0x036f;
pub const SLM_SETSCALETEXT:    u32 = 0x0370;
pub const SLM_SETSLIDERINFO:   u32 = 0x0371;
pub const SLM_SETTICKSIZE:     u32 = 0x0372;
pub const SLN_CHANGE:      u32 = 1;
pub const SLN_SLIDERTRACK: u32 = 2;
pub const SLN_SETFOCUS:    u32 = 3;
pub const SLN_KILLFOCUS:   u32 = 4;
/// `SLDCDATA` (pCtlData of a WC_SLIDER): `cbSize` (ULONG), then
/// `usScale1Increments`, `usScale1Spacing`, `usScale2Increments`,
/// `usScale2Spacing` (USHORTs) at these byte offsets.
pub const SLDCDATA_SCALE1_INCREMENTS: usize = 4;
pub const SLDCDATA_SCALE1_SPACING:    usize = 6;
pub const SLDCDATA_SCALE2_INCREMENTS: usize = 8;
pub const SLDCDATA_SCALE2_SPACING:    usize = 10;
/// Size of `SLDCDATA`.
pub const SLDCDATA_SIZE: usize = 12;

// Value sets (WC_VALUESET, <pmstddlg.h> INCL_WINSTDVALSET).
pub const VS_BITMAP:       u32 = 0x0001;
pub const VS_ICON:         u32 = 0x0002;
pub const VS_TEXT:         u32 = 0x0004;
pub const VS_RGB:          u32 = 0x0008;
pub const VS_COLORINDEX:   u32 = 0x0010;
pub const VS_BORDER:       u32 = 0x0020;
pub const VS_ITEMBORDER:   u32 = 0x0040;
pub const VS_SCALEBITMAPS: u32 = 0x0080;
pub const VS_RIGHTTOLEFT:  u32 = 0x0100;
pub const VS_OWNERDRAW:    u32 = 0x0200;
pub const VIA_BITMAP:     u16 = 0x0001;
pub const VIA_ICON:       u16 = 0x0002;
pub const VIA_TEXT:       u16 = 0x0004;
pub const VIA_RGB:        u16 = 0x0008;
pub const VIA_COLORINDEX: u16 = 0x0010;
pub const VIA_OWNERDRAW:  u16 = 0x0020;
pub const VIA_DISABLED:   u16 = 0x0040;
pub const VIA_DRAGGABLE:  u16 = 0x0080;
pub const VIA_DROPONABLE: u16 = 0x0100;
pub const VMA_ITEMSIZE:    u32 = 0x0001;
pub const VMA_ITEMSPACING: u32 = 0x0002;
/// Owner-drawn value set parts (`OWNERITEM.idItem` of `WM_DRAWITEM`).
pub const VDA_ITEM:           u32 = 0x0001;
pub const VDA_ITEMBACKGROUND: u32 = 0x0002;
pub const VDA_SURROUNDING:    u32 = 0x0003;
pub const VDA_BACKGROUND:     u32 = 0x0004;
pub const VSERR_INVALID_PARAMETERS: i32 = -1;
pub const VM_QUERYITEM:         u32 = 0x0375;
pub const VM_QUERYITEMATTR:     u32 = 0x0376;
pub const VM_QUERYMETRICS:      u32 = 0x0377;
pub const VM_QUERYSELECTEDITEM: u32 = 0x0378;
pub const VM_SELECTITEM:        u32 = 0x0379;
pub const VM_SETITEM:           u32 = 0x037a;
pub const VM_SETITEMATTR:       u32 = 0x037b;
pub const VM_SETMETRICS:        u32 = 0x037c;
pub const VN_SELECT:    u32 = 120;
pub const VN_ENTER:     u32 = 121;
pub const VN_SETFOCUS:  u32 = 127;
pub const VN_KILLFOCUS: u32 = 128;
pub const VN_HELP:      u32 = 129;
/// `VSCDATA` (pCtlData of a WC_VALUESET): `cbSize` (ULONG), then
/// `usRowCount`, `usColumnCount` (USHORTs) at these byte offsets.
pub const VSCDATA_ROW_COUNT:    usize = 4;
pub const VSCDATA_COLUMN_COUNT: usize = 6;
/// Size of `VSCDATA`.
pub const VSCDATA_SIZE: usize = 8;
/// `VSTEXT` (mp2 of `VM_QUERYITEM` for a text item): `pszItemText`, then
/// `ulBufLen` at this byte offset.
pub const VSTEXT_BUF_LEN: u32 = 4;

// WC_MLE messages (MLM_*) and MLM_QUERYSEL modes (MLFQS_*, <pmwin.h>)
pub const MLM_SETSEL:   u32 = 0x01ca;
pub const MLM_QUERYSEL: u32 = 0x01cb;
/// mp1 = TRUE to wrap lines at word boundaries (`MLS_WORDWRAP`).
pub const MLM_SETWRAP:   u32 = 0x01b4;
/// Returns TRUE when word wrap is on.
pub const MLM_QUERYWRAP: u32 = 0x01b5;
pub const MLM_CUT:      u32 = 0x01d8;
pub const MLM_COPY:     u32 = 0x01d9;
pub const MLM_PASTE:    u32 = 0x01da;
pub const MLM_CLEAR:    u32 = 0x01db;
/// mp1 = TRUE for read-only; returns the previous state.
pub const MLM_SETREADONLY:   u32 = 0x01b8;
/// Returns TRUE when the MLE is read-only.
pub const MLM_QUERYREADONLY: u32 = 0x01b9;
pub const MLFQS_MINMAXSEL: u32 = 0;
pub const MLFQS_MINSEL:    u32 = 1;
pub const MLFQS_MAXSEL:    u32 = 2;
pub const MLFQS_ANCHORSEL: u32 = 3;
pub const MLFQS_CURSORSEL: u32 = 4;

// MLE notification codes (high word of WM_CONTROL mp1).
pub const MLN_CHANGE:    u32 = 0x0007;
pub const MLN_SETFOCUS:  u32 = 0x0008;
pub const MLN_KILLFOCUS: u32 = 0x0009;

// WM_CHAR packing (CHRMSG in <pmwin.h>):
//   mp1 = fs (KC_* flags, low word) | cRepeat << 16 | scancode << 24
//   mp2 = chr (low word) | vkey (VK_*, high word)
pub const WM_CHAR_REPEAT_SHIFT:   u32 = 16;
pub const WM_CHAR_SCANCODE_SHIFT: u32 = 24;
pub const WM_CHAR_VKEY_SHIFT:     u32 = 16;

// KC_* flags for WM_CHAR message (MP1 low word)
pub const KC_CHAR:       u32 = 0x0001; // character code in MP2 low word is valid
pub const KC_VIRTUALKEY: u32 = 0x0002; // virtual key code in MP2 high word is valid
pub const KC_SCANCODE:   u32 = 0x0004; // hardware scan code in MP1 bits 24-31 is valid
pub const KC_SHIFT:      u32 = 0x0008; // shift key held
pub const KC_CTRL:       u32 = 0x0010; // Ctrl key held
pub const KC_ALT:        u32 = 0x0020; // Alt key held
pub const KC_KEYUP:      u32 = 0x0040; // key-release event
pub const KC_PREVDOWN:   u32 = 0x0080; // key was already down (auto-repeat)
pub const KC_LONEKEY:    u32 = 0x0100; // key pressed without any other key

// VK_* virtual key codes for WM_CHAR MP2 high word
pub const VK_BACKSPACE:  u32 = 0x05;
pub const VK_TAB:        u32 = 0x06;
pub const VK_BACKTAB:    u32 = 0x07; // Shift+Tab
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
pub const SWP_ACTIVATE: u32 = 0x0080;
pub const SWP_DEACTIVATE: u32 = 0x0100;
pub const SWP_MINIMIZE: u32 = 0x0400;
pub const SWP_MAXIMIZE: u32 = 0x0800;
pub const SWP_RESTORE: u32 = 0x1000;

// Pseudo window handles (<pmwin.h>): HWND_TOP / HWND_BOTTOM are the Z-order
// sentinels used with SWP_ZORDER in WinSetWindowPos.
pub const HWND_TOP: u32          = 3; // Place window at the top of Z-order
pub const HWND_BOTTOM: u32       = 4; // Place window at the bottom of Z-order
pub const HWND_DESKTOP: u32      = 1; // PM desktop pseudo-handle (origin of coordinate space)
pub const HWND_OBJECT: u32       = 2; // Parent of object windows (never shown)
pub const HWND_THREADCAPTURE: u32 = 5; // WinSetCapture: capture to the calling thread

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
pub const FCF_HIDEBUTTON:    u32 = 0x0100_0000; // hide button (in place of the minimize button)
pub const FCF_HIDEMAX:       u32 = 0x0100_0020; // hide + maximize buttons
pub const FCF_STANDARD:      u32 = 0x0000_CC3F; // FCF_TITLEBAR|SYSMENU|MENU|SIZEBORDER|MINMAX|SHELLPOSITION|TASKLIST|ICON|ACCELTABLE

// ── GPI return values and colour sentinels (<pmgpi.h>) ───────────────────────
pub const GPI_ERROR: u32 = 0;
pub const GPI_OK:    u32 = 1;
/// `GpiQueryPel` result for a point outside the drawing surface.
pub const CLR_NOINDEX: i32 = -254;
/// `GpiQueryColor` / `GpiQueryBackColor` error result.
pub const CLR_ERROR: i32 = -255;
/// Default colour-table entry that is pure black (no 0–15 index is).
pub const CLR_BLACK: i32 = -1;

pub const GPI_ALTERROR: i32 = -1;
/// Default colour value (`GpiSetColor`), and the default background index.
pub const CLR_DEFAULT: i32 = -3;
pub const CLR_BACKGROUND: u32 = 0;
/// `GpiSetBitmap` / `GpiLoadBitmap` error handle.
pub const HBM_ERROR: u32 = 0xFFFF_FFFF;
/// `GpiSetBitmapBits` / `GpiQueryBitmapBits` error result.
pub const BMB_ERROR: i32 = -1;
/// `GpiCreateBitmap` flOptions: initialise the bitmap from the given data.
pub const CBM_INIT: u32 = 4;
/// `GpiCreatePS` flOptions: associate the presentation space with the DC.
pub const GPIA_ASSOC: u32 = 0x4000;
/// Bitmap compression (`ulCompression` of BITMAPINFOHEADER2). The RLE
/// formats are decoded; `BCA_HUFFMAN1D` is not supported.
pub const BCA_UNCOMP: u32 = 0;
pub const BCA_RLE8:   u32 = 1;
pub const BCA_RLE4:   u32 = 2;
pub const BCA_RLE24:  u32 = 4;
/// Bitmap file types (`usType` of BITMAPFILEHEADER / BITMAPARRAYFILEHEADER).
pub const BFT_BMAP: u16 = 0x4d42;        // "BM"
pub const BFT_BITMAPARRAY: u16 = 0x4142; // "BA"

// ── Raster operations (GpiBitBlt lRop), <pmgpi.h> ────────────────────────────
// Ternary ROPs: bit (P<<2 | S<<1 | D) of the code is the result for that
// combination of pattern P, source S and destination D.
pub const ROP_SRCCOPY:    u32 = 0x00CC;
pub const ROP_SRCPAINT:   u32 = 0x00EE;
pub const ROP_SRCAND:     u32 = 0x0088;
pub const ROP_SRCINVERT:  u32 = 0x0066;
pub const ROP_NOTSRCCOPY: u32 = 0x0033;
pub const ROP_PATCOPY:    u32 = 0x00F0;
pub const ROP_PATINVERT:  u32 = 0x005A;
pub const ROP_DSTINVERT:  u32 = 0x0055;
pub const ROP_ZERO:       u32 = 0x0000;
pub const ROP_ONE:        u32 = 0x00FF;

// ── LCOL / LCOLF / QLCT (GpiCreateLogColorTable), <pmgpi.h> ──────────────────
pub const LCOL_RESET:      u32 = 0x0001;
pub const LCOLF_DEFAULT:   u32 = 0;
pub const LCOLF_INDRGB:    u32 = 1;
pub const LCOLF_CONSECRGB: u32 = 2;
pub const LCOLF_RGB:       u32 = 3;
pub const LCOLOPT_INDEX:   u32 = 2;
pub const QLCT_ERROR:      i32 = -1;
pub const QLCT_RGB:        i32 = -2;
pub const QLCT_NOTLOADED:  i32 = -1;
/// Upper bound on colour-table entries read or written per call.
pub const MAX_LOG_COLOR_ENTRIES: u32 = 4096;

// ── LINETYPE_* (GpiSetLineType), <pmgpi.h> ───────────────────────────────────
pub const LINETYPE_DEFAULT:       u32 = 0;
pub const LINETYPE_DOT:           u32 = 1;
pub const LINETYPE_SHORTDASH:     u32 = 2;
pub const LINETYPE_DASHDOT:       u32 = 3;
pub const LINETYPE_DOUBLEDOT:     u32 = 4;
pub const LINETYPE_LONGDASH:      u32 = 5;
pub const LINETYPE_DASHDOUBLEDOT: u32 = 6;
pub const LINETYPE_SOLID:         u32 = 7;
pub const LINETYPE_INVISIBLE:     u32 = 8;
pub const LINETYPE_ALTERNATE:     u32 = 9;
pub const LINETYPE_ERROR:         i32 = -1;

// ── PATSYM_* (GpiSetPattern), <pmgpi.h> ──────────────────────────────────────
pub const PATSYM_ERROR:     i32 = -1;
pub const PATSYM_DEFAULT:   u32 = 0;
pub const PATSYM_DENSE1:    u32 = 1;
pub const PATSYM_DENSE8:    u32 = 8;
pub const PATSYM_VERT:      u32 = 9;
pub const PATSYM_HORIZ:     u32 = 10;
pub const PATSYM_DIAG1:     u32 = 11;
pub const PATSYM_DIAG2:     u32 = 12;
pub const PATSYM_DIAG3:     u32 = 13;
pub const PATSYM_DIAG4:     u32 = 14;
pub const PATSYM_NOSHADE:   u32 = 15;
pub const PATSYM_SOLID:     u32 = 16;
pub const PATSYM_HALFTONE:  u32 = 17;
pub const PATSYM_HATCH:     u32 = 18;
pub const PATSYM_DIAGHATCH: u32 = 19;
pub const PATSYM_BLANK:     u32 = 64;

// ── Line widths (GpiSetLineWidth / GpiSetLineWidthGeom), <pmgpi.h> ───────────
/// Cosmetic widths are FIXED multiples of the normal width (one pel here).
pub const LINEWIDTH_DEFAULT:   u32 = 0;
pub const LINEWIDTH_NORMAL:    u32 = 0x0001_0000;
pub const LINEWIDTH_THICK:     u32 = 0x0002_0000;
pub const LINEWIDTH_ERROR:     i32 = -1;
pub const LINEWIDTHGEOM_ERROR: i32 = -1;

// ── GpiBox / GpiFullArc lControl, <pmgpi.h> ──────────────────────────────────
pub const DRO_FILL:        u32 = 1;
pub const DRO_OUTLINE:     u32 = 2;
pub const DRO_OUTLINEFILL: u32 = 3;

// ── Presentation-space options, state and transforms, <pmgpi.h> ──────────────
// GpiCreatePS / GpiSetPS / GpiQueryPS flOptions: page units (PS_UNITS),
// coordinate format (PS_FORMAT), type (PS_TYPE), association (PS_ASSOCIATE).
pub const PU_ARBITRARY: u32 = 0x0004;
pub const PU_PELS:      u32 = 0x0008;
pub const PU_LOMETRIC:  u32 = 0x000C;
pub const PU_HIMETRIC:  u32 = 0x0010;
pub const PU_LOENGLISH: u32 = 0x0014;
pub const PU_HIENGLISH: u32 = 0x0018;
pub const PU_TWIPS:     u32 = 0x001C;
pub const GPIF_DEFAULT: u32 = 0;
pub const GPIF_SHORT:   u32 = 0x0100;
pub const GPIF_LONG:    u32 = 0x0200;
pub const GPIT_NORMAL:  u32 = 0;
pub const GPIT_MICRO:   u32 = 0x1000;
pub const GPIA_NOASSOC: u32 = 0;
pub const PS_UNITS:     u32 = 0x00FC;
pub const PS_FORMAT:    u32 = 0x0F00;
pub const PS_TYPE:      u32 = 0x1000;
pub const PS_MODE:      u32 = 0x2000;
pub const PS_ASSOCIATE: u32 = 0x4000;
pub const PS_NORESET:   u32 = 0x8000;
// GpiResetPS flOptions.
pub const GRES_ATTRS:    u32 = 0x0001;
pub const GRES_SEGMENTS: u32 = 0x0002;
pub const GRES_ALL:      u32 = 0x0004;
// GpiSetAttrs / GpiQueryAttrs primitive types and attribute masks.
pub const PRIM_LINE:   u32 = 1;
pub const PRIM_CHAR:   u32 = 2;
pub const PRIM_MARKER: u32 = 3;
pub const PRIM_AREA:   u32 = 4;
pub const PRIM_IMAGE:  u32 = 5;
pub const LBB_COLOR:         u32 = 0x0001;
pub const LBB_BACK_COLOR:    u32 = 0x0002;
pub const LBB_MIX_MODE:      u32 = 0x0004;
pub const LBB_BACK_MIX_MODE: u32 = 0x0008;
pub const LBB_WIDTH:         u32 = 0x0010;
pub const LBB_GEOM_WIDTH:    u32 = 0x0020;
pub const LBB_TYPE:          u32 = 0x0040;
pub const LBB_END:           u32 = 0x0080;
pub const LBB_JOIN:          u32 = 0x0100;
pub const CBB_COLOR:         u32 = 0x0001;
pub const CBB_BACK_COLOR:    u32 = 0x0002;
pub const CBB_MIX_MODE:      u32 = 0x0004;
pub const CBB_BACK_MIX_MODE: u32 = 0x0008;
pub const CBB_SET:           u32 = 0x0010;
pub const CBB_MODE:          u32 = 0x0020;
pub const CBB_BOX:           u32 = 0x0040;
pub const CBB_ANGLE:         u32 = 0x0080;
pub const CBB_SHEAR:         u32 = 0x0100;
pub const CBB_DIRECTION:     u32 = 0x0200;
pub const CBB_TEXT_ALIGN:    u32 = 0x0400;
pub const CBB_EXTRA:         u32 = 0x0800;
pub const CBB_BREAK_EXTRA:   u32 = 0x1000;
pub const MBB_COLOR:         u32 = 0x0001;
pub const MBB_BACK_COLOR:    u32 = 0x0002;
pub const MBB_MIX_MODE:      u32 = 0x0004;
pub const MBB_BACK_MIX_MODE: u32 = 0x0008;
pub const MBB_SET:           u32 = 0x0010;
pub const MBB_SYMBOL:        u32 = 0x0020;
pub const MBB_BOX:           u32 = 0x0040;
pub const ABB_COLOR:         u32 = 0x0001;
pub const ABB_BACK_COLOR:    u32 = 0x0002;
pub const ABB_MIX_MODE:      u32 = 0x0004;
pub const ABB_BACK_MIX_MODE: u32 = 0x0008;
pub const ABB_SET:           u32 = 0x0010;
pub const ABB_SYMBOL:        u32 = 0x0020;
pub const ABB_REF_POINT:     u32 = 0x0040;
pub const IBB_COLOR:         u32 = 0x0001;
pub const IBB_BACK_COLOR:    u32 = 0x0002;
pub const IBB_MIX_MODE:      u32 = 0x0004;
pub const IBB_BACK_MIX_MODE: u32 = 0x0008;
// GpiConvert coordinate spaces and transform-matrix options.
pub const CVTC_WORLD:       u32 = 1;
pub const CVTC_MODEL:       u32 = 2;
pub const CVTC_DEFAULTPAGE: u32 = 3;
pub const CVTC_PAGE:        u32 = 4;
pub const CVTC_DEVICE:      u32 = 5;
pub const TRANSFORM_REPLACE: u32 = 0;
pub const TRANSFORM_ADD:     u32 = 1;
pub const TRANSFORM_PREEMPT: u32 = 2;
// Errors of these calls.
pub const PMERR_INV_COORD_SPACE:         u32 = 0x205A;
pub const PMERR_INV_GRAPHICS_FIELD:      u32 = 0x207A;
pub const PMERR_INV_ID:                  u32 = 0x2081;
pub const PMERR_INV_MATRIX_ELEMENT:      u32 = 0x209B;
pub const PMERR_INV_OR_INCOMPAT_OPTIONS: u32 = 0x20A9;
pub const PMERR_INV_PAGE_VIEWPORT:       u32 = 0x20AD;
pub const PMERR_INV_PRIMITIVE_TYPE:      u32 = 0x20B9;
pub const PMERR_INV_PS_SIZE:             u32 = 0x20BA;
pub const PMERR_INV_RESET_OPTIONS:       u32 = 0x20C2;
pub const PMERR_INV_TRANSFORM_TYPE:      u32 = 0x20D0;
pub const PMERR_INV_VIEWING_LIMITS:      u32 = 0x20D2;
pub const PMERR_UNSUPPORTED_ATTR:        u32 = 0x2109;
pub const PMERR_INV_BACKGROUND_MIX_ATTR: u32 = 0x2045;
pub const PMERR_INV_LINE_TYPE_ATTR:      u32 = 0x2095;
pub const PMERR_INV_LINE_WIDTH_ATTR:     u32 = 0x2096;
pub const PMERR_INV_MIX_ATTR:            u32 = 0x20A3;
pub const PMERR_INV_PATTERN_ATTR:        u32 = 0x20B0;

// ── Device contexts (<pmdev.h>) ──────────────────────────────────────────────
pub const OD_INFO:   u32 = 6;
pub const OD_MEMORY: u32 = 8;
pub const DEV_ERROR: u32 = 0;
pub const DEV_OK:    u32 = 1;
pub const CAPS_FAMILY:                u32 = 0;
pub const CAPS_WIDTH:                 u32 = 4;
pub const CAPS_HEIGHT:                u32 = 5;
pub const CAPS_WIDTH_IN_CHARS:        u32 = 6;
pub const CAPS_HEIGHT_IN_CHARS:       u32 = 7;
pub const CAPS_HORIZONTAL_RESOLUTION: u32 = 8;
pub const CAPS_VERTICAL_RESOLUTION:   u32 = 9;
pub const CAPS_CHAR_WIDTH:            u32 = 10;
pub const CAPS_CHAR_HEIGHT:           u32 = 11;
pub const CAPS_COLORS:                u32 = 14;
pub const CAPS_COLOR_PLANES:          u32 = 15;
pub const CAPS_COLOR_BITCOUNT:        u32 = 16;
pub const CAPS_MOUSE_BUTTONS:         u32 = 18;
pub const CAPS_BITMAP_FORMATS:        u32 = 23;
pub const CAPS_PHYS_COLORS:           u32 = 33;
/// Number of CAPS_* entries `DevQueryCaps` reports (indices 0..).
pub const CAPS_COUNT: u32 = 42;

// ── DBM_* (WinDrawBitmap fl), <pmwin.h> ──────────────────────────────────────
pub const DBM_INVERT:  u32 = 0x0001;
pub const DBM_STRETCH: u32 = 0x0004;
pub const DBM_XOR:     u32 = 0x0010;

// ── FM_* foreground mix modes (GpiSetMix), <pmgpi.h> ─────────────────────────
// Each combines the source (current colour) S with the destination pel D.
pub const FM_DEFAULT:     u32 = 0;  // = FM_OVERPAINT
pub const FM_OR:          u32 = 1;  // S | D
pub const FM_OVERPAINT:   u32 = 2;  // S
pub const FM_XOR:         u32 = 4;  // S ^ D
pub const FM_LEAVEALONE:  u32 = 5;  // D
pub const FM_AND:         u32 = 6;  // S & D
pub const FM_SUBTRACT:    u32 = 7;  // !S & D
pub const FM_MASKSRCNOT:  u32 = 8;  // S & !D
pub const FM_ZERO:        u32 = 9;  // 0
pub const FM_NOTMERGESRC: u32 = 10; // !(S | D)
pub const FM_NOTXORSRC:   u32 = 11; // !(S ^ D)
pub const FM_INVERT:      u32 = 12; // !D
pub const FM_MERGESRCNOT: u32 = 13; // S | !D
pub const FM_NOTCOPYSRC:  u32 = 14; // !S
pub const FM_MERGENOTSRC: u32 = 15; // !S | D
pub const FM_NOTMASKSRC:  u32 = 16; // !(S & D)
pub const FM_ONE:         u32 = 17; // all ones (white)

// ── BM_* background mix modes (GpiSetBackMix), <pmgpi.h> ─────────────────────
// BM_DEFAULT..BM_ONE share the FM_* values; two more are background-only.
pub const BM_LEAVEALONE:      u32 = 5;
pub const BM_ONE:             u32 = 17;
pub const BM_SRCTRANSPARENT:  u32 = 18;
pub const BM_DESTTRANSPARENT: u32 = 19;

/// Window positions are clamped to ± this and sizes to `0..=` this when a
/// guest sets them (`WinSetWindowPos`, `WinCreateWindow`), far beyond any
/// desktop, so geometry arithmetic (`x + cx`) cannot overflow.
pub const MAX_WINDOW_COORD: i32 = 1 << 20;

// Mock handle constants
pub const MOCK_HAB: u32 = 0x1234;

// OS/2 error codes
pub const NO_ERROR: u32 = 0;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_INVALID_DRIVE: u32 = 15;
pub const ERROR_INVALID_FUNCTION: u32 = 1;
pub const ERROR_BUFFER_OVERFLOW: u32 = 111;
pub const ERROR_INVALID_LEVEL: u32 = 124;
pub const ERROR_MOD_NOT_FOUND: u32 = 126;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;
pub const ERROR_ENVVAR_NOT_FOUND: u32 = 203;
pub const ERROR_INIT_ROUTINE_FAILED: u32 = 295;
pub const ERROR_INVALID_PARAMETER:    u32 = 87;
pub const ERROR_INVALID_CODE_PAGE:    u32 = 472;
pub const ERROR_NESTING_NOT_ALLOWED:  u32 = 215;
/// An NLS table did not fit the caller's buffer (`<bseerr.h>`).
pub const ERROR_NLS_TABLE_TRUNCATED:  u32 = 399;

// ── VIO error codes ───────────────────────────────────────────────────────────
pub const ERROR_VIO_ROW: u32 = 358; // Row parameter out of range
pub const ERROR_VIO_COL: u32 = 359; // Column parameter out of range
pub const ERROR_VIO_INVALID_PARMS: u32 = 421; // <bseerr.h>
/// `VioSetAnsi` / `VioGetAnsi` states (`<bsesub.h>`).
pub const ANSI_ON: u16 = 1;
pub const ANSI_OFF: u16 = 0;

// ── KBDCALLS wait flags (`<bsesub.h>`) ───────────────────────────────────────
pub const IO_WAIT: u32 = 0;
pub const IO_NOWAIT: u32 = 1;
/// `<bseerr.h>`: returned by a keyboard read that ends the process at the
/// end of piped input (the guest normally never sees it).
pub const ERROR_KBD_NO_DEVICE: u32 = 374;
/// Exit code of a process whose waiting `KbdCharIn` / `KbdStringIn` found
/// the end of a piped (non-terminal) standard input: Warpine's own choice,
/// the same as the built-in CMD.EXE at end of input.
pub const EXIT_CODE_KBD_END_OF_INPUT: i32 = 0;

// ── DosQueryHType handle types (`<bsedos.h>`) ────────────────────────────────
pub const HANDTYPE_FILE: u32 = 0x0000;
pub const HANDTYPE_DEVICE: u32 = 0x0001;
pub const HANDTYPE_PIPE: u32 = 0x0002;
/// Device attribute word of the console reported by `DosQueryHType`:
/// bit 0 = standard input device, bit 1 = standard output device.
pub const CONSOLE_DEVICE_ATTR: u32 = 0x0003;
/// The standard output handle.
pub const STDOUT_HANDLE: u32 = 1;
/// `DosDupHandle` new-handle value asking for a fresh handle.
pub const NEW_HANDLE_REQUEST: u32 = 0xFFFF_FFFF;

// ── Pipes, handle states and shared memory (`<bsedos.h>`, `<bsememf.h>`,
//    `<bseerr.h>`) ─────────────────────────────────────────────────────────────
pub const ERROR_BAD_FORMAT: u32 = 11;
pub const ERROR_INTERRUPT: u32 = 95;
pub const ERROR_BROKEN_PIPE: u32 = 109;
pub const ERROR_SEM_TIMEOUT: u32 = 121;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ERROR_BAD_PIPE: u32 = 230;
pub const ERROR_PIPE_BUSY: u32 = 231;
pub const ERROR_NO_DATA: u32 = 232;
pub const ERROR_PIPE_NOT_CONNECTED: u32 = 233;
pub const ERROR_MORE_DATA: u32 = 234;
pub const ERROR_INVALID_PROCID: u32 = 303;
pub const ERROR_INVALID_ADDRESS: u32 = 487;

/// `DosOpen` / `DosQueryFHState` access and flag bits.
pub const OPEN_ACCESS_READONLY: u32 = 0x0000;
pub const OPEN_ACCESS_WRITEONLY: u32 = 0x0001;
pub const OPEN_ACCESS_READWRITE: u32 = 0x0002;
pub const OPEN_FLAGS_NOINHERIT: u32 = 0x0080;
pub const OPEN_FLAGS_NO_CACHE: u32 = 0x1000;
pub const OPEN_FLAGS_FAIL_ON_ERROR: u32 = 0x2000;
pub const OPEN_FLAGS_WRITE_THROUGH: u32 = 0x4000;
/// `DosOpen` action: the file (pipe) existed.
pub const FILE_EXISTED: u32 = 1;

/// `DosExecPgm` execution flags.
pub const EXEC_SYNC: u32 = 0;
pub const EXEC_ASYNC: u32 = 1;
pub const EXEC_ASYNCRESULT: u32 = 2;
pub const EXEC_BACKGROUND: u32 = 4;

/// Named-pipe states (`DosPeekNPipe`).
pub const NP_STATE_DISCONNECTED: u32 = 1;
pub const NP_STATE_LISTENING: u32 = 2;
pub const NP_STATE_CONNECTED: u32 = 3;
pub const NP_STATE_CLOSING: u32 = 4;
/// `DosCreateNPipe` open mode.
pub const NP_ACCESS_INBOUND: u32 = 0x0000;
pub const NP_ACCESS_OUTBOUND: u32 = 0x0001;
pub const NP_ACCESS_DUPLEX: u32 = 0x0002;
pub const NP_NOINHERIT: u32 = 0x0080;
pub const NP_NOWRITEBEHIND: u32 = 0x4000;
/// `DosCreateNPipe` pipe mode / `DosQueryNPHState` / `DosSetNPHState` state.
pub const NP_READMODE_MESSAGE: u32 = 0x0100;
pub const NP_TYPE_MESSAGE: u32 = 0x0400;
pub const NP_END_SERVER: u32 = 0x4000;
pub const NP_NOWAIT: u32 = 0x8000;
pub const NP_UNLIMITED_INSTANCES: u32 = 0x00FF;
pub const NP_ICOUNT: u32 = 0x00FF;
/// `DosWaitNPipe` timeouts: the pipe's default, forever.
pub const NP_DEFAULT_WAIT: u32 = 0;
pub const NP_INDEFINITE_WAIT: u32 = 0xFFFF_FFFF;
/// `PIPESEMSTATE.fStatus` (`DosQueryNPipeSemState`, `<bsedos.h>`): end of
/// the records, data to read, space to write, the pipe closed.
pub const NPSS_EOI: u8 = 0;
pub const NPSS_RDATA: u8 = 1;
pub const NPSS_WSPACE: u8 = 2;
pub const NPSS_CLOSE: u8 = 3;
/// `PIPESEMSTATE.fFlag`: a thread waits at the other end of the pipe.
pub const NPSS_WAIT: u8 = 0x01;
/// `sizeof(PIPESEMSTATE)`: two BYTEs and two USHORTs.
pub const PIPESEMSTATE_SIZE: u32 = 6;

/// `DosAllocSharedMem` / `DosGiveSharedMem` flags.
pub const PAG_READ: u32 = 0x0001;
pub const PAG_WRITE: u32 = 0x0002;
pub const PAG_EXECUTE: u32 = 0x0004;
pub const PAG_GUARD: u32 = 0x0008;
pub const PAG_COMMIT: u32 = 0x0010;
pub const OBJ_TILE: u32 = 0x0040;
pub const OBJ_GETTABLE: u32 = 0x0100;
pub const OBJ_GIVEABLE: u32 = 0x0200;
pub const OBJ_ANY: u32 = 0x0400;

// ── MOUCALLS error codes (<bseerr.h>) ────────────────────────────────────────
pub const ERROR_MOUSE_NO_DEVICE: u32 = 385;
pub const ERROR_MOUSE_INV_HANDLE: u32 = 386;
pub const ERROR_MOUSE_INV_PARMS: u32 = 387;
pub const ERROR_MOUSE_INV_MASK: u32 = 392;

// ── MOUCALLS flags (<bsesub.h>, INCL_MOU) ────────────────────────────────────
/// `MouReadEventQue` read types.
pub const MOU_NOWAIT: u16 = 0;
pub const MOU_WAIT: u16 = 1;
/// `MOUEVENTINFO::fs` state bits; the same bits form the event mask.
pub const MOUSE_MOTION: u16 = 0x0001;
pub const MOUSE_MOTION_WITH_BN1_DOWN: u16 = 0x0002;
pub const MOUSE_BN1_DOWN: u16 = 0x0004;
pub const MOUSE_MOTION_WITH_BN2_DOWN: u16 = 0x0008;
pub const MOUSE_BN2_DOWN: u16 = 0x0010;
pub const MOUSE_MOTION_WITH_BN3_DOWN: u16 = 0x0020;
pub const MOUSE_BN3_DOWN: u16 = 0x0040;
/// `MouGetDevStatus` / `MouSetDevStatus` flags.
pub const MOUSE_QUEUEBUSY: u16 = 0x0001;
pub const MOUSE_BLOCKREAD: u16 = 0x0002;
pub const MOUSE_FLUSH: u16 = 0x0004;
pub const MOUSE_UNSUPPORTED_MODE: u16 = 0x0008;
pub const MOUSE_DISABLED: u16 = 0x0100;
pub const MOUSE_MICKEYS: u16 = 0x0200;

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
pub const WC_SPINBUTTON_ATOM: u32 = 0xFFFF_0020;
pub const WC_CONTAINER_ATOM:  u32 = 0xFFFF_0025;
pub const WC_NOTEBOOK_ATOM:   u32 = 0xFFFF_0028;
pub const WC_SLIDER_ATOM:     u32 = 0xFFFF_0026;
pub const WC_VALUESET_ATOM:   u32 = 0xFFFF_0027;

/// Height of the PM menu bar in pixels (OS/2-space units).
pub const MENU_BAR_HEIGHT: i16 = 20;
/// Left inset of the first top-level menu item inside the menu bar (pixels).
pub const MENU_BAR_ITEM_INSET: i32 = 4;
/// Horizontal padding added to a top-level item's label width (pixels).
pub const MENU_BAR_ITEM_PAD: i32 = 8;
/// Width of a pull-down menu (pixels).
pub const MENU_DROPDOWN_W: i32 = 120;
/// Height of one pull-down menu row (pixels).
pub const MENU_DROPDOWN_ITEM_H: i32 = 16;

// Dialog and menu message constants
pub const WM_INITDLG:    u32 = 0x003B; // First message to a dialog box
pub const WM_INITMENU:   u32 = 0x0033; // Sent before a menu is displayed
pub const WM_MENUSELECT: u32 = 0x0034; // Menu item selected (non-activating)
pub const WM_MENUEND:    u32 = 0x0035; // Menu dismissed

// Dialog result IDs returned by WinDlgBox / WinProcessDlg
pub const DID_OK:     u32 = 1;         // OK button
pub const DID_CANCEL: u32 = 2;         // Cancel button
pub const DID_ERROR:  u32 = 0xFFFF;    // Error return
/// Largest DLGTEMPLATE read from a resource (`cbTemplate` is a USHORT).
pub const MAX_DLG_TEMPLATE: usize = 0x1_0000;

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

// WM_SYSCOMMAND (0x0021) — system command sent by frame chrome to the frame.
// mp1 = SC_* command code, mp2 = unused.
pub const WM_SYSCOMMAND: u32 = 0x0021;

// SC_* system command codes (sent in mp1 of WM_SYSCOMMAND).
pub const SC_SIZE:     u32 = 0x8000; // size the window with the keyboard
pub const SC_MOVE:     u32 = 0x8001; // move the window with the keyboard
pub const SC_CLOSE:    u32 = 0x8004; // close the window
pub const SC_MINIMIZE: u32 = 0x8002; // minimize the window
pub const SC_MAXIMIZE: u32 = 0x8003; // maximize the window
pub const SC_RESTORE:  u32 = 0x8008; // restore from min/max
pub const SC_HIDE:     u32 = 0x802A; // hide the frame window
pub const SC_SYSMENU:  u32 = 0x8007; // open the system menu (MM_STARTMENUMODE to FID_SYSMENU)
pub const SC_TASKMANAGER: u32 = 0x8011; // activate the window list (task list)
/// Identity of a frame's system menu (`<pmwin.h>`).
pub const FID_SYSMENU: u32 = 0x8002;

// WinTrackRect / WM_TRACKFRAME tracking flags (`TRACKINFO.fs`, `<pmwin.h>`):
// which edges of the rectangle move.
pub const TF_LEFT:   u32 = 0x0001;
pub const TF_TOP:    u32 = 0x0002;
pub const TF_RIGHT:  u32 = 0x0004;
pub const TF_BOTTOM: u32 = 0x0008;
pub const TF_MOVE:   u32 = 0x000F;

// WinPopupMenu options (`<pmwin.h>`).
pub const PU_POSITIONONITEM: u32 = 0x0001;
pub const PU_HCONSTRAIN:     u32 = 0x0002;
pub const PU_VCONSTRAIN:     u32 = 0x0004;
pub const PU_NONE:           u32 = 0x0000;
pub const PU_MOUSEBUTTON1DOWN: u32 = 0x0008;
pub const PU_SELECTITEM:     u32 = 0x0020;
pub const PU_KEYBOARD:       u32 = 0x0200;

// WM_MINMAXFRAME — sent to a frame (passed on to its client) before it is
// minimized, maximized or restored. mp1 = PSWP; a TRUE reply skips the
// default action.
pub const WM_MINMAXFRAME: u32 = 0x0046;
/// Identity of a frame's minimize / maximize buttons (`<pmwin.h>`).
pub const FID_MINMAX: u32 = 0x8004;

// WM_ACTIVATE (0x000D) — sent to a frame when it gains or loses activation.
// mp1 = fActivate (TRUE = gaining focus, FALSE = losing); mp2 = other frame HWND.
pub const WM_ACTIVATE: u32 = 0x000D;

// WM_CONTROL — notification message from a child control to its owner.
// mp1 = MPFROM2SHORT(usID, usNotifyCode); mp2 = control-specific data.
pub const WM_CONTROL: u32 = 0x0030;

// Button notification codes (high word of WM_CONTROL mp1)
pub const BN_CLICKED: u32 = 1;
pub const BN_DBLCLICKED: u32 = 2;

// Dialog units → pixels. OS/2 defines one horizontal dialog unit as 1/4 of the
// system font's average character width and one vertical unit as 1/8 of its
// height, so the factors follow the metrics of the PM system font (IBM Plex
// Sans) rather than a fixed cell size.
pub const DLG_UNITS_PER_CHAR_W: i32 = 4;
pub const DLG_UNITS_PER_CHAR_H: i32 = 8;

/// Convert a horizontal dialog-unit value (DLGTEMPLATE x / cx) to pixels.
pub fn dlg_units_to_px_x(du: i16) -> i32 {
    let avg_w = crate::gui::font::metrics(crate::gui::font::FontSpec::SYSTEM).avg_char_width;
    (du as i32 * avg_w + DLG_UNITS_PER_CHAR_W / 2).div_euclid(DLG_UNITS_PER_CHAR_W)
}

/// Convert a vertical dialog-unit value (DLGTEMPLATE y / cy) to pixels.
pub fn dlg_units_to_px_y(du: i16) -> i32 {
    let line_h = crate::gui::font::metrics(crate::gui::font::FontSpec::SYSTEM).line_height;
    (du as i32 * line_h + DLG_UNITS_PER_CHAR_H / 2).div_euclid(DLG_UNITS_PER_CHAR_H)
}

/// Default size of the virtual PM desktop (pixels).
pub const DESKTOP_DEFAULT_W: u32 = 1024;
pub const DESKTOP_DEFAULT_H: u32 = 768;
/// Default size given to a frame by `WinCreateStdWindow`.
pub const FRAME_DEFAULT_CX: i32 = 640;
pub const FRAME_DEFAULT_CY: i32 = 480;
/// Offset of the first default-positioned frame from the desktop's top-left
/// corner, and the step between successive frames in the cascade.
pub const FRAME_CASCADE_ORIGIN: i32 = 32;
pub const FRAME_CASCADE_STEP: i32 = 24;
/// Number of cascade slots before default positions wrap around.
pub const FRAME_CASCADE_SLOTS: i32 = 8;

/// Size of the virtual PM desktop: `WARPINE_DESKTOP_W` / `WARPINE_DESKTOP_H`
/// when set to a valid number, otherwise 1024×768. The renderer's desktop
/// surface and the values the guest sees (`SV_CXSCREEN` / `SV_CYSCREEN`,
/// default window placement) all come from here so they cannot disagree.
pub fn desktop_size() -> (u32, u32) {
    let var = |name: &str, default: u32| {
        std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|&v| v > 0).unwrap_or(default)
    };
    (var("WARPINE_DESKTOP_W", DESKTOP_DEFAULT_W), var("WARPINE_DESKTOP_H", DESKTOP_DEFAULT_H))
}

/// Default bottom-left position (OS/2 desktop coordinates) of the `n`-th
/// frame created without an explicit position: a cascade stepping down and
/// right from the desktop's top-left corner, kept on the desktop.
pub fn default_frame_pos(n: usize, cx: i32, cy: i32, desktop: (u32, u32)) -> (i32, i32) {
    let off = FRAME_CASCADE_ORIGIN + (n as i32 % FRAME_CASCADE_SLOTS) * FRAME_CASCADE_STEP;
    let (dw, dh) = (desktop.0 as i32, desktop.1 as i32);
    let x = off.min(dw - cx).max(0);
    let y = (dh - cy - off).max(0);
    (x, y)
}

// VDR chrome dimensions, shared by the compositor and WinQuerySysValue.
/// Height of the title bar chrome in desktop pixels.
pub const CHROME_TITLE_H: i32 = 20;
/// Width of a frame that a title-bar drag keeps on the desktop past its
/// left edge.
pub const FRAME_MIN_VISIBLE_W: i32 = 32;
/// Width of the frame border in desktop pixels (each side).
pub const CHROME_BORDER_W: i32 = 2;
/// Size of a minimized frame (Warpine's own choice): a title bar with a
/// border, `MINIMIZED_CX` wide.
pub const MINIMIZED_CX: i32 = 160;
pub const MINIMIZED_CY: i32 = CHROME_TITLE_H + CHROME_BORDER_W;
/// Gap between minimized frames in the icon area along the bottom of the
/// desktop, and between them and the desktop's edges.
pub const ICON_AREA_GAP: i32 = 4;

/// Width of a frame's border: `CHROME_BORDER_W` with any border style, else 0.
pub fn frame_border_width(frame_flags: u32) -> i32 {
    if frame_flags & (FCF_BORDER | FCF_DLGBORDER | FCF_SIZEBORDER) != 0 { CHROME_BORDER_W } else { 0 }
}

/// Timer id of the system timer that blinks the cursor (`<pmwin.h>`):
/// Warpine posts `WM_TIMER(TID_CURSOR)` to the focused entry field or MLE.
pub const TID_CURSOR: u32 = 0xFFFF;
/// Timer id of the system scroll timer (`<pmwin.h>`): Warpine posts
/// `WM_TIMER(TID_SCROLL)` to an entry field, MLE or list box held with
/// button 1 while the pointer rests outside it, so it keeps scrolling.
pub const TID_SCROLL: u32 = 0xFFFE;
/// How long the caret of a text control stays on, then off (Warpine's own
/// choice): the initial `SV_CURSORRATE`.
pub const CARET_BLINK_MS: u64 = 500;
/// Interval of the drag scroll timer (`TID_SCROLL`) — Warpine's own choice;
/// TODO(unverified): the PM references give no rate for a text control
/// scrolling under a held pointer (`SV_SCROLLRATE` is for scroll bars).
pub const DRAG_SCROLL_MS: u64 = 100;

// ── Scroll bars (WC_SCROLLBAR, <pmwin.h> INCL_WINSCROLLBARS) ──────────────
/// Scroll-bar notifications to the owner: mp1 = MPFROMSHORT(id),
/// mp2 = MPFROM2SHORT(slider position, SB_* command).
pub const WM_VSCROLL: u32 = 0x0031;
pub const WM_HSCROLL: u32 = 0x0032;
/// Scroll-bar styles: horizontal (0) or vertical.
pub const SBS_HORZ:      u32 = 0x0000;
pub const SBS_VERT:      u32 = 0x0001;
pub const SBS_THUMBSIZE: u32 = 0x0002;
pub const SBS_AUTOTRACK: u32 = 0x0004;
pub const SBS_AUTOSIZE:  u32 = 0x2000;
/// `WM_VSCROLL` / `WM_HSCROLL` commands (high word of mp2).
pub const SB_LINEUP:         u32 = 1;
pub const SB_LINEDOWN:       u32 = 2;
pub const SB_LINELEFT:       u32 = 1;
pub const SB_LINERIGHT:      u32 = 2;
pub const SB_PAGEUP:         u32 = 3;
pub const SB_PAGEDOWN:       u32 = 4;
pub const SB_PAGELEFT:       u32 = 3;
pub const SB_PAGERIGHT:      u32 = 4;
pub const SB_SLIDERTRACK:    u32 = 5;
pub const SB_SLIDERPOSITION: u32 = 6;
pub const SB_ENDSCROLL:      u32 = 7;
/// Scroll-bar control messages.
pub const SBM_SETSCROLLBAR: u32 = 0x01a0;
pub const SBM_SETPOS:       u32 = 0x01a1;
pub const SBM_QUERYPOS:     u32 = 0x01a2;
pub const SBM_QUERYRANGE:   u32 = 0x01a3;
pub const SBM_SETTHUMBSIZE: u32 = 0x01a6;
/// Frame scroll-bar identities (`FCF_VERTSCROLL` / `FCF_HORZSCROLL`).
pub const FID_VERTSCROLL: u32 = 0x8006;
pub const FID_HORZSCROLL: u32 = 0x8007;
/// `SBCDATA` (pCtlData of a WC_SCROLLBAR, packed to 2): byte offsets of
/// `cb`, `sHilite`, `posFirst`, `posLast`, `posThumb`, `cVisible`, `cTotal`.
pub const SBCDATA_POS_FIRST: u32 = 4;
pub const SBCDATA_POS_LAST:  u32 = 6;
pub const SBCDATA_POS_THUMB: u32 = 8;
pub const SBCDATA_C_VISIBLE: u32 = 10;
pub const SBCDATA_C_TOTAL:   u32 = 12;
/// Size of `SBCDATA` (with `cVisible` / `cTotal`; a smaller `cb` has no
/// thumb size).
pub const SBCDATA_SIZE: u32 = 14;

// ── PM text cursor (WinCreateCursor, <pmwin.h> INCL_WINCURSORS) ────────────
pub const CURSOR_SOLID:    u32 = 0x0000;
pub const CURSOR_HALFTONE: u32 = 0x0001;
pub const CURSOR_FRAME:    u32 = 0x0002;
pub const CURSOR_FLASH:    u32 = 0x0004;
pub const CURSOR_SETPOS:   u32 = 0x8000;
pub const CURSOR_BIDI_FIRST: u32 = 0x0100;
pub const CURSOR_BIDI_LAST:  u32 = 0x0200;
/// `CURSORINFO`: hwnd, x, y, cx, cy, fs, rclClip (4 LONGs).
pub const CURSORINFO_SIZE: u32 = 40;

// WinQuerySysValue indices (SV_*, <pmwin.h>)
pub const SV_CXSIZEBORDER: i32 = 4;
pub const SV_CURSORRATE:   i32 = 9;
pub const SV_CYSIZEBORDER: i32 = 5;
pub const SV_CXSCREEN:     i32 = 20;
pub const SV_CYSCREEN:     i32 = 21;
pub const SV_CXVSCROLL:    i32 = 22;
pub const SV_CYHSCROLL:    i32 = 23;
pub const SV_CXBORDER:     i32 = 26;
pub const SV_CYBORDER:     i32 = 27;
pub const SV_CXDLGFRAME:   i32 = 28;
pub const SV_CYDLGFRAME:   i32 = 29;
pub const SV_CYTITLEBAR:   i32 = 30;
pub const SV_CYMENU:       i32 = 35;
pub const SV_CXFULLSCREEN: i32 = 36;
pub const SV_CYFULLSCREEN: i32 = 37;
pub const SV_CXPOINTER:    i32 = 40;
pub const SV_CYPOINTER:    i32 = 41;
pub const SV_POINTERLEVEL: i32 = 44;
pub const SV_MOUSEPRESENT: i32 = 48;

// WinQueryWindow relationship codes (QW_*, <pmwin.h>)
pub const QW_NEXT:   i32 = 0;
pub const QW_PREV:   i32 = 1;
pub const QW_TOP:    i32 = 2;
pub const QW_BOTTOM: i32 = 3;
pub const QW_OWNER:  i32 = 4;
pub const QW_PARENT: i32 = 5;
pub const QW_NEXTTOP:    i32 = 6;
pub const QW_PREVTOP:    i32 = 7;
/// The last relationship code; a larger (or negative) `lCode` is out of range.
pub const QW_FRAMEOWNER: i32 = 8;

// Common window style bits (flStyle in WinCreateWindow)
pub const WS_VISIBLE:  u32 = 0x8000_0000;
pub const WS_DISABLED: u32 = 0x4000_0000;
pub const WS_TABSTOP:  u32 = 0x0002_0000;
pub const WS_GROUP:    u32 = 0x0001_0000;
pub const WS_MINIMIZED: u32 = 0x0100_0000;
pub const WS_MAXIMIZED: u32 = 0x0080_0000;
/// Painting the window leaves its children alone (`WinInvalidateRect`).
pub const WS_CLIPCHILDREN: u32 = 0x2000_0000;

// WinQueryWindowULong index of the window style.
pub const QWL_STYLE: i32 = -2;
// WinQueryWindowPtr / WinSetWindowPtr index of the window procedure.
pub const QWP_PFNWP: i32 = -3;
// Lowest reserved window-word indices: WinQuery/SetWindowULong accept
// QWL_MIN..=-1 and WinQuery/SetWindowUShort QWS_MIN..=-1 below the
// application's own words (0 .. cbWindowData).
pub const QWL_MIN: i32 = -11;
pub const QWS_MIN: i32 = -1;

// Frame window words (WinQueryWindowUShort / WinSetWindowUShort): where a
// minimized or maximized frame is restored to, and where it was minimized.
pub const QWS_XRESTORE:  i32 = 0x000c;
pub const QWS_YRESTORE:  i32 = 0x000e;
pub const QWS_CXRESTORE: i32 = 0x0010;
pub const QWS_CYRESTORE: i32 = 0x0012;
pub const QWS_XMINIMIZE: i32 = 0x0014;
pub const QWS_YMINIMIZE: i32 = 0x0016;

// Button styles (low 16 bits of flStyle for WC_BUTTON windows)
pub const BS_PUSHBUTTON:     u32 = 0x0000;
pub const BS_CHECKBOX:       u32 = 0x0001;
pub const BS_AUTOCHECKBOX:   u32 = 0x0002;
pub const BS_RADIOBUTTON:    u32 = 0x0003;
pub const BS_AUTORADIOBUTTON: u32 = 0x0004;
pub const BS_3STATE:         u32 = 0x0005;
pub const BS_AUTO3STATE:     u32 = 0x0006;
pub const BS_USERBUTTON:     u32 = 0x0007;
/// Mask of the button kind (`BS_PUSHBUTTON`, `BS_CHECKBOX`, ...) in a style.
pub const BS_PRIMARYSTYLES:  u32 = 0x000F;
pub const BS_HELP:           u32 = 0x0100; // click posts WM_HELP
pub const BS_SYSCOMMAND:     u32 = 0x0200; // click posts WM_SYSCOMMAND
pub const BS_DEFAULT:        u32 = 0x0400; // default push button (Enter in a dialog)
pub const BS_NOPOINTERFOCUS: u32 = 0x0800; // a click does not take the focus
pub const BS_NOCURSORSELECT: u32 = 0x2000; // arrow keys do not check this auto-radio button

// Button messages (BM_*, <pmwin.h>)
/// mp1 = usUp: TRUE the up-click action (the click), FALSE the down-click.
pub const BM_CLICK:           u32 = 0x0120;
pub const BM_QUERYCHECKINDEX: u32 = 0x0121;
/// Returns whether a push button is highlighted (FALSE for other styles).
pub const BM_QUERYHILITE:     u32 = 0x0122;
/// mp1 = usHilite; returns the old highlight state.
pub const BM_SETHILITE:       u32 = 0x0123;
pub const BM_QUERYCHECK:      u32 = 0x0124;
pub const BM_SETCHECK:        u32 = 0x0125;
/// mp1 = usDefault; TRUE for a push / user button, FALSE for other styles.
pub const BM_SETDEFAULT:      u32 = 0x0126;

// Static control styles (low bits of flStyle for WC_STATIC windows)
pub const SS_TEXT:     u32 = 0x0001;
pub const SS_GROUPBOX: u32 = 0x0002;

// Frame styles (FS_*): the border a dialog template asks for.
pub const FS_DLGBORDER:  u32 = 0x0000_0080;
pub const FS_BORDER:     u32 = 0x0000_0100;
pub const FS_SIZEBORDER: u32 = 0x0000_0800;

// WM_COMMAND source codes: mp2 = MPFROM2SHORT(CMDSRC_*, fPointer).
pub const CMDSRC_OTHER:       u32 = 0;
pub const CMDSRC_PUSHBUTTON:  u32 = 1;
pub const CMDSRC_MENU:        u32 = 2;
pub const CMDSRC_ACCELERATOR: u32 = 3;

// WinPeekMsg flags (fl)
pub const PM_NOREMOVE: u32 = 0;
pub const PM_REMOVE:   u32 = 1;

// WM_HELP: posted by BS_HELP buttons and AF_HELP accelerators.
pub const WM_HELP: u32 = 0x0022;

// Accelerator flags (ACCEL.fs, <pmwin.h>)
pub const AF_CHAR:       u16 = 0x0001;
pub const AF_VIRTUALKEY: u16 = 0x0002;
pub const AF_SCANCODE:   u16 = 0x0004;
pub const AF_SHIFT:      u16 = 0x0008;
pub const AF_CONTROL:    u16 = 0x0010;
pub const AF_ALT:        u16 = 0x0020;
pub const AF_LONEKEY:    u16 = 0x0040;
pub const AF_SYSCOMMAND: u16 = 0x0100;
pub const AF_HELP:       u16 = 0x0200;

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
/// The OS/2 version Warpine reports (OS/2 Warp 4.5: `QSV_VERSION_MAJOR`
/// 20, `QSV_VERSION_MINOR` 45; `WinQueryVersion`).
pub const OS2_VERSION_MAJOR: u32 = 20;
pub const OS2_VERSION_MINOR: u32 = 45;
pub const QSV_VERSION_MAJOR: u32 = 11;
pub const QSV_VERSION_MINOR: u32 = 12;
pub const QSV_VERSION_REVISION: u32 = 13;
/// Milliseconds since system start-up (wraps at 2^32, about 49.7 days).
pub const QSV_MS_COUNT: u32 = 14;
/// Low and high doublewords of the current time in seconds since 1970-01-01.
pub const QSV_TIME_LOW: u32 = 15;
pub const QSV_TIME_HIGH: u32 = 16;
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
pub const MB_DEFBUTTON1:       u32 = 0x0000;
pub const MB_DEFBUTTON2:       u32 = 0x0100;
pub const MB_DEFBUTTON3:       u32 = 0x0200;
pub const MB_NOICON:           u32 = 0x0000;
pub const MB_ICONHAND:         u32 = 0x0040; // error
pub const MB_ICONQUESTION:     u32 = 0x0010; // question
pub const MB_ICONEXCLAMATION:  u32 = 0x0020; // warning
pub const MB_ICONASTERISK:     u32 = 0x0030; // information
pub const MB_CUANOTIFICATION:  u32 = 0x0000;
pub const MB_CUAWARNING:       u32 = 0x0020;
pub const MB_CUACRITICAL:      u32 = 0x0040;
/// An application icon (WinMessageBox2); `WinMessageBox` shows none.
pub const MB_CUSTOMICON:       u32 = 0x0080;
// Aliases (<pmwin.h> defines them by name; values repeated here).
pub const MB_QUERY:            u32 = MB_ICONQUESTION;
pub const MB_WARNING:          u32 = MB_CUAWARNING;
pub const MB_INFORMATION:      u32 = MB_ICONASTERISK;
pub const MB_CRITICAL:         u32 = MB_CUACRITICAL;
pub const MB_ERROR:            u32 = MB_CRITICAL;
/// Adds a Help push button that sends `WM_HELP` to the message box.
pub const MB_HELP:             u32 = 0x2000;

// System pointers (WinQuerySysPointer SPTR_*, <pmwin.h>); the icons are
// also the message-box icons.
pub const SPTR_ARROW:           u32 = 1;
pub const SPTR_TEXT:            u32 = 2;
pub const SPTR_WAIT:            u32 = 3;
pub const SPTR_SIZE:            u32 = 4;
pub const SPTR_MOVE:            u32 = 5;
pub const SPTR_SIZENWSE:        u32 = 6;
pub const SPTR_SIZENESW:        u32 = 7;
pub const SPTR_SIZEWE:          u32 = 8;
pub const SPTR_SIZENS:          u32 = 9;
pub const SPTR_APPICON:         u32 = 10;
pub const SPTR_ICONINFORMATION: u32 = 11;
pub const SPTR_ICONQUESTION:    u32 = 12;
pub const SPTR_ICONERROR:       u32 = 13;
pub const SPTR_ICONWARNING:     u32 = 14;
pub const SPTR_ILLEGAL:         u32 = 18;
pub const SPTR_FILE:            u32 = 19;
pub const SPTR_PROGRAM:         u32 = 22;
pub const SPTR_PENFIRST:        u32 = 23;
pub const SPTR_CPTR:            u32 = 39;
/// System pointer handles are this plus the `SPTR_*` index; pointers made
/// by WinLoadPointer and copies from WinQuerySysPointer get handles from
/// `FIRST_CREATED_HPOINTER` up, never reused.
pub const SYS_HPOINTER_BASE:      u32 = 0x5000;
pub const FIRST_CREATED_HPOINTER: u32 = 0x5100;
/// Side of the pointers of the display (SV_CXPOINTER / SV_CYPOINTER) and
/// the largest pointer image WinLoadPointer accepts.
pub const POINTER_SIZE:    u32 = 32;
pub const MAX_POINTER_DIM: u32 = 256;

// WinMessageBox return values (MBID_*)
pub const MBID_OK:     u32 = 1;
pub const MBID_CANCEL: u32 = 2;
pub const MBID_ABORT:  u32 = 3;
pub const MBID_RETRY:  u32 = 4;
pub const MBID_IGNORE: u32 = 5;
pub const MBID_YES:    u32 = 6;
pub const MBID_NO:     u32 = 7;
pub const MBID_HELP:   u32 = 8;
pub const MBID_ENTER:  u32 = 9;
/// `MBID_ERROR` (-1) as the ULONG `WinMessageBox` returns.
pub const MBID_ERROR:  u32 = 0xFFFF_FFFF;

// ── OS/2 Structured Exception Handling (SEH) ─────────────────────────────────

/// TIB byte offset of `tib_pexchain` (exception handler chain head pointer).
pub const TIB_EXCHAIN_OFFSET: u32 = 0x00;
/// TIB byte offsets of `tib_pstack`, `tib_pstacklimit`, `tib_ptib2` and
/// `tib_ordinal` (`<bsedos.h>`).
pub const TIB_PSTACK_OFFSET: u32 = 0x04;
pub const TIB_PSTACKLIMIT_OFFSET: u32 = 0x08;
pub const TIB_PTIB2_OFFSET: u32 = 0x0C;
pub const TIB_ORDINAL_OFFSET: u32 = 0x14;
/// Warpine's own TIB extras past the 0x18-byte `TIB`: a self pointer and
/// the PIB address.
pub const TIB_SELF_OFFSET: u32 = 0x18;
pub const TIB_PIB_OFFSET: u32 = 0x30;
/// Where Warpine places a thread's `TIB2`, from the start of its TIB.
pub const TIB2_FROM_TIB: u32 = 0x40;
/// `TIB2` byte offsets of `tib2_ultid` and `tib2_ulpri`.
pub const TIB2_TID_OFFSET: u32 = 0x00;
pub const TIB2_PRI_OFFSET: u32 = 0x04;
/// Guest memory reserved for a created thread's TIB + TIB2.
pub const THREAD_TIB_SIZE: u32 = 4096;
/// Stack size of a created thread when `DosCreateThread` asks for 0 bytes.
pub const DEFAULT_THREAD_STACK: u32 = 65536;
/// OS/2 error codes of the thread calls (`<bseerr.h>`).
pub const ERROR_MAX_THRDS_REACHED: u32 = 164;
pub const ERROR_THREAD_NOT_TERMINATED: u32 = 294;
pub const ERROR_INVALID_THREADID: u32 = 309;
/// `DosResumeThread` on a thread that is not suspended (`<bseerr.h>`).
pub const ERROR_NOT_FROZEN: u32 = 90;
/// `DosCreateThread` flags (`<bsedos.h>`).
pub const CREATE_READY: u32 = 0;
pub const CREATE_SUSPENDED: u32 = 1;
pub const STACK_SPARSE: u32 = 0;
pub const STACK_COMMITTED: u32 = 2;

// ── Signal exceptions (`<bsedos.h>`, `<bsexcpt.h>`, `<bseerr.h>`) ────────────

/// `DosSetSignalExceptionFocus` flag.
pub const SIG_UNSETFOCUS: u32 = 0;
pub const SIG_SETFOCUS: u32 = 1;
/// Signal numbers: `ExceptionInfo[0]` of an `XCPT_SIGNAL` exception.
pub const XCPT_SIGNAL_INTR: u32 = 1;
pub const XCPT_SIGNAL_KILLPROC: u32 = 3;
pub const XCPT_SIGNAL_BREAK: u32 = 4;
/// Error codes of the signal-exception calls.
pub const ERROR_SIGNAL_REFUSED: u32 = 156;
pub const ERROR_NO_SIGNAL_SENT: u32 = 205;
pub const ERROR_INVALID_SIGNAL_NUMBER: u32 = 209;
pub const ERROR_NESTING_TOO_DEEP: u32 = 650;
/// `DosKillProcess` actions.
pub const DKP_PROCESSTREE: u32 = 0;
pub const DKP_PROCESS: u32 = 1;
/// Error codes of `DosKillProcess`.
pub const ERROR_ZOMBIE_PROCESS: u32 = 217;
pub const ERROR_NOT_DESCENDANT: u32 = 305;

// ── Semaphores (`<bsedos.h>` INCL_DOSSEMAPHORES, `<bseerr.h>`) ───────────────

/// The process id of a process started without a Warpine parent (the
/// first process of a session); children get theirs from their parent
/// (`FIRST_CHILD_PROCESS_ID`, `inherit.rs`).
pub const PROCESS_ID: u32 = 42;
/// Semaphore attribute flags.
pub const DC_SEM_SHARED: u32 = 0x0001;
pub const DCMW_WAIT_ANY: u32 = 0x0002;
pub const DCMW_WAIT_ALL: u32 = 0x0004;
/// `DosCreateEventSem` flags of `<bsedos.h>`: a post releases one waiter
/// (`DCE_POSTONE`), a released wait resets the semaphore (`DCE_AUTORESET`).
pub const DCE_POSTONE: u32 = 0x0800;
pub const DCE_AUTORESET: u32 = 0x1000;
/// Name prefix every named 32-bit semaphore carries.
pub const SEM32_PREFIX: &str = "\\SEM32\\";
/// `sizeof(SEMRECORD)`: `HSEM hsemCur` + `ULONG ulUser`.
pub const SEMRECORD_SIZE: u32 = 8;
/// Most semaphores a muxwait list holds (Control Program Reference).
pub const MAX_MUXWAIT_SEMS: usize = 64;
/// Highest post count of an event semaphore.
pub const MAX_EVENT_POSTS: u32 = 65535;
/// Highest usage (open) count of a semaphore.
pub const MAX_SEM_OPENS: u32 = 65535;
/// Error codes of the semaphore calls.
pub const ERROR_INVALID_DATA: u32 = 13;
pub const ERROR_TOO_MANY_SEMAPHORES: u32 = 100;
/// A mutex semaphore's owner ended without releasing it.
pub const ERROR_SEM_OWNER_DIED: u32 = 105;
pub const ERROR_INVALID_NAME: u32 = 123;
pub const ERROR_SEM_NOT_FOUND: u32 = 187;
pub const ERROR_DUPLICATE_HANDLE: u32 = 284;
pub const ERROR_DUPLICATE_NAME: u32 = 285;
pub const ERROR_EMPTY_MUXWAIT: u32 = 286;
pub const ERROR_NOT_OWNER: u32 = 288;
pub const ERROR_PARAM_TOO_SMALL: u32 = 289;
pub const ERROR_TOO_MANY_HANDLES: u32 = 290;
pub const ERROR_TOO_MANY_OPENS: u32 = 291;
pub const ERROR_WRONG_TYPE: u32 = 292;
pub const ERROR_TOO_MANY_POSTS: u32 = 298;
/// Closing a semaphore another thread is blocked on.
pub const ERROR_SEM_BUSY: u32 = 301;
pub const ERROR_ALREADY_POSTED: u32 = 299;
pub const ERROR_ALREADY_RESET: u32 = 300;
pub const ERROR_TIMEOUT: u32 = 640;

// ── Exit lists (`<bsedos.h>`: DosExitList) ───────────────────────────────────

/// `DosExitList` actions (low byte of the order code).
pub const EXLST_ADD: u32 = 1;
pub const EXLST_REMOVE: u32 = 2;
pub const EXLST_EXIT: u32 = 3;
/// Termination codes an exit-list routine receives at `[ESP+4]`.
pub const TC_EXIT: u32 = 0;
pub const TC_HARDERROR: u32 = 1;
pub const TC_TRAP: u32 = 2;
pub const TC_KILLPROCESS: u32 = 3;
pub const TC_EXCEPTION: u32 = 4;

// ── Critical sections (`<bseerr.h>`: DosEnterCritSec / DosExitCritSec) ───────

pub const ERROR_CRITSEC_OVERFLOW: u32 = 484;
pub const ERROR_CRITSEC_UNDERFLOW: u32 = 485;

// ── Timers (`<bsedos.h>`: DosAsyncTimer / DosStartTimer / DosStopTimer) ──────

/// Error codes of the timer calls (`<bseerr.h>`).
pub const ERROR_TS_SEMHANDLE: u32 = 323;
pub const ERROR_TS_NOTIMER: u32 = 324;
pub const ERROR_TS_HANDLE: u32 = 326;
/// Warpine's system clock tick in milliseconds: timer intervals are rounded
/// up to it, and `DosQuerySysInfo(QSV_TIMER_INTERVAL)` reports it (in tenths
/// of a millisecond).
pub const DOS_TIMER_TICK_MS: u32 = 32;

// ── DosSearchPath / DosQueryModuleName (`<bsedos.h>`, `<bseerr.h>`) ──────────

/// PIB byte offset of `pib_pchenv` (the environment block).
pub const PIB_PCHENV_OFFSET: u32 = 0x10;
/// The module handle Warpine gives the executable (its initial stack and
/// resources use 0): `DosQueryModuleName(0)` names the executable.
pub const EXE_MODULE_HANDLE: u32 = 0;
pub const SEARCH_PATH: u32 = 0;
pub const SEARCH_CUR_DIRECTORY: u32 = 1;
pub const SEARCH_ENVIRONMENT: u32 = 2;
pub const SEARCH_IGNORENETERRS: u32 = 4;
pub const ERROR_BAD_LENGTH: u32 = 24;

// ── Memory suballocation (`<bsememf.h>`, `<bseerr.h>`) ───────────────────────

pub const DOSSUB_INIT: u32 = 1;
pub const DOSSUB_GROW: u32 = 2;
pub const DOSSUB_SPARSE_OBJ: u32 = 4;
pub const DOSSUB_SERIALIZE: u32 = 8;
pub const ERROR_DOSSUB_SHRINK: u32 = 310;
pub const ERROR_DOSSUB_NOMEM: u32 = 311;
pub const ERROR_DOSSUB_OVERLAP: u32 = 312;
pub const ERROR_DOSSUB_BADSIZE: u32 = 313;
pub const ERROR_DOSSUB_BADFLAG: u32 = 314;
pub const ERROR_DOSSUB_CORRUPTED: u32 = 532;
/// Sentinel stored at the tail of the exception handler chain (no further handlers).
pub const XCPT_CHAIN_END: u32 = 0xFFFF_FFFF;

// Exception handler return values (from the handler function)
/// Handler fixed the exception — resume at the fault address.
pub const XCPT_CONTINUE_EXECUTION: u32 = 0xFFFF_FFFF;
/// Handler did not handle the exception — try the next one in the chain.
pub const XCPT_CONTINUE_SEARCH:    u32 = 0x0000_0000;

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
pub const XCPT_GUARD_PAGE_VIOLATION:    u32 = 0x8000_0001;
pub const XCPT_UNABLE_TO_GROW_STACK:    u32 = 0x8001_0001;
pub const XCPT_ACCESS_VIOLATION:        u32 = 0xC000_0005;
pub const XCPT_IN_PAGE_ERROR:           u32 = 0xC000_0006;
pub const XCPT_ILLEGAL_INSTRUCTION:     u32 = 0xC000_001C;
pub const XCPT_INVALID_LOCK_SEQUENCE:   u32 = 0xC000_001D;
pub const XCPT_FLOAT_DENORMAL_OPERAND:  u32 = 0xC000_0094;
pub const XCPT_FLOAT_DIVIDE_BY_ZERO:    u32 = 0xC000_0095;
pub const XCPT_FLOAT_INEXACT_RESULT:    u32 = 0xC000_0096;
pub const XCPT_FLOAT_INVALID_OPERATION: u32 = 0xC000_0097;
pub const XCPT_FLOAT_OVERFLOW:          u32 = 0xC000_0098;
pub const XCPT_FLOAT_STACK_CHECK:       u32 = 0xC000_0099;
pub const XCPT_FLOAT_UNDERFLOW:         u32 = 0xC000_009A;
pub const XCPT_INTEGER_DIVIDE_BY_ZERO:  u32 = 0xC000_009B;
pub const XCPT_INTEGER_OVERFLOW:        u32 = 0xC000_009C;
pub const XCPT_PRIVILEGED_INSTRUCTION:  u32 = 0xC000_009D;
pub const XCPT_DATATYPE_MISALIGNMENT:   u32 = 0xC000_009E;
pub const XCPT_BREAKPOINT:              u32 = 0xC000_009F;
pub const XCPT_SINGLE_STEP:             u32 = 0xC000_00A0;
pub const XCPT_SIGNAL:                  u32 = 0xC001_0003;
/// Catch-all for unclassified hardware faults.
pub const XCPT_FATAL_EXCEPTION:         u32 = 0xC000_0000;

// ERRORID severities (<os2def.h>): the high word of a WinGetLastError value.
pub const SEVERITY_NOERROR:       u32 = 0x0000;
pub const SEVERITY_WARNING:       u32 = 0x0004;
pub const SEVERITY_ERROR:         u32 = 0x0008;
pub const SEVERITY_SEVERE:        u32 = 0x000C;
pub const SEVERITY_UNRECOVERABLE: u32 = 0x0010;

// PM error codes (<pmerr.h>): the low word of a WinGetLastError value.
pub const PMERR_INVALID_HWND:             u32 = 0x1001;
pub const PMERR_INVALID_HMQ:              u32 = 0x1002;
pub const PMERR_PARAMETER_OUT_OF_RANGE:   u32 = 0x1003;
pub const PMERR_RESOURCE_NOT_FOUND:       u32 = 0x100A;
pub const PMERR_INVALID_FLAG:             u32 = 0x1019;
pub const PMERR_INVALID_HACCEL:           u32 = 0x101A;
pub const PMERR_INVALID_HPTR:             u32 = 0x101B;
pub const PMERR_NO_MSG_QUEUE:             u32 = 0x1036;
pub const PMERR_MSG_QUEUE_ALREADY_EXISTS: u32 = 0x1052;
pub const PMERR_INVALID_STRING_PARM:      u32 = 0x100B;
pub const PMERR_INVALID_ATOM_NAME:        u32 = 0x1015;
pub const PMERR_INVALID_INTEGER_ATOM:     u32 = 0x1016;
pub const PMERR_ATOM_NAME_NOT_FOUND:      u32 = 0x1017;
pub const PMERR_INV_HDC:                  u32 = 0x207C;
pub const PMERR_INV_HPS:                  u32 = 0x207F;
pub const PMERR_INV_DRAW_BORDER_OPTION:   u32 = 0x2068;

// ── Window lookups, input state and system colours (pm_query.rs,
// pm_keystate.rs, pm_syscolor.rs), <pmwin.h> ─────────────────────────────────
/// `WinEnumDlgItem` codes.
pub const EDI_FIRSTTABITEM:   u32 = 0;
pub const EDI_LASTTABITEM:    u32 = 1;
pub const EDI_NEXTTABITEM:    u32 = 2;
pub const EDI_PREVTABITEM:    u32 = 3;
pub const EDI_FIRSTGROUPITEM: u32 = 4;
pub const EDI_LASTGROUPITEM:  u32 = 5;
pub const EDI_NEXTGROUPITEM:  u32 = 6;
pub const EDI_PREVGROUPITEM:  u32 = 7;
/// Class styles reported by `WinQueryClassInfo` for the preregistered classes.
pub const CS_PUBLIC: u32 = 0x0000_0010;
pub const CS_FRAME:  u32 = 0x0000_0020;
/// Virtual keys of the pointing-device buttons and the shift keys.
pub const VK_BUTTON1:   u32 = 0x01;
pub const VK_BUTTON2:   u32 = 0x02;
pub const VK_BUTTON3:   u32 = 0x03;
pub const VK_SHIFT:     u32 = 0x09;
pub const VK_CTRL:      u32 = 0x0a;
pub const VK_ALT:       u32 = 0x0b;
pub const VK_ALTGRAF:   u32 = 0x0c;
pub const VK_PAUSE:     u32 = 0x0d;
pub const VK_CAPSLOCK:  u32 = 0x0e;
pub const VK_PRINTSCRN: u32 = 0x19;
/// `WinGetKeyState` / `WinGetPhysKeyState` result bits (the PM Reference:
/// 0x8000 "The key is down", 0x0001 toggled / pressed since the last call).
pub const KEY_STATE_DOWN:    u32 = 0x8000;
pub const KEY_STATE_TOGGLED: u32 = 0x0001;
/// Sent to the main windows when `WinSetSysColors` changes a system colour.
pub const WM_SYSCOLORCHANGE: u32 = 0x002b;
/// `WinSetSysColors` option (`<pmgpi.h>`; `LCOL_RESET` is above).
pub const LCOL_PURECOLOR: u32 = 0x0004;
/// System colour indices (`SYSCLR_*`): `SYSCLR_CSYSCOLORS` consecutive
/// negative values from `SYSCLR_SHADOWHILITEBGND` to `SYSCLR_HELPHILITE`.
pub const SYSCLR_SHADOWHILITEBGND:      i32 = -50;
pub const SYSCLR_SHADOWHILITEFGND:      i32 = -49;
pub const SYSCLR_SHADOWTEXT:            i32 = -48;
pub const SYSCLR_ENTRYFIELD:            i32 = -47;
pub const SYSCLR_MENUDISABLEDTEXT:      i32 = -46;
pub const SYSCLR_MENUHILITE:            i32 = -45;
pub const SYSCLR_MENUHILITEBGND:        i32 = -44;
pub const SYSCLR_PAGEBACKGROUND:        i32 = -43;
pub const SYSCLR_FIELDBACKGROUND:       i32 = -42;
pub const SYSCLR_BUTTONLIGHT:           i32 = -41;
pub const SYSCLR_BUTTONMIDDLE:          i32 = -40;
pub const SYSCLR_BUTTONDARK:            i32 = -39;
pub const SYSCLR_BUTTONDEFAULT:         i32 = -38;
pub const SYSCLR_TITLEBOTTOM:           i32 = -37;
pub const SYSCLR_SHADOW:                i32 = -36;
pub const SYSCLR_ICONTEXT:              i32 = -35;
pub const SYSCLR_DIALOGBACKGROUND:      i32 = -34;
pub const SYSCLR_HILITEFOREGROUND:      i32 = -33;
pub const SYSCLR_HILITEBACKGROUND:      i32 = -32;
pub const SYSCLR_INACTIVETITLETEXTBGND: i32 = -31;
pub const SYSCLR_ACTIVETITLETEXTBGND:   i32 = -30;
pub const SYSCLR_INACTIVETITLETEXT:     i32 = -29;
pub const SYSCLR_ACTIVETITLETEXT:       i32 = -28;
pub const SYSCLR_OUTPUTTEXT:            i32 = -27;
pub const SYSCLR_WINDOWSTATICTEXT:      i32 = -26;
pub const SYSCLR_SCROLLBAR:             i32 = -25;
pub const SYSCLR_BACKGROUND:            i32 = -24;
pub const SYSCLR_ACTIVETITLE:           i32 = -23;
pub const SYSCLR_INACTIVETITLE:         i32 = -22;
pub const SYSCLR_MENU:                  i32 = -21;
pub const SYSCLR_WINDOW:                i32 = -20;
pub const SYSCLR_WINDOWFRAME:           i32 = -19;
pub const SYSCLR_MENUTEXT:              i32 = -18;
pub const SYSCLR_WINDOWTEXT:            i32 = -17;
pub const SYSCLR_TITLETEXT:             i32 = -16;
pub const SYSCLR_ACTIVEBORDER:          i32 = -15;
pub const SYSCLR_INACTIVEBORDER:        i32 = -14;
pub const SYSCLR_APPWORKSPACE:          i32 = -13;
pub const SYSCLR_HELPBACKGROUND:        i32 = -12;
pub const SYSCLR_HELPTEXT:              i32 = -11;
pub const SYSCLR_HELPHILITE:            i32 = -10;
pub const SYSCLR_CSYSCOLORS:            u32 = 41;
pub const PMERR_INV_LENGTH_OR_COUNT:      u32 = 0x2092;

// WinScrollWindow options (<pmwin.h>).
pub const SW_SCROLLCHILDREN: u32 = 1;
pub const SW_INVALIDATERGN:  u32 = 2;

// Region complexity (<pmgpi.h>), returned by WinScrollWindow.
pub const RGN_ERROR:   u32 = 0;
pub const RGN_NULL:    u32 = 1;
pub const RGN_RECT:    u32 = 2;
pub const RGN_COMPLEX: u32 = 3;

// ── Regions, clipping, areas, paths and polygons (<pmgpi.h>, <pmerr.h>) ─────
// GpiCombineRegion lMode.
pub const CRGN_OR:   u32 = 1;
pub const CRGN_COPY: u32 = 2;
pub const CRGN_XOR:  u32 = 4;
pub const CRGN_AND:  u32 = 6;
pub const CRGN_DIFF: u32 = 7;
// GpiEqualRegion results.
pub const EQRGN_ERROR:    u32 = 0;
pub const EQRGN_NOTEQUAL: u32 = 1;
pub const EQRGN_EQUAL:    u32 = 2;
/// `GpiSetClipRegion` old-region result on error.
pub const HRGN_ERROR: u32 = 0xFFFF_FFFF;
// GpiPtInRegion results.
pub const PRGN_ERROR:   u32 = 0;
pub const PRGN_OUTSIDE: u32 = 1;
pub const PRGN_INSIDE:  u32 = 2;
// GpiRectInRegion results.
pub const RRGN_ERROR:   u32 = 0;
pub const RRGN_OUTSIDE: u32 = 1;
pub const RRGN_PARTIAL: u32 = 2;
pub const RRGN_INSIDE:  u32 = 3;
// RGNRECT ulDirection.
pub const RECTDIR_LFRT_TOPBOT: u32 = 1;
pub const RECTDIR_RTLF_TOPBOT: u32 = 2;
pub const RECTDIR_LFRT_BOTTOP: u32 = 3;
pub const RECTDIR_RTLF_BOTTOP: u32 = 4;
// GpiBeginArea flOptions.
pub const BA_NOBOUNDARY: u32 = 0;
pub const BA_BOUNDARY:   u32 = 1;
pub const BA_ALTERNATE:  u32 = 0;
pub const BA_WINDING:    u32 = 2;
pub const BA_INCL:       u32 = 0;
pub const BA_EXCL:       u32 = 8;
// GpiFillPath / GpiPathToRegion lOptions.
pub const FPATH_ALTERNATE: u32 = 0;
pub const FPATH_WINDING:   u32 = 2;
pub const FPATH_INCL:      u32 = 0;
pub const FPATH_EXCL:      u32 = 8;
/// GpiModifyPath lMode.
pub const MPATH_STROKE: u32 = 6;
// GpiSetClipPath lOptions.
pub const SCP_ALTERNATE: u32 = 0;
pub const SCP_WINDING:   u32 = 2;
pub const SCP_AND:       u32 = 4;
pub const SCP_RESET:     u32 = 0;
pub const SCP_INCL:      u32 = 0;
pub const SCP_EXCL:      u32 = 8;
// GpiPolygons flOptions / flModel.
pub const POLYGON_NOBOUNDARY: u32 = 0;
pub const POLYGON_BOUNDARY:   u32 = 1;
pub const POLYGON_ALTERNATE:  u32 = 0;
pub const POLYGON_WINDING:    u32 = 2;
pub const POLYGON_INCL:       u32 = 0;
pub const POLYGON_EXCL:       u32 = 8;
pub const POLYGON_FILL:       u32 = 0;
pub const POLYGON_NOFILL:     u32 = 16;
/// Size of a `POLYGON` (ULONG ulPoints, PPOINTL aPointl).
pub const POLYGON_SIZE: u32 = 8;
/// Size of a `RGNRECT` (ircStart, crc, crcReturned, ulDirection).
pub const RGNRECT_SIZE: u32 = 16;
// Errors of these calls.
pub const PMERR_ALREADY_IN_AREA:       u32 = 0x2001;
pub const PMERR_ALREADY_IN_PATH:       u32 = 0x2003;
pub const PMERR_COORDINATE_OVERFLOW:   u32 = 0x2014;
pub const PMERR_HRGN_BUSY:             u32 = 0x2034;
pub const PMERR_INSUFFICIENT_MEMORY:   u32 = 0x203E;
pub const PMERR_INV_AREA_CONTROL:      u32 = 0x2041;
pub const PMERR_INV_CLIP_PATH_OPTIONS: u32 = 0x2051;
pub const PMERR_INV_COORDINATE:        u32 = 0x205B;
pub const PMERR_INV_FILL_PATH_OPTIONS: u32 = 0x2070;
pub const PMERR_INV_HRGN:              u32 = 0x2080;
pub const PMERR_INV_IN_AREA:           u32 = 0x2085;
pub const PMERR_INV_IN_PATH:           u32 = 0x208B;
pub const PMERR_INV_MODIFY_PATH_MODE:  u32 = 0x20A6;
pub const PMERR_INV_PATH_ID:           u32 = 0x20AE;
pub const PMERR_INV_RECT:              u32 = 0x20BD;
pub const PMERR_INV_REGION_CONTROL:    u32 = 0x20BE;
pub const PMERR_INV_REGION_MIX_MODE:   u32 = 0x20BF;
pub const PMERR_INV_RESERVED_FIELD:    u32 = 0x20C1;
pub const PMERR_INV_SHARPNESS_PARM:    u32 = 0x20CD;
pub const PMERR_NOT_IN_AREA:           u32 = 0x20DD;
pub const PMERR_NOT_IN_PATH:           u32 = 0x20E1;
pub const PMERR_PATH_UNKNOWN:          u32 = 0x20EE;
pub const PMERR_REGION_IS_CLIP_REGION: u32 = 0x20F8;

// WinDrawBorder flCmd (<pmwin.h>): a raster operation (DB_ROP) and flags.
pub const DB_PATCOPY:     u32 = 0x0000;
pub const DB_PATINVERT:   u32 = 0x0001;
pub const DB_DESTINVERT:  u32 = 0x0002;
pub const DB_AREAMIXMODE: u32 = 0x0003;
pub const DB_ROP:         u32 = 0x0007;
pub const DB_INTERIOR:    u32 = 0x0008;
pub const DB_AREAATTRS:   u32 = 0x0010;
pub const DB_STANDARD:    u32 = 0x0100;
pub const DB_DLGBORDER:   u32 = 0x0200;

// WinDrawText flags (<pmwin.h>).
pub const DT_LEFT:            u32 = 0x0000_0000;
pub const DT_QUERYEXTENT:     u32 = 0x0000_0002;
pub const DT_UNDERSCORE:      u32 = 0x0000_0010;
pub const DT_STRIKEOUT:       u32 = 0x0000_0020;
pub const DT_TEXTATTRS:       u32 = 0x0000_0040;
pub const DT_EXTERNALLEADING: u32 = 0x0000_0080;
pub const DT_CENTER:          u32 = 0x0000_0100;
pub const DT_RIGHT:           u32 = 0x0000_0200;
pub const DT_TOP:             u32 = 0x0000_0000;
pub const DT_VCENTER:         u32 = 0x0000_0400;
pub const DT_BOTTOM:          u32 = 0x0000_0800;
pub const DT_HALFTONE:        u32 = 0x0000_1000;
pub const DT_MNEMONIC:        u32 = 0x0000_2000;
pub const DT_WORDBREAK:       u32 = 0x0000_4000;
pub const DT_ERASERECT:       u32 = 0x0000_8000;

// Profile (Prf*) errors (<pmerr.h>).
pub const PMERR_BUFFER_TOO_SMALL:         u32 = 0x110B;
pub const PMERR_INVALID_INI_FILE_HANDLE:  u32 = 0x1115;
pub const PMERR_INI_FILE_IS_SYS_OR_USER:  u32 = 0x1124;
pub const PMERR_OPENING_INI_FILE:         u32 = 0x1301;
pub const PMERR_INI_FILE_CORRUPT:         u32 = 0x1302;
pub const PMERR_INVALID_PARM:             u32 = 0x1303;
pub const PMERR_NOT_IN_IDX:               u32 = 0x1304;
pub const PMERR_INI_WRITE_FAIL:           u32 = 0x1306;

// Profile handles (<pmwin.h>, INCL_WINSHELLDATA): HINI_PROFILE is both the
// user and the system profile (user first), the others one of them.
pub const HINI_PROFILE:       u32 = 0;
pub const HINI_USERPROFILE:   u32 = 0xFFFF_FFFF; // (HINI)-1
pub const HINI_SYSTEMPROFILE: u32 = 0xFFFF_FFFE; // (HINI)-2

// Help Manager (<pmhelp.h>): HM_* messages start at HM_MSG_BASE.
pub const HM_MSG_BASE:          u32 = 0x0220;
pub const HM_DISPLAY_HELP:      u32 = HM_MSG_BASE + 0x0002;
pub const HM_SET_ACTIVE_WINDOW: u32 = HM_MSG_BASE + 0x0004;
pub const HM_CONTROL:           u32 = HM_MSG_BASE + 0x0024;

// ── Process ids (`pib_ulpid` / `pib_ulppid`, `inherit.rs`) ──────────────────

/// PIB byte offset of `pib_ulpid`.
pub const PIB_PID_OFFSET: u32 = 0x00;
/// PIB byte offset of `pib_ulppid`.
pub const PIB_PPID_OFFSET: u32 = 0x04;
/// PIB byte offset of `pib_pchcmd` (the command line).
pub const PIB_PCHCMD_OFFSET: u32 = 0x0C;
/// The parent id of a process started without a Warpine parent.
pub const NO_PARENT_PROCESS_ID: u32 = 0;
/// The first id a session gives a child process; ids below it are never
/// handed out, so they cannot meet `PROCESS_ID`.
pub const FIRST_CHILD_PROCESS_ID: u32 = 100;
/// The largest process id: ids stay within the 16-bit `PID` of the 16-bit
/// API; after it, numbering starts again at `FIRST_CHILD_PROCESS_ID`.
pub const MAX_PROCESS_ID: u32 = 0xFFFF;

#[cfg(test)]
mod header_tests {
    //! Every constant named after an OS/2 toolkit `#define` must have the
    //! header's value: guest programs are compiled against those headers.
    //! Runs when the Open Watcom headers are installed
    //! (`./vendor/setup_watcom.sh`); they are not part of the repository.

    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    /// Constants that share a toolkit name but mean something else here.
    const NOT_TOOLKIT: &[&str] = &[
        "ATTR_ERROR", // cmd.rs: the VGA text attribute for error messages
    ];

    fn parse_int(token: &str) -> Option<i64> {
        let t = token.trim().replace('_', "");
        let (neg, t) = t.strip_prefix('-').map_or((false, t.as_str()), |r| (true, r));
        let t = t.trim_end_matches(['L', 'l', 'U', 'u']);
        let v = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some(hex) => i64::from_str_radix(hex, 16).ok()?,
            None => t.parse().ok()?,
        };
        Some(if neg { -v } else { v })
    }

    /// The value of a `#define NAME value` line: a plain number, optionally
    /// in parentheses and with a cast such as `(HWND)1`.
    fn define_value(rest: &str) -> Option<i64> {
        let rest = rest.split("/*").next()?.split("//").next()?.trim();
        let mut t = rest.trim_start_matches('(').trim_end_matches(')').trim();
        if let Some(i) = t.find(')') {
            t = t[i + 1..].trim().trim_start_matches('(').trim_end_matches(')');
        }
        parse_int(t)
    }

    fn header_values(dir: &Path) -> HashMap<String, HashSet<i64>> {
        let mut out: HashMap<String, HashSet<i64>> = HashMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "h") {
                    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
                    for line in text.lines() {
                        let Some(rest) = line.trim_start().strip_prefix("#define") else { continue };
                        let mut parts = rest.trim_start().splitn(2, char::is_whitespace);
                        let (Some(name), Some(value)) = (parts.next(), parts.next()) else { continue };
                        if let Some(v) = define_value(value) {
                            out.entry(name.to_string()).or_default().insert(v);
                        }
                    }
                }
            }
        }
        out
    }

    /// `(file:line, name, value, bits)` of every integer `const` in `src/`.
    fn source_constants(dir: &Path) -> Vec<(String, String, i64, u32)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") { continue; }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    let Some(pos) = line.find("const ") else { continue };
                    let decl = &line[pos + 6..];
                    let Some((name, rest)) = decl.split_once(':') else { continue };
                    let Some((ty, value)) = rest.split_once('=') else { continue };
                    let bits = match ty.trim() {
                        "u8" | "i8" => 8, "u16" | "i16" => 16, "u32" | "i32" => 32, _ => continue,
                    };
                    let value = value.split(';').next().unwrap_or("").trim();
                    let value = value.split(" as ").next().unwrap_or("");
                    if let Some(v) = parse_int(value) {
                        out.push((format!("{}:{}", path.display(), i + 1),
                                  name.trim().to_string(), v, bits));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn constants_match_toolkit_headers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let headers = root.join("vendor/watcom/h/os2");
        if !headers.is_dir() {
            eprintln!("skipped: Open Watcom headers not installed ({})", headers.display());
            return;
        }
        let defs = header_values(&headers);
        assert!(defs.contains_key("WM_COMMAND"), "header parsing found no PM messages");
        let mut wrong = Vec::new();
        let mut checked = 0;
        for (at, name, value, bits) in source_constants(&root.join("src")) {
            let Some(expected) = defs.get(&name) else { continue };
            if NOT_TOOLKIT.contains(&name.as_str()) { continue; }
            checked += 1;
            let mask = (1i64 << bits) - 1;
            if !expected.iter().any(|&e| e & mask == value & mask) {
                wrong.push(format!("{at}: {name} = {value:#x}, header {expected:x?}"));
            }
        }
        assert!(checked > 100, "only {checked} constants compared");
        // The WinGetLastError values must be compared, not skipped as unknown.
        for name in ["SEVERITY_NOERROR", "SEVERITY_WARNING", "SEVERITY_ERROR", "SEVERITY_SEVERE",
                     "SEVERITY_UNRECOVERABLE", "PMERR_INVALID_HWND", "PMERR_INVALID_HMQ",
                     "PMERR_NO_MSG_QUEUE", "PMERR_MSG_QUEUE_ALREADY_EXISTS",
                     "PMERR_PARAMETER_OUT_OF_RANGE", "PMERR_INVALID_FLAG", "PMERR_INVALID_HACCEL",
                     "QW_FRAMEOWNER", "QWL_MIN", "QWS_MIN", "HWND_THREADCAPTURE",
                     "SV_CURSORRATE", "TID_SCROLL", "SLM_SETSLIDERINFO", "SLN_SLIDERTRACK",
                     "SLS_PRIMARYSCALE2", "SDA_SLIDERARM", "VM_SETMETRICS", "VN_ENTER",
                     "VIA_DISABLED", "VS_RIGHTTOLEFT", "VDA_BACKGROUND"] {
            assert!(defs.contains_key(name), "{name} is not defined in the toolkit headers");
        }
        assert!(wrong.is_empty(), "constants differ from the toolkit headers:\n{}", wrong.join("\n"));
    }

    /// The `WC_*_ATOM` constants are named differently from the header's
    /// `WC_*` class names, so the test above does not see them.
    #[test]
    fn class_atoms_match_toolkit_headers() {
        use super::*;
        let headers = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/watcom/h/os2");
        if !headers.is_dir() {
            eprintln!("skipped: Open Watcom headers not installed ({})", headers.display());
            return;
        }
        let defs = header_values(&headers);
        for (name, atom) in [
            ("WC_FRAME", WC_FRAME_ATOM), ("WC_COMBOBOX", WC_COMBOBOX_ATOM), ("WC_BUTTON", WC_BUTTON_ATOM),
            ("WC_MENU", WC_MENU_ATOM), ("WC_STATIC", WC_STATIC_ATOM), ("WC_ENTRYFIELD", WC_ENTRYFIELD_ATOM),
            ("WC_LISTBOX", WC_LISTBOX_ATOM), ("WC_SCROLLBAR", WC_SCROLLBAR_ATOM),
            ("WC_TITLEBAR", WC_TITLEBAR_ATOM), ("WC_MLE", WC_MLE_ATOM),
            ("WC_SPINBUTTON", WC_SPINBUTTON_ATOM), ("WC_CONTAINER", WC_CONTAINER_ATOM),
            ("WC_NOTEBOOK", WC_NOTEBOOK_ATOM), ("WC_SLIDER", WC_SLIDER_ATOM),
            ("WC_VALUESET", WC_VALUESET_ATOM),
        ] {
            let expected = defs.get(name).unwrap_or_else(|| panic!("{name} not in the headers"));
            assert!(expected.contains(&(atom as i64)), "{name}: {atom:#x}, header {expected:x?}");
        }
    }
}

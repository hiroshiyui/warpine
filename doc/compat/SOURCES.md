# API compatibility chart: data and sources

`os2_api_versions.tsv` dates every 32-bit export of the Open Watcom import library `os2386.lib` (as listed in `doc/os2_ordinals.md`) with the OS/2 version it first appeared in: `2.0` (2.1 / 2.11 additions folded in), `3.0` (Warp 3), `4.0` (Warp 4), `later` (4.5 and after) or `unknown`. Columns: module, ordinal, name, since, source id, note (the reason for the line). `other_api_versions.tsv` does the same, in the same columns, for the full documented export lists of the modules outside that library (VIO / KBD / MOU, REXX, the SOM kernel, MSG, PMCTLS, UCONV, MMPM/2, TCP/IP; the ordinal is `-` where no public source gives it; section [Modules outside os2386.lib](#modules-outside-os2386lib-other_api_versionstsv) below). `excluded.tsv` lists the lines left out of both tables (Workplace Shell class-data exports, truncated duplicate names, `DOSPTRACE`, internal or 1.x-only entry points, Warpine's own names); 16-bit `DOS16*` entry points were filtered before.

`gen_compat` (`src/bin/gen_compat.rs`, ADR-0033) reads both tables and Warpine's source (an export of the second is matched by module and name through `targets/os2api.def`) and renders `api_coverage.svg` / `api_coverage_dark.svg`, shown at the head of the README. Exports dated `later` or `unknown` count in no version. Lines marked `H1` rest on the ordinal-chronology rule below, not on a document.

The dating was researched from public documentation only (near-clean-room policy, ADR-0004); no IBM binary was examined. It was compiled on 2026-11-18 and extended to the other modules on 2026-11-24; corrections with a source are welcome.

All sources are public documentation (IBM manuals and Redbooks as scanned on
bitsavers, the IBM Toolkit 4.5 INF books as HTML on os2.kr, the EDM/2 wiki) or
the local Open Watcom snapshot. No IBM binary was examined.

## Sources

| id | Title | Location | What it gives us |
|----|-------|----------|------------------|
| S1 | IBM *OS/2 2.0 Control Program Programming Reference*, S10G-6263-00, March 1992 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_2.x/IBM_OS2_2.0_Technical_Library_1992/S10G-6263-00_OS2_2.0_Control_Program_Programming_Reference_199203.pdf | The 2.0 `Dos*` set (DOSCALLS, NLS, QUECALLS, SESMGR, MSG). OCR text searched for every name. |
| S2 | IBM *OS/2 2.0 Presentation Manager Programming Reference* Vol. 1 (S10G-6264-00) and Vol. 2 (S10G-6265-00), March 1992 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_2.x/IBM_OS2_2.0_Technical_Library_1992/ | The 2.0 `Win*`, `Gpi*`, `Dev*`, `Drg*`, `Prf*`, `Spl*`, `Ddf*`, standard dialogs, WPS `Win*Object*` calls. |
| S4 | EDM/2, *What APIs are new to OS/2 2.1?* | https://www.edm2.com/index.php/What_APIs_are_new_to_OS/2_2.1%3F | 2.1 additions: the 12 `DosProtect*` calls and `WinCheckInput`, `WinLockPointerUpdate`, `WinLockupSystem`, `WinUnlockSystem`, `WinQuerySysPointerData`, `WinSetSysPointerData`, `WinSetPointerOwner`. These count as 2.x (2.0 bucket). |
| S5 | IBM *OS/2 Warp Version 3 Presentation Manager Programming Reference* Vol. 1 (G25H-7190-00) and Vol. 2 (G25H-7191-00), Oct 1994 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_3.x/ | PM names first documented in Warp 3: lazy drag (`DrgLazyDrag` … `DrgReallocDraginfo`), `WinMessageBox2`, visible-region calls, `SplMessageBox`, the Warp 3 WPS calls (`WinOpenObject`, `WinMoveObject`, `WinCopyObject`, `WinCreateShadow`, `WinSaveObject`, `WinQueryObjectPath`, `WinQueryActiveDesktopPathname`, DSOM `WinRestartWPDServer` / `WinRestartSOMDD` / `WinIs*Ready`). These volumes have no GPI function pages; GPI is covered only through the PMERR lists. |
| S6 | IBM Redbooks *OS/2 Debugging Handbook* Vol. 4 *System Diagnostic Reference* (SG24-4643, Feb 1996) and Vol. 3 *System Trace Reference* (SG24-4642, Feb 1996) | https://bitsavers.org/pdf/ibm/pc/os2/OS2_3.x/ | Vol. 4 Table 168 "System Ordinal Cross-Reference" lists every DOSCALLS entry point of Warp 3 (with fixpaks) by ordinal (copied to `warp3_doscalls_ordinal_table.txt`). Vol. 4 Ch. 2 documents `DosForceSystemDump`, `DosDumpProcess`, `DosSuppressPopUps`, `DosQueryRASInfo` for 2.11/Warp. Vol. 3 has PM trace points (used for `WinSwitchProgramRegister`). |
| S7 | IBM Toolkit 4.5 *Control Program Programming Guide and Reference* (Warp 4 edition; no 4.5 large-file calls in it) | https://www.os2.kr/komh/os2books/os2tk45/cp1/ | Warp 4 documented CP set: adds `DosQueryThreadContext`, `DosPerfSysCall`, also documents `DosAllocThreadLocalMemory`, `DosSetExtLIBPATH`, `DosTmrQueryFreq/Time`, `DosSuppressPopUps`. |
| S8 | IBM Toolkit 4.5 *PM Programming Guide and Reference*, *GPI Guide and Reference*, *WPS Programming Reference* (TOCs) | https://www.os2.kr/komh/os2books/os2tk45/ (pm1–pm5, gpi1–gpi4, wps1–wps3) | Warp 4 documented PM set. |
| S9 | IBM *Programming Guide and Reference Addendum* (OS/2 Warp Server for e-business = 4.5) | https://www.os2.kr/komh/os2books/os2tk45/addendum/ | 4.5 ("later") additions: `DosOpenL`, `DosSet/ProtectSetFile*L`, `DosCancelLockRequestL`, `DosListIO(L)`, `DosCreateThread2`, `WinWaitForShell`; states `WinRestartWorkplace` "is applicable to OS/2 Warp 4, or higher". |
| S10 | IBM Toolkit 4.5 *Using Your Toolkit*, "What's New / New APIs in OS/2 Warp Server for e-business" | https://www.os2.kr/komh/os2books/os2tk45/usetlkt/025_L3_NewAPIsinOS2WarpServ.html | Confirms the 4.5 additions are control-program (LVM, performance, RAS) calls, documented in S9. |
| S11 | EDM/2, *Introducing the Developer's Toolkit for OS/2 Warp Version 4* | https://www.edm2.com/index.php/Introducing_the_Developer%27s_Toolkit_for_OS/2_Warp_Version_4 | New in the Warp 4 toolkit: Universal Language Support (UCONV), TCP/IP, OpenGL, MPTS, problem determination headers. |
| S13 | EDM/2, *GPI Functions* index, section "Not Documented / To Verify" | https://www.edm2.com/index.php/GPI_API | Lists `GpiOval`, `GpiAbortPath`, `GpiQueryPath`, `Gpi*MitreLimit`, `GpiCreate{RoundRect,Elliptic,Polygon}Region`, `Gpi*FontMappingFlags`, `GpiQueryTabbedTextExtent`, `GpiTabbedCharStringAt`, `GpiQueryRasterizerCaps`, `GpiResizePalette`, `Gpi{Suspend,Resume}Play`, ink paths as undocumented. No version data. |
| S14 | EDM/2, *DosQuerySysState* | https://www.edm2.com/index.php/DosQuerySysState | Notes behaviour "prior to Warp 3.0 Fixpack 18": the call predates Warp 3 FP18. |
| S15 | Local: `doc/os2_ordinals.md` (Open Watcom `os2386.lib`) | doc/os2_ordinals.md | Name and ordinal universe of `api_versions.tsv`. The library is 4.5-era (it has the 4.5 large-file calls). |
| S16 | Local: `targets/os2api.def` | targets/os2api.def | Warpine's names and ordinals; `gen_compat` matches `other_api_versions.tsv` against it by name. Not a source of dates. |
| S17 | OS/2 Museum, *OS/2 Warp* | https://www.os2museum.com/wp/os2-history/os2-warp/ | Warp 3: "improved multimedia"; CD 1 has the OS, drivers and multimedia samples; TCP/IP only in the BonusPak (Internet Connection) or Warp Connect. Weak support for MMPM/2 (MDM) being base in 3.0. |
| S18 | OS/2 Museum, *OS/2 Warp 4* and MIT OS/2 FAQ (networking) | https://www.os2museum.com/wp/os2-history/os2-warp-4/ , https://web.mit.edu/os2/www/faq/os2faq0308.html | Warp 4 ships networking with TCP/IP 4.0; Warp 3's Internet Connection is a dial-up subset. |
| H1 | Ordinal-chronology heuristic | — | IBM assigned ordinals in increasing order over releases. An undocumented export whose ordinal sits inside a range fully documented in an earlier release is put in that release. Always marked `H1` in `source_id`. |

`api_versions.tsv` has an extra 6th column `note` with the reason for each line.
`excluded.tsv` lists what was dropped and why.

## Per-module method and confidence

General rule, in order: listed new in 2.1 (S4) → 2.0 bucket; found in the 2.0
reference (S1/S2) → 2.0; first found in a Warp 3 source (S5/S6) → 3.0; first
documented in the Toolkit 4 references (S7/S8) → 4.0; documented only in the
4.5 Addendum (S9) → later; otherwise H1 or `unknown`. Name matching ran over
the OCR text with I/l/1 and O/0 folded; every 2.0 hit that relied on that
folding was reviewed (all were OCR damage, except four 4.5 `*L` names that
matched by prefix and were corrected).

- **DOSCALLS (210)** — High confidence. 2.0 from S1 (141 documented) plus the
  12 `DosProtect*` calls of 2.1 (S4). The Warp 3 ordinal table (S6) is a
  complete export list for Warp 3, so 3.0 is solid for ordinals that first
  appear there (change notification 440–442, thread local memory 454/455,
  `DosVerifyPidTid`, `DosForceSystemDump`, `DosSetExtLIBPATH`/
  `DosQueryExtLIBPATH` 873/874, `DosSysCtl`, the `DosPM*Sem` helpers,
  `DosInitializePorthole`, `DosQueryHeaderInfo`, the RAS calls 112–114 that
  S6 dates to 2.11/Warp). Assumption: an undocumented entry of the Warp 3 table
  inside the 2.0-documented block 209–429 existed in 2.0 (H1; 20 names, e.g.
  `DosQuerySysState`, `DosTmrQueryFreq/Time`, `DosQueryModFromEIP`,
  `DosReplaceModule`, `DosQueryABIOSSupport`, perf counters, `DosTIB`,
  `DosFlatToSel`). Caveat: the Warp 3 table was printed in Feb 1996, so some
  3.0 lines (notably the extended LIBPATH pair) arrived by fixpak, not in the
  Oct 1994 GA; Warp 4 is where they were first documented. 4.0: only
  `DosQueryThreadContext`, `DosPerfSysCall` (S7) and `DosSGQueryTopmost` (H1).
- **NLS, QUECALLS, SESMGR, MSG** — High: all in S1.
- **PMWIN (258)** — High for documented names. 3 names first in Warp 3; the
  2.1 pointer/input calls go to 2.x. H1 puts `WinMsgSemWait`,
  `WinMsgMuxSemWait`, `WinGetErasePS`, `WinReleaseErasePS`,
  `WinStretchPointer`, `KbdPacket` in 2.0 (ordinals below 977, inside the
  2.0-documented block). `WinSetClipRegion`/`WinQueryClipRegion` (1043/1044)
  are `unknown` (ordinal after Warp 3's `WinMessageBox2` 1015: 3.0 or 4.0).
- **PMGPI (290)** — High for the 270 documented calls, which are all 2.0.
  Weak spot: the Warp 3 PM Reference has no GPI pages, and 19 GPI exports
  (ordinals 658–730, see S13) are undocumented anywhere → `unknown`.
  `GpiQueryFullFontFileDescs` is 3.0 (named in the Warp 3 PMERR list).
  `GpiSuspendPlay`/`GpiResumePlay`, `DevStdOpen` are 2.0 by H1.
- **PMDRAG (42)** — High. 2.0 for 32–66, Warp 3 lazy drag 67–74;
  `DrgQueryFormat` (75) `unknown`.
- **PMSHAPI (30)** — High for `Prf*` profile and switch-list calls (2.0).
  The 1.x program-list calls (`PrfAddProgram` …) are H1 2.0.
  `WinSwitchProgramRegister` is 3.0 from a Warp 3 trace point (S6 Vol. 3).
- **PMSPL (48)** — Medium. 2.0 documented queue/job/device calls; 1.x
  `SplStd*`/`Prt*`, `SplInit`, `SplQueryJobId`, `SplQmSetup` 2.0 by H1;
  `SplMessageBox` 3.0.
- **HELPMGR (21)** — High: all in S2.
- **PMWP (34 functions)** — Medium-high. 2.0 from S2, 2.1 lockup pair, 3.0
  from S5, `WinRestartWorkplace` 4.0 (S9), `WinWaitForShell` later (S9),
  `WinNotebookButtonFromID` later (H1, low). The 178 SOM class-data exports
  (`WP*ClassData`, `M_WP*NewClass` …) are excluded: they are the WPS class
  library, not API functions.
- **Modules outside the library** — see
  [Modules outside os2386.lib](#modules-outside-os2386lib-other_api_versionstsv).
  Not covered at all: Open32 (PMWINX), OpenGL, DIVE, MMIO, MCIAPI, LIBUNI,
  TCPIP32 and BDCALLS.


## Modules of `os2_ordinals.md` (full export lists)

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total |
|---|---:|---:|---:|---:|---:|---:|---:|
| DOSCALLS | 173 | 194 | 197 | 197 | 13 | 0 | 210 |
| HELPMGR | 21 | 21 | 21 | 21 | 0 | 0 | 21 |
| NLS | 4 | 4 | 4 | 4 | 0 | 0 | 4 |
| PMDRAG | 33 | 41 | 41 | 42 | 0 | 1 | 42 |
| PMGPI | 270 | 271 | 271 | 290 | 0 | 19 | 290 |
| PMSHAPI | 29 | 30 | 30 | 30 | 0 | 0 | 30 |
| PMSPL | 47 | 48 | 48 | 48 | 0 | 0 | 48 |
| PMWIN | 253 | 256 | 256 | 258 | 0 | 2 | 258 |
| PMWP | 20 | 31 | 32 | 32 | 2 | 0 | 34 |
| QUECALLS | 8 | 8 | 8 | 8 | 0 | 0 | 8 |
| SESMGR | 4 | 4 | 4 | 4 | 0 | 0 | 4 |
| **Total** | **862** | **908** | **912** | 934 | 15 | 22 | 949 |

The other modules' counts are in their sections below.

For the chart I suggest bars of 862 / 908 / 912 for 2.0 / 3.0 / 4.0 from the
main file, with the 22 `unknown` names shown as a hatched range on top of the
4.0 bar (they are all in the 4.5-era import library; most are probably 3.0
or 4.0). The 15 `later` names (4.5 large-file I/O, `DosListIO`,
`DosCreateThread2`, `WinWaitForShell` …) are outside the 4.0 set.

## What the numbers say

The import-library view of OS/2 barely grows after 2.0: Warp 3 adds 46 entry
points (lazy drag, Warp 3 WPS object calls, DSOM restart calls, thread-local
memory, change notification, RAS calls) and Warp 4 adds 4. Warp 4's new APIs
live in **new DLLs**: UCONV (Unicode), TCP/IP 4.0 sockets, OpenGL, Open32
(PMWINX), VoiceType, Java. OS2386.LIB does not cover them, so the chart adds UCONV, the socket DLLs,
MMPM/2's MDM, REXX, SOM and the 16-bit VIO / KBD / MOU from
`other_api_versions.tsv`.

## Weakest parts of the data

1. **No Warp 3 Control Program Reference was found online.** DOSCALLS 3.0
   rests on the Redbook's Warp 3 ordinal table (Feb 1996, Warp 3 with fixpaks).
   It proves the calls existed in a serviced Warp 3. Some (the extended
   LIBPATH pair 873/874, maybe `DosSysCtl`, `DosVerifyPidTid`) probably came
   by fixpak after the Oct 1994 GA.
2. **No Warp 4 PM Reference edition separate from Toolkit 4.5.** "4.0" for PM
   means "documented in the Toolkit 4.5 PM/GPI books and not in a Warp 3 or
   4.5-Addendum source". Only `WinRestartWorkplace` has an explicit "Warp 4 or
   higher" statement.
3. **GPI after 2.0 is close to blind.** The Warp 3 PM Reference has no GPI
   function pages. 19 undocumented GPI exports (658–730: round/elliptic/polygon
   regions, tabbed text, font mapping flags, ink paths, mitre limit, `GpiOval`,
   `GpiQueryPath`, `GpiAbortPath`, `DevPostEscape`) are `unknown`. They look
   like Pen for OS/2 and Open32 support (Warp 3–4 era), but nothing public
   dates them.
4. **The ordinal heuristic (H1) decides 37 names outright** (`source_id` H1)
   and moves 20 more DOSCALLS names from 3.0 back to 2.0 (`S6;H1`: in the
   Warp 3 table, undocumented in 1992, ordinal inside the 2.0 block, e.g.
   `DosQuerySysState`, `DosTmrQueryFreq`, `DosQueryModFromEIP`, perf counters,
   `DosTIB`). All are undocumented compatibility or internal exports (1.x
   spooler `SplStd*`/`Prt*`, 1.x `Prf*Program*`, `WinMsgSemWait` …). They stay in
   2.0 only on the assumption that IBM numbered ordinals in release order.
   `WinStretchPointer` is the clearest conflict: ordinal 968 (2.x block) but
   first documented in Toolkit 4.
5. **"Exists" vs "documented".** 2.0 counts only names in the 1992 manuals
   or provably in the 2.0 ordinal block. An undocumented export is still
   counted, because a program could import it; drop the `H1` lines if the
   chart should mean "documented public API".
6. **Universe limits.** `os2_ordinals.md` is one import library. The modules
   outside it come from their own documented lists (below), whose dating is
   weaker: MDM 3.0 and sockets 4.0 date their entry into the base product
   (MMPM/2 was an add-on for 2.1, TCP/IP for Warp 3).
7. **OCR matching.** Presence in the 1992/1994 scans was found by text search
   with I/l/1 and O/0 folded. Each 2.0 hit that relied on folding was
   reviewed. A missed OCR match would wrongly move a name later, but no
   documented 2.0 name is known to be affected.


## Modules outside os2386.lib (`other_api_versions.tsv`)

Researched on 2026-11-24 with the same rules: public documentation only, every line with its source, H1 marked. The four groups follow, each with its new sources, method, counts and weak spots.

### VIOCALLS / KBDCALLS / MOUCALLS: sources and dating

Universe: the `## VIOCALLS` (63), `## KBDCALLS` (20) and `## MOUCALLS` (27)
sections of `doc/os2_ordinals.md` (110 exports, all 16-bit `VIO16*` / `KBD16*` /
`MOU16*`). Each is counted once under the 32-bit toolkit name that maps to it
(`#define VioWrtTTY VIO16WRTTTY` in Open Watcom `bsesub.h` / `pmavio.h`, S24)
at its `os2_ordinals.md` ordinal. Name case is Open Watcom's (`KbdGetHWID`,
`VioGetOrg`); where Open Watcom has no 32-bit name, Warpine's SDK (S25) gives
it (`MouGetHotKey`, `MouSetHotKey`, `VioCheckCharType`). 104 lines in
`other_api_versions.tsv`, 6 in `excluded.tsv`.

#### (a) New source rows

| id | Title | Location | What it gives us |
|----|-------|----------|------------------|
| S20 | Microsoft *MS OS/2 Programmer's Reference* Vol. 3, Version 1.1, 1989 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_1.x/Microsoft_-_OS2_Programmers_Reference_Volume_3_Version_1.1_1989.pdf | Function pages for the 1.1 Kbd / Mou / Vio calls, AVIO (`VioCreatePS` … `VioShowPS`) included: 96 of the 110 exports have a page. `MouGetHotKey` / `MouSetHotKey` are only named (codes 9 and 10 of the `MouRegister` function list). OCR text searched for every name. |
| S21 | Microsoft *MS OS/2 Programmer's Reference* Vol. 4, Version 1.2, 1989 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_1.x/Microsoft_-_OS2_Programmers_Reference_Volume_4_Version_1.2_1989.pdf | 1.2 changes: `KbdGetHWID` marked "New"; no other new Kbd / Mou / Vio call. |
| S22 | EDM/2 wiki function pages (`VioGlobalReg`, `VioCheckCharType`, `KbdSetHWID`, `VioGetPSAddress`, `MouGetHotKey`, `MouSetHotKey`, `MouGetThreshold`) | https://www.edm2.com/index.php/VioGlobalReg (same pattern per name) | Syntax and purpose; no version data. `VioGetPSAddress` is in the "Undocumented" category; `KbdSetHWID` is shown as a legacy page with a thunk sample. No page for the `XVio*` names. |
| S23 | IBM *OS/2 2.0 Presentation Manager Programming Reference* Vol. 3 (S10G-6272-00), March 1992 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_2.x/IBM_OS2_2.0_Technical_Library_1992/S10G-6272-00_OS2_2.0_Presentation_Manager_Programming_Reference_Volume_3_199203.pdf | 2.0 PM has advanced VIO presentation spaces ("Default AVIO window procedure"; WM_SIZE for a window showing an advanced VIO PS). No Vio* function pages. |
| S24 | Local: Open Watcom headers `vendor/watcom/h/os2/bsesub.h`, `pmavio.h`, `bseord.h` | vendor/watcom/h/os2/ | 32-bit toolkit names and their `VIO16*` / `KBD16*` / `MOU16*` targets; `ORD_*` ordinals (match `os2_ordinals.md`). `XVio*`, `VioGetPSAddress`, `MouGet/SetHotKey` have only `ORD_*` lines; `VioCheckCharType` only `Vio16CheckCharType`. |
| S25 | Local: Warpine SDK `sdk/include/os2/sub.h` | sdk/include/os2/sub.h | 32-bit names `MouGetHotKey`, `MouSetHotKey`, `VioCheckCharType`. |

Reused: **S1** (2.0 CP Reference) has no Kbd / Mou / Vio function pages, but
shows the 16-bit subsystems callable in 2.0 from 32-bit code: the `DosExitList`
example (a 32-bit `APIENTRY` routine) calls `VioWrtTTY`; `DosExecPgm` says a
detached process "cannot make any VIO, KBD, or MOU calls, except from within a
video pop-up requested by VioPopUp" and to test with `VioGetAnsi`; error texts
name `VioDeRegister`, `KbdRegister`, `KbdDeRegister`. **S6** Vol. 3 (*System
Trace Reference*, Feb 1996, Warp 3) lists OS2CHAR.DLL trace points (major code
0x0018) for every Vio / Kbd / Mou API of Warp 3 by name: all 1.x names plus
`VioGetPSAddress` (0x0013), `VioGlobalReg` (0x0015), `MouGetThreshold`
(0x020D), `MouSetThreshold` (0x0218); none for `XVio*`, `VioCheckCharType`,
`KbdSetHWID`, `MouGetHotKey`, `MouSetHotKey`. **S7** (Toolkit 4.5 CP Guide and
Reference) documents the application subset: 15 Kbd, 19 Mou (with the
threshold pair), 48 Vio pages (`VioGetOrg` / `VioSetOrg` there named
`VioGetOrigin` / `VioSetOrigin`). The S2 Vol. 1/2 OCR has none of the names.

Note on S20/S21: they are Microsoft's, not IBM's, editions of the OS/2 1.x
reference (the API was jointly specified); public scans on bitsavers, cited
only for the existence of a name in 1.1 / 1.2.

#### (b) Per-module method and confidence

- **VIOCALLS, KBDCALLS, MOUCALLS (104 counted, 6 excluded)** — High for the
  1.x set. Rule: a name with a function page in the OS/2 1.1 / 1.2 reference
  (S20/S21) is 2.0, because 2.0 keeps the 16-bit subsystems and lets 32-bit
  code call them (S1; AVIO also S23); the Warp 3 trace table (S6 Vol. 3)
  confirms every one of them by name except the hot-key pair. 97 names have
  pages (96 in 1.1, `KbdGetHWID` in 1.2); `MouGetHotKey` / `MouSetHotKey` are 2.0 because S20 names them as Mou
  functions (`MouRegister` codes 9/10) and their ordinals (4, 10) sit inside
  the 1.1 block. Names absent from 1.x but present in the Warp 3 trace table
  are 3.0 (S6): `VioGlobalReg` (70), `MouGetThreshold` / `MouSetThreshold`
  (29/30, ordinals above the 1.1 block, so H1 cannot move them). `unknown`:
  `KbdSetHWID` (25; toolkit name only, no IBM page, no Warp 3 trace point) and
  `VioCheckCharType` (75; DBCS, undated EDM/2 page). Excluded: the five
  `XVio*` DBCS conversion-area calls and `VioGetPSAddress` — no 32-bit name in
  any toolkit header, undocumented internals (the IME front end and PM's AVIO).
  `VioGetPSAddress` would be 3.0 (S6) if counted.

#### (c) Per-version counts

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total | excluded |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| KBDCALLS | 19 | 19 | 19 | 20 | 0 | 1 | 20 | 0 |
| MOUCALLS | 25 | 27 | 27 | 27 | 0 | 0 | 27 | 0 |
| VIOCALLS | 55 | 56 | 56 | 57 | 0 | 1 | 57 | 6 |
| **Total** | **99** | **102** | **102** | 104 | 0 | 2 | 104 | 6 |

#### (d) Weak spots

1. **No IBM 2.0 document has Kbd / Mou / Vio function pages.** The 1992
   Technical Library covers the 32-bit API only; S1 only mentions them.
   2.0 rests on the 1.1 / 1.2 references (Microsoft editions) plus S1's
   evidence that 2.0 exposes the subsystems to 32-bit code. That 2.0 kept every
   1.x export is an inference (binary compatibility with 1.x programs), borne
   out by the Warp 3 trace table listing them all.
2. **No IBM OS/2 1.3 reference was found online.** `VioGlobalReg`,
   `MouGetThreshold`, `MouSetThreshold` may date from 1.3 or 2.0 (they are
   1.x-style 16-bit calls); they are 3.0 here only because the Warp 3 trace
   table is their earliest public trace. Same caveat as S6 for DOSCALLS: the
   table is Warp 3 with fixpaks (Feb 1996).
3. **`MouGetHotKey` / `MouSetHotKey`** have no function page anywhere public
   (only the 1.1 `MouRegister` code list and an empty EDM/2 stub) and no Warp 3
   trace point; 2.0 is a medium-confidence call.
4. **DBCS calls** (`VioCheckCharType`, `XVio*`) have no dated public source;
   they probably come from the DBCS (Japanese / Korean / Chinese) 1.x or 2.x
   editions. Excluding `XVio*` and counting `VioCheckCharType` as `unknown`
   follows the toolkit-name rule, not evidence of age.
5. **Name choice.** Toolkit 4 renamed `VioGetOrg` / `VioSetOrg` to
   `VioGetOrigin` / `VioSetOrigin` in its documentation; the table keeps Open
   Watcom's names. The Warp 3 trace table spells `VioDeleteSetID`,
   `VioQuerySetIDs`, `KbdGetCP`, `KbdSetCP`, `KbdSetCustXT` (same calls).

### REXX / REXXAPI: sources, method, counts

Universe: `## REXX` (11 exports) and `## REXXAPI` (48 exports) of
`doc/os2_ordinals.md` (S15). 59 exports: 34 dated in `other_api_versions.tsv`, 25 excluded in
`excluded.tsv`. No IBM binary was examined.

#### (a) New source rows

| id | Title | Location | What it gives us |
|----|-------|----------|------------------|
| S30 | IBM *OS/2 2.0 Procedures Language 2/REXX Reference*, S10G-6268-00, March 1992 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_2.x/IBM_OS2_2.0_Technical_Library_1992/S10G-6268-00_OS2_2.0_Procedures_Language_2_REXX_Reference_199203.pdf | Ch. 9 "Application Programming Interface" documents 24 calls: `RexxStart`, `RexxVariablePool`, `RexxSetHalt`, `RexxSetTrace`, `RexxResetTrace`, the subcommand, external-function and system-exit `Register*Dll/Exe` / `Deregister*` / `Query*` calls, and all 7 macrospace calls (`RexxAddMacro` … `RexxReorderMacro`). No queue API, no `RexxLoadSubcom` / `RexxCall*` / `RexxExecuteMacroFunction`, no `Rx*` names. It does document the `RXQUEUE()` built-in and the `RXQUEUE` filter (named queues shared between sessions). The companion *User's Guide* S10G-6269-00 (same directory) names no API call. |
| S31 | IBM Toolkit 4.5 *Object REXX Programming Guide for OS/2*, appendix "REXX Application Programming Interfaces" | https://www.os2.kr/komh/os2books/os2tk45/rexxpg/153_L1_REXXApplicationProgr.html (queue calls: …/rexxpg/289_L3_QueueInterfaceFuncti.html) | The same 24 calls plus the "Queue Interface": `RexxCreateQueue`, `RexxDeleteQueue`, `RexxQueryQueue`, `RexxAddQueue`, `RexxPullQueue`. First public document found for the queue calls. No `Rx*` names, no `RexxLoadSubcom` / `RexxCall*` / `RexxExecuteMacroFunction`. |
| S32 | Local: Open Watcom `lib386/os2/rexx.lib` (listed with Open Watcom `wlib -l`) and `h/os2/rexxsaa.h` | vendor/watcom/lib386/os2/rexx.lib, vendor/watcom/h/os2/rexxsaa.h | Export names in their real case: all 34 `Rexx*` names are mixed case (32-bit `_System` exports), the 25 others are upper case (`REXXSAA`, `RXVAR`, `RXSUBCOMREGISTER` …), the Pascal-convention naming of the OS/2 1.x API. The 32-bit `rexxsaa.h` declares only the 24 S30 calls (not even the queue calls). |
| S33 | IBM Toolkit 4.5 `rxstring.txt` (RXSTRING.LIB readme) | https://www.os2.kr/komh/os2books/os2tk45/rxstring.txt | "The functions provided for OS/2 2.0 are 32-bit only … For 16-bit RXSTRING functions, use the 1.2 or 1.3 OS/2 Toolkit": a 16-bit REXX programming interface existed in the OS/2 1.2/1.3 toolkits, which is where the upper-case `Rx*` exports come from. |
| S34 | EDM/2, *REXX API* | https://www.edm2.com/index.php/REXX_API | Function index: the same 24 calls as S30; no queue calls, no `Rx*` names. No version data. |

(Checked and not useful: Redbooks GG24-4199-00 *OS/2 REXX: From Bark to Byte*
(Dec 1993, archive.org `GG2441990`) and SG24-4586 *Object REXX for OS/2* (Jul
1996, archive.org `Sg244586`) name `RexxStart` / macrospace calls only;
Debugging Handbook Vols. 3/4 (S6) have no REXX entries; the OS/2 2.0 CP
Reference and Programming Guides name no REXX call. A third-party book, *OS/2
2.1 REXX Handbook* (VNR, archive.org `0-442-01734-0`), uses `RxSubcomQuery`,
`RxFunctionQuery` and "Variable Pool Interface (RxVar)" as C calls, which agrees
with the `Rx*` names being the older interface; it is not an IBM source and is
not cited in the tables.)

#### (b) Per-module method and confidence

- **REXX (5 of 11), REXXAPI (29 of 48)** — High for the 24 calls documented in
  the 1992 REXX Reference (S30, all 2.0; all still in S31). Medium for the
  queue calls `RexxCreateQueue` … `RexxQueryQueue` (REXXAPI 1–5): first
  documented in Toolkit 4.5 (S31), but they hold the lowest ordinals of
  REXXAPI, below the 2.0-documented block 6–28, and the 2.0 `RXQUEUE()`
  built-in and `RXQUEUE` filter already manage named cross-session queues, so
  they are 2.0 by H1 (`S31;H1`). Medium-low for the undocumented
  `RexxLoadSubcom` (10), `RexxCallExit` (15), `RexxCallSubcom` (16),
  `RexxCallFunction` (21): 2.0 by H1 (inside block 6–28); three of them pair by
  name with 1.x calls (`RXSUBCOMLOAD`, `RXSUBCOMEXECUTE`, `RXFUNCTIONCALL`), so
  they look like the 32-bit forms of those, not later additions.
  `RexxExecuteMacroFunction` (29) is `unknown`: undocumented, and its ordinal
  sits just after the 2.0 block, so H1 does not reach it (probably 2.0).
  The 25 upper-case names (`REXXSAA`, `RXTRACESET`, `RXVAR`, `RXBREAKCLEANUP`,
  `RXHALTSET`, `RXTRACERESET`, REXXAPI 30–48 `RXSUBCOM*` / `RXFUNCTION*` /
  `RXEXIT*` / `RXMACRO*`) are excluded as 16-bit OS/2 1.x-era entry points:
  Pascal upper-case export names (S32), a 16-bit REXX interface in the 1.2/1.3
  toolkits (S33), named by no 2.0-or-later IBM document (S30, S31) and declared
  by no 32-bit header (S32). Each has a 32-bit `Rexx*` counterpart, except
  `RXBREAKCLEANUP`. A 32-bit linker can bind them (they are in `lib386`'s
  `rexx.lib`), but nothing public gives a 32-bit calling form, the same reason
  `DOS16*` / `VIO16*` were left out of the main table.

#### (c) Per-version counts

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total dated | excluded |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| REXX | 5 | 5 | 5 | 5 | 0 | 0 | 5 | 6 |
| REXXAPI | 28 | 28 | 28 | 29 | 0 | 1 | 29 | 19 |
| **Total** | **33** | **33** | **33** | 34 | 0 | 1 | 34 | 25 |

Of the 33 in 2.0: 24 documented (S30), 5 queue calls `S31;H1`, 4 `H1` only.
If the chart should mean "documented public API in 2.0", the 2.0 bar is 24 and
the queue calls move to `unknown` (first documented 4.5-era toolkit).

#### (d) Weak spots

1. **Queue calls (5).** No Warp 3 or Warp 4 classic-REXX reference (the
   toolkit's *REXX Program Reference* INF) was found online; the first public
   document is the Toolkit 4.5 Object REXX guide (S31). The 2.0 date rests on
   H1 (ordinals 1–5 below the 2.0 block) and on the 2.0 `RXQUEUE` facilities;
   Open Watcom's 32-bit `rexxsaa.h` lacks them, so the header it copies may
   predate their documentation.
2. **Four undocumented helpers** (`RexxLoadSubcom`, `RexxCallExit`,
   `RexxCallSubcom`, `RexxCallFunction`) are 2.0 only by H1; they are probably
   interpreter-side helpers that applications were never meant to call. Drop
   them if the chart counts only documented API. `RexxExecuteMacroFunction`
   is `unknown`.
3. **The upper-case 1.x names (25)** are excluded on naming convention plus
   the 1.x-toolkit note (S33); no IBM document of their 16-bit signatures was
   found (no OS/2 1.2/1.3 REXX reference is on bitsavers). If the caller wants
   every importable name counted, they would be `unknown` (or 2.0 under a
   looser H1: they must have existed in 2.0 for 1.x REXX programs to run), but
   they are not a 32-bit API.
4. **Ordinals** come only from Open Watcom's import library (S15/S32); no IBM
   document lists REXX/REXXAPI ordinals.
5. **Warp 3 / Warp 4 changes.** Nothing public shows a REXX API added in Warp 3
   or Warp 4 (classic REXX); Object REXX (Warp 4 option) documents the same
   set plus queues, so 3.0 and 4.0 add nothing.

### SOM kernel (SOM.DLL): sources, method, counts

Compiled 2026-11-24 from public documentation only. No IBM binary was examined;
the IBM PDFs in `doc/references/` were cited, not copied. Files: `other_api_versions.tsv`
(77 names), `excluded.tsv`.

#### (a) New source rows

| id | Title | Location | What it gives us |
|----|-------|----------|------------------|
| S60 | IBM *OS/2 2.0 Technical Library: System Object Model Guide and Reference*, S10G-6309-00, First Edition, December 1991 | https://archive.org/details/os2-2.0-som-1991 | SOM 1.0 as shipped with OS/2 2.0. Ch. 3 documents the functions `somEnvironmentNew`, `somPrintf`, `somResolve`, `somRegisterId`, `somUniqueKey`, `somTotalRegIds`, `somSetExpectedIds`, `somBegin/EndPersistentIds`; the data `SOMClassMgrObject`, `SOM_TraceLevel/WarnLevel/AssertLevel`; the macros `SOM_CheckId`, `SOM_IdFromString`, `SOM_CompareIds`, `SOM_StringFromId`, `SOM_ParentResolve`, `SOM_Assert`, `SOM_Test`; the `<className>ClassData` / `<className>NewClass` conventions. Ch. 4 the nine replaceable entry points (`SOMCalloc` … `SOMRealloc`). Ch. 5 the kernel classes `SOMObject`, `SOMClass`, `SOMClassMgr`. OCR text searched with I/l/1 and O/0 folded and spaces removed. |
| S61 | osFree, `SOM/som/makefile`, `EXPORTS` list (ordinals 1–47) | https://github.com/osfree-project/osfree/blob/master/SOM/som/makefile | The 47 SOM.DLL names and ordinals Warpine already uses (`targets/os2api.def` SOM.1–47). 1–19 data in alphabetical order, 20–22 the three `NewClass` functions, 23–46 functions in alphabetical order, 47 `somDataResolve` appended. Not verified against IBM's `som.lib`. |
| S62 | Bull DPX/20 *SOMobjects Base Toolkit Quick Reference Guide* (AIX), 86 A2 29AQ 01, June 1995 — abstracted from IBM's *SOMobjects Base Toolkit Programmer's Reference Manual*, Version 2.0 | https://archive.org/details/manualzz-id-40748 | The complete SOM 2.0 kernel function list ("every method, function, and macro", SOM Kernel Quick Reference, pp. 2-1…2-4). Platform-neutral SOM 2.0 API (it mentions OS/2 DLLs); lacks `somDataResolveChk`, `somVaBuf_*`, `somvalist*`. |
| S63 | EDM/2, Cathy Bloomfield, *The Enhanced Workplace Shell Programming Interface in OS/2 Warp Version 3* | https://www.edm2.com/index.php/The_Enhanced_Workplace_Shell_Programming_Interface_in_OS/2_Warp_Version_3 | "We used the IBM System Object Model (SOM) 2 when developing the OS/2 Warp Version 3 Workplace Shell"; Warp 3's toolkit uses IDL. Puts SOM 2.0 in base Warp 3. |
| S64 | IBM OS/2 Toolkit (Warp 4 / Toolkit 4.5) *System Object Model Programming Reference* | https://www.os2.kr/komh/os2books/os2tk45/somref/ (local copy `doc/references/os2toolkit/`, from https://archive.org/details/ToolkitDocs1) | SOM 2.1 "SOM Kernel Reference": the full documented OS/2 kernel set, including the 2.1 additions `somDataResolveChk`, `somVaBuf_add/create/destroy/get_valist`, `somvalistGetTarget/SetTarget`. |
| S65 | IBM OS/2 Toolkit (Warp 4 / Toolkit 4.5) *System Object Model Programming Guide* | https://www.os2.kr/komh/os2books/os2tk45/somguide/ (local copy as S64) | "What's New in SOMobjects Version 2.1" (DTS C++ API enhancements …); "Customizing Mutual Exclusion Services" (`SOMCreateMutexSem`, `SOMRequestMutexSem`, `SOMReleaseMutexSem`, `SOMDestroyMutexSem`, `SOMGetThreadId`) and "Customizing Multi-threading Services" (`SOMStartThread`, `SOMEndThread`, `SOMKillThread`, `SOMGetThreadHandle`). |
| S66 | IBM *SOMobjects Developer's Toolkit 3.0 Programmer's Reference, Volume I: SOM and DSOM*, 2nd ed., 1996 | https://archive.org/details/SOMobjectsDevelopersToolkitBooks/ (local `doc/references/somobjects30/`) | Cross-check: its kernel function list adds nothing to S64 (no SOM 3.0-only kernel exports to put in `later`). |
| S67 | IBM Redbook *OS/2 Version 2.0 Volume 4: Writing Applications*, GG24-3774-00, Jan 1993 | https://bitsavers.org/pdf/ibm/pc/os2/OS2_2.x/redbooks/GG24-3774-00_OS2_V2.0_Vol_4_Writing_Applications_199301.pdf | Corroborates 2.x use of `somPrintf`, `SOMOutCharRoutine`, `SOMClassMgrObject`, `SOM_TraceLevel`, `SOM_WarnLevel` in WPS code. |

H1 is reused in a SOM-specific form: osFree's SOM 1.0 export list (S61) is in
alphabetical blocks (1–19 data, 20–22 `NewClass`, 23–46 functions), which
reads as one original release; `somDataResolve` (47) is appended after the
block and is not in S60, so it is treated as a later addition. A name inside
the alphabetical blocks that S60 does not document is put in 2.0 as `S61;H1`.

#### (b) Per-module method and confidence

- **SOM (77: SOM.DLL kernel functions and exported data)** — Medium-high.
  Universe: the S64 "SOM Kernel Reference" functions and replaceable entry
  points, the S65 mutex and thread hooks, the class-data / `NewClass` exports
  of `SOMObject`, `SOMClass`, `SOMClassMgr`, `SOMClassMgrObject`, the three
  debug-level variables, and the undocumented SOM 1.0 exports of S61.
  2.0 (46): 28 documented in the Dec 1991 OS/2 2.0 SOM Guide and Reference
  (S60); 18 by S61's alphabetical SOM 1.0 export block (`S61;H1`): the seven
  functions behind S60's `SOM_CheckId`/`SOM_IdFromString`/`SOM_CompareIds`/
  `SOM_StringFromId`/`SOM_ParentResolve`/`SOM_Assert`/`SOM_Test` macros, the
  three `*CClassData`, `somLPrintf`/`somPrefixLevel`/`somVprintf`, and five
  undocumented exports (`somCheckArgs`, `somConstructClass`, `somSaveMetrics`,
  `somTestCls`, `somWriteMetrics`). 3.0 (15): the SOM 2.0 kernel functions of
  the SOM 2.0 Quick Reference (S62) that are not in S60 / S61's 1.0 block
  (`somBuildClass`, `somApply`, `somResolveByName`, `somClassResolve`,
  `somParentNumResolve`, `somDataResolve`, the CORBA `Environment` /
  exception calls, `somMainProgram`, `somEnvironmentEnd`, `somIsObj`,
  `somSetOutChar`), dated to Warp 3 because the Warp 3 WPS was built on SOM 2
  (S63). 4.0 (16): the SOM 2.1 additions first documented for OS/2 in the
  Warp 4 Toolkit (S64/S65): `somDataResolveChk`, the six va_list helpers, the
  nine mutex/thread hook variables. SOMobjects 3.0 (S66) adds no kernel
  export, so nothing is `later`. Ordinals 1–47 are osFree's (S61), identical to
  `targets/os2api.def`; every other ordinal is `-` (no public IBM source).
  Macros, `SOMInitModule` (exported by class DLLs), methods, DSOM, and
  Warpine's own `Wrp*` entries are in `excluded.tsv`.

#### (c) Per-version counts

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total |
|---|---:|---:|---:|---:|---:|---:|---:|
| SOM | 46 | 61 | 77 | 77 | 0 | 0 | 77 |

Of the 2.0 lines, 28 rest on S60 and 18 on `S61;H1`. Warpine today maps 61 of
the 77 names (all but `somApply`, the six va_list helpers and the nine
mutex/thread hooks).

#### (d) Weak spots

1. **SOM 1.0 export list is osFree's, not IBM's.** The 2.0 bucket's 18 `H1`
   names and all ordinals depend on osFree's list (S61), which the project has
   not verified against IBM's `som.lib`. No IBM document lists SOM.DLL ordinals.
2. **The SOM 2.0 function set comes from an AIX edition** (Bull's reprint of
   IBM's SOMobjects Base Toolkit 2.0 Quick Reference, June 1995). The SOM 2.0
   API is platform-neutral, but no OS/2 Warp 3 SOM Reference was found online,
   so "3.0" means "in SOM 2.0, and Warp 3 shipped SOM 2" (S63). Some sources
   (OS/2 history talks) say the WPS of 2.x ran on SOM 1.0 and SOM 2 came with
   3.0, consistent with this; none dates the Warp 3 GA's exact SOM build.
3. **SOM 2.1 vs Warp 3 fixpaks.** SOMobjects 2.1 also shipped as a separate
   toolkit (its Guide speaks of Windows and AIX editions) and may have reached
   Warp 3 by fixpak; no public source dates that; the 16
   4.0 names are dated by their first OS/2 base documentation (Warp 4 Toolkit).
4. **The mutex/thread hooks may predate 2.1.** They are not in S62, but a quick
   reference could omit variables (it does list the other replaceable
   variables, which argues for 2.1). They are also not in S64's kernel
   reference, only in the Guide (S65).
5. **Exporting DLL of the 2.1 helpers is unconfirmed.** `somVaBuf_*` appear in
   osFree only in a commented-out export block next to typecode helpers
   (another DLL), and the thread hooks are declared in `somthrd.h`. They are
   kept in SOM because IBM documents them in the kernel reference/guide; a
   program might import them from a different SOM DLL.
6. **`somSetGlobalEnvironment`** is named once in S64 (a cross-reference) with no
   entry; excluded, but it may be a real export.
7. **OCR.** S60 is a 1991 scan; matches were made with spaces removed and
   I/l/1, O/0 folded, then read in context (e.g. S60's `SOM_CheckId` macro vs
   the `somCheckId` function).

### Group `dlls`: MSG, PMCTLS, UCONV, MDM, SO32DLL, TCP32DLL

Replaces `doc/compat/extra_modules.tsv` (which listed only the names Warpine maps)
with each module's full list of documented application functions.
Compiled 2026-11-24 from public documentation and the local Open Watcom snapshot;
no IBM binary was examined. Import libraries were read by parsing their OMF
IMPDEF records (`scripts/impdef.py`; the module, ordinal and name of each import).

#### (a) New source rows

| id | Title | Location | What it gives us |
|----|-------|----------|------------------|
| S40 | Local: Open Watcom `lib386/os2/mmpm2.lib` (IMPDEF records) | vendor/watcom/lib386/os2/mmpm2.lib | MDM exports with ordinals: `mciSendCommand` 1, `mciSendString` 2, `mciGetErrorString` 3, connection calls 6–9, `mciSetSysValue` 10, `mciQuerySysValue` 11, `mciMakeGroup` 12, `mciDeleteGroup` 13, `mciGetDeviceID` 16, `mdmDriverNotify` 17, 16-bit `MCI16*` 18–20, internal 21–22. Also shows `mciPlayFile` / `mciPlayResource` / `mciRecordAudioFile` are in MCIAPI, not MDM. |
| S41 | Local: Open Watcom `lib386/os2/libconv.lib` (IMPDEF records) and `h/os2/uconv.h` | vendor/watcom/lib386/os2/libconv.lib | UCONV ordinals 1–4, 7–14; `uconv.h` declares the 9 public conversion calls. |
| S42 | Local: Open Watcom `lib386/os2/os2386.lib`, MSG and PMCTLS IMPDEF records | vendor/watcom/lib386/os2/os2386.lib | MSG 1–9 (32-bit 4, 5, 6 = `DOS32TRUEGETMESSAGE`, 8 = `DOS32IQUERYMESSAGECP`) and PMCTLS 2–6, 24. `os2_ordinals.md` (S15) has no PMCTLS section. |
| S43 | IBM Toolkit 4.5 *Multimedia Programming Reference* 1, "MCI Functions" | https://www.os2.kr/komh/os2books/os2tk45/mmref1/ | Documented MCI functions: `mciGetDeviceID`, `mciGetErrorString`, `mciQuerySysValue`, `mciSendCommand`, `mciSendString`, `mciSetSysValue`, `mciPlayFile`, `mciPlayResource`, `mciRecordAudioFile`, `mdmDriverNotify`. |
| S44 | Local: Open Watcom `h/os2/mcios2.h`, `mciapi.h`, `mmdrvos2.h` | vendor/watcom/h/os2/ | Prototypes of the MDM calls, including `mciMakeGroup` / `mciDeleteGroup` (no reference page); `mdmDriverNotify` is in the driver header. |
| S45 | IBM Toolkit 4.5 *Unicode Programming Reference* | https://www.os2.kr/komh/os2books/os2tk45/uniapi.htm | Documents the 9 `uconv.h` calls (`UniCreateUconvObject` … `UniStrToUcs`) beside LIBUNI's locale/character calls. Names no DLL. |
| S46 | IBM *TCP/IP Version 4.21 Programming Reference* (Toolkit 4.5), function chapters and "What Is New" pages | https://www.os2.kr/komh/os2books/os2tk45/tcppr/ (006/007/008_L3_Changesin…) | The "Protocol-Independent C Sockets API" and "Network Utility Routines API" lists; "Changes in Version 4 (Merlin)" lists calls new in TCP/IP 4.0 (`addsockettolist`, `removesocketfromlist`, `dn_find`, `dn_skipname`, `getlong`, `getshort`, `putlong`, `putshort`, `R*` SOCKS calls, `res_query`, `res_querydomain`, `res_search`, `rexec`) and removes `os2_connect` / `os2_gethostbyname`; "Changes in 4.21" adds `accept_and_recv`, `send_file`, `sysctl`, splits `os2_ioctl` / `os2_select`, removes `sock_init`, `bswap`, `lswap`, `tcp_h_errno`. |
| S47 | Same book, "Introduction to Networking Services" | https://www.os2.kr/komh/os2books/os2tk45/tcppr/012_L2_IntroductiontoNetwor.html | "TCP32DLL.DLL and SO32DLL.DLL together export the 32-bit BSD Version 4.3 socket APIs"; TCPIP32.DLL exports the BSD 4.4 API. No per-function DLL split. |
| S48 | Apache Portable Runtime, `network_io/os2/os2calls.c` (Apache-2.0 source) | https://apache.googlesource.com/apr/+/1.2.x/network_io/os2/os2calls.c | Loads `SOCKET`, `SELECT`, `SOCK_ERRNO`, `ACCEPT`, `BIND`, `CONNECT`, `GETPEERNAME`, `GETSOCKNAME`, `GETSOCKOPT`, `IOCTL`, `LISTEN`, `RECV`, `SEND`, `SETSOCKOPT`, `SHUTDOWN`, `SOCLOSE`, `WRITEV`, `SENDTO`, `RECVFROM` from SO32DLL by name: confirms these are SO32DLL exports. |
| S49 | IBM Toolkit 4.5 *PM Programming Guide*, "Creating a Circular Slider" | https://www.os2.kr/komh/os2books/os2tk45/pm5/368_L3_CreatingaCircularSli.html | "This window class must be registered with WinRegisterCircularSlider before you can create a circular slider." |
| S50 | Local: Open Watcom `lib386/os2/tcpip32.lib` (IMPDEF records) | vendor/watcom/lib386/os2/tcpip32.lib | The TCPIP32 (BSD 4.4) export list, 149 names: socket calls at 1–38, `gethostid` at 4 in that block, netdb/resolver from 102, `gethostname` 144. Used only as layout evidence; TCPIP32 itself is out of scope. |

#### (b) Per-module method and confidence

- **Extra modules (`other_api_versions.tsv`, full documented lists)** — Universe: the
  functions the public references document for each DLL, with ordinals from
  Open Watcom's import libraries where one covers the module (MSG, PMCTLS via
  os2386.lib; UCONV via libconv.lib; MDM via mmpm2.lib). SO32DLL / TCP32DLL
  have no public ordinal source (as ADR-0041 already notes) → ordinal `-`.
  - **MSG (4)** — High: the four 32-bit calls are in S1 (2.0). The 16-bit
    entries and the undocumented `DOSCLOSEMESSAGEFILE` are excluded.
  - **PMCTLS (6)** — High for the five standard-dialog calls (S2, 2.0).
    `WinRegisterCircularSlider` 3.0, low-medium: the `WC_CIRCULARSLIDER` class
    is first documented in the Warp 3 PM Reference (S5), and S49 says the class
    must be registered with this call; the function itself is named only in the
    Toolkit 4.5 guide.
  - **UCONV (9)** — Medium: the 9 conversion calls of `uconv.h` documented in
    S45, dated 4.0 (ULS first in the Warp 4 toolkit, S11). LIBUNI's locale and
    character calls are a different DLL and not counted. `UniQueryUconvCp`,
    `_UniMalloc`, `_UniFree` are undocumented exports → excluded.
  - **MDM (8)** — Low for the date, high for the list. 6 documented MCI
    functions (S43) + `mciMakeGroup` / `mciDeleteGroup` (prototypes only, S44;
    H1 between documented ordinals). Dated 3.0 as before (S17): MMPM/2 was an
    add-on for 2.1 and installed from the Warp 3 CD. `mciPlayFile` & co. are
    MCIAPI.DLL; `mdmDriverNotify` is for drivers → excluded.
  - **SO32DLL (28) / TCP32DLL (55)** — Low-medium. List: the 4.21 reference
    (S46) minus its 4.21 additions, plus the calls it says 4.21 removed
    (`sock_init`, `bswap`, `lswap`, `tcp_h_errno`). Module split: no IBM page
    says which of the two DLLs exports a call (S47 says only "together"); the
    Sockets API chapter is put in SO32DLL (19 of its calls confirmed by S48),
    the Network Utility Routines in TCP32DLL. `gethostid` goes to SO32DLL
    (TCPIP32 keeps it in the socket block, S50; H1). All dated 4.0: TCP/IP is
    base from Warp 4 (S18); in Warp 3 it was the BonusPak / Warp Connect
    add-on. The 18 "new in Version 4 (Merlin)" calls are 4.0 with more
    confidence. `htonl` / `htons` / `ntohl` / `ntohs` are `unknown`: in 4.0
    they were probably macros over `lswap` / `bswap`.

#### (c) Per-version counts

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total |
|---|---:|---:|---:|---:|---:|---:|---:|
| MDM | 0 | 8 | 8 | 8 | 0 | 0 | 8 |
| MSG | 4 | 4 | 4 | 4 | 0 | 0 | 4 |
| PMCTLS | 5 | 6 | 6 | 6 | 0 | 0 | 6 |
| SO32DLL | 0 | 0 | 28 | 28 | 0 | 0 | 28 |
| TCP32DLL | 0 | 0 | 51 | 55 | 0 | 4 | 55 |
| UCONV | 0 | 0 | 9 | 9 | 0 | 0 | 9 |
| **Total** | **9** | **18** | **106** | 110 | 0 | 4 | 110 |

Warpine's current names in this universe: MSG 4/4, PMCTLS 6/6, UCONV 5/9,
MDM 2/8 (`mciSendCommand`, `mciSendString`), SO32DLL 20/28, TCP32DLL 6/55.

#### (d) Weak spots

1. **SO32DLL vs TCP32DLL split is by chapter, not by an export list.** No
   public source lists either DLL's exports or ordinals. Warpine maps most
   socket calls in both DLLs and the netdb calls in both; only the copy in the
   chapter's DLL is in the universe (the others are listed in
   `excluded.tsv`). The chart's TCP/IP row sums both DLLs, so only a call's
   presence in its chapter's DLL matters.
2. **Warpine's MDM ordinals 3 and 4 are not OS/2 functions.** `mciFreeBlock`
   and `mciGetLastError` appear in no OS/2 source; Open Watcom's mmpm2.lib puts
   `mciGetErrorString` at MDM.3 and has no MDM.4. A program importing
   `mciGetErrorString` reaches Warpine's `mciFreeBlock`.
3. **Warpine's UCONV ordinals disagree with Open Watcom's libconv.lib:**
   `UniFreeUconvObject` is 4 there (Warpine 2), `UniUconvToUcs` 2 (Warpine 3),
   `UniUconvFromUcs` 3 (Warpine 4), `UniMapCpToUcsCp` 10 (Warpine 6). Only
   `UniCreateUconvObject` 1 agrees. Programs linked with Open Watcom import by
   these ordinals.
4. **Dates of add-on subsystems.** MDM (3.0) and sockets (4.0) date the
   subsystem's arrival in the base product, not the function: both existed
   as add-ons earlier (MMPM/2 for 2.1; TCP/IP 2.0/3.0 for 2.x / Warp 3).
5. **The TCP/IP reference available is the 4.21 edition.** The 4.0 list is
   rebuilt from its change notes; a call added between 4.0 and 4.21 without a
   change note (e.g. `so_cancel`, `readv`/`writev`, `recvmsg`/`sendmsg`)
   would be wrongly dated 4.0.
6. **Out of scope:** TCPIP32.DLL (BSD 4.4, 149 imports in tcpip32.lib), MCIAPI,
   MMIO, DIVE, SW (secondary windows), LIBUNI — not mapped by Warpine.

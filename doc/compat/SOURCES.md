# API compatibility chart: data and sources

`os2_api_versions.tsv` dates every 32-bit export of the Open Watcom import library `os2386.lib` (as listed in `doc/os2_ordinals.md`) with the OS/2 version it first appeared in: `2.0` (2.1 / 2.11 additions folded in), `3.0` (Warp 3), `4.0` (Warp 4), `later` (4.5 and after) or `unknown`. Columns: module, ordinal, name, since, source id, note (the reason for the line). `excluded.tsv` lists the lines left out of the table (Workplace Shell class-data exports, truncated duplicate names, `DOSPTRACE`); 16-bit entry points (`DOS16*`, `VIO16*`, …) were filtered before.

`gen_compat` (`src/bin/gen_compat.rs`, ADR-0033) reads this table and Warpine's source and renders `api_coverage.svg` / `api_coverage_dark.svg`, shown at the head of the README. Exports dated `later` or `unknown` count in no version. Lines marked `H1` rest on the ordinal-chronology rule below, not on a document.

The dating was researched from public documentation only (near-clean-room policy, ADR-0004); no IBM binary was examined. It was compiled on 2026-11-18; corrections with a source are welcome.

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
| S16 | Local: `targets/os2api.def` | targets/os2api.def | Ordinals of MSG, PMCTLS, UCONV, MDM, SO32DLL, TCP32DLL used in `extra_modules.tsv`. These are partial lists: only what Warpine maps. |
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
- **Extra modules (`extra_modules.tsv`, partial name lists from S16)** —
  MSG and PMCTLS (standard file/font dialogs) are base since 2.0 (S1/S2, high).
  UCONV is base since 4.0 (S11, medium). MDM (MMPM/2) put at 3.0 (S17, low:
  MMPM/2 was an add-on for 2.1; Warp 3 installs it from the base CD).
  SO32DLL/TCP32DLL at 4.0 (S18, low-medium: in 3.0 only via BonusPak or
  Warp Connect). Other base-4.0 DLLs not covered here: Open32 (PMWINX),
  OpenGL, DIVE (Warp 3), MMIO, REXX.
- **VIOCALLS, KBDCALLS, MOUCALLS, BDCALLS** — only 16-bit entry points
  (`VIO16*` …). A 32-bit program calls them through the toolkit's thunked
  `Vio*`/`Kbd*`/`Mou*` names, and they exist since 2.0 (S1 documents them),
  but they are excluded from the 32-bit chart as asked.


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

### Other base-system modules (partial lists)

| Module | 2.0 | 3.0 (cum.) | 4.0 (cum.) | 4.0 if all unknown ≤ 4.0 | later (4.5+) | unknown | total |
|---|---:|---:|---:|---:|---:|---:|---:|
| MDM | 0 | 4 | 4 | 4 | 0 | 0 | 4 |
| MSG | 4 | 4 | 4 | 4 | 0 | 0 | 4 |
| PMCTLS | 5 | 5 | 5 | 5 | 0 | 0 | 5 |
| SO32DLL | 0 | 0 | 27 | 27 | 0 | 0 | 27 |
| TCP32DLL | 0 | 0 | 24 | 24 | 0 | 0 | 24 |
| UCONV | 0 | 0 | 5 | 5 | 0 | 0 | 5 |
| **Total** | **9** | **13** | **69** | 69 | 0 | 0 | 69 |

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
(PMWINX), VoiceType, Java. OS2386.LIB does not cover them, so a fair 4.0 bar
should add those modules (at least UCONV and the socket DLLs, `extra_modules.tsv`).

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
6. **Universe limits.** `os2_ordinals.md` is one import library. It lacks
   PMCTLS, MSG, MMPM/2, UCONV, TCP/IP and Open32. `extra_modules.tsv` covers
   only the names Warpine already maps, and its version calls (MDM 3.0,
   sockets 4.0) are low confidence: MMPM/2 was an add-on for 2.1 and TCP/IP an
   add-on (BonusPak / Warp Connect) for Warp 3.
7. **OCR matching.** Presence in the 1992/1994 scans was found by text search
   with I/l/1 and O/0 folded. Each 2.0 hit that relied on folding was
   reviewed. A missed OCR match would wrongly move a name later, but no
   documented 2.0 name is known to be affected.

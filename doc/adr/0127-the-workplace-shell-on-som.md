# ADR-0127: The Workplace Shell is built on SOM

- **Status:** Accepted
- **Date:** 2026-11-25
- **Sources:** IBM's OS/2 Warp 4 Toolkit *Workplace Shell Programming Guide* and *Reference* and *System Object Model Programming Guide* and *Reference* (kept in `doc/references/`, cited only).

## Context

The Workplace Shell's classes (`WPObject`, `WPFileSystem`, `WPFolder` …) are SOM classes: programs subclass them, call their methods through SOM bindings and register replacements with the class manager. Warpine's SOM kernel is done as far as the public sources allow (ADR-0059–0063, 0070–0073, 0122, 0126; the class manager's substitution and merge since 2026-11-25). The TODOs also listed a second object model to weigh for the WPS should SOM stay blocked: Mozilla's XPCOM (interfaces in XPIDL, reference counting, `QueryInterface`), which is not binary-compatible with SOM.

## Decision

The Workplace Shell and Warpine's own object classes are built on SOM, through Warpine's kernel and `somidl`'s bindings. XPCOM is no longer considered.

## Consequences

- WPS classes are written in IDL and built with `somidl`; programs built from source subclass and call them as on OS/2 (source-level compatibility, ADR-0123).
- What blocks IBM-built SOM binaries (SOM.DLL ordinals 48 and above, the initializer / destructor control blocks; the TODOs' blockers) blocks IBM-built WPS classes and extensions too; this decision does not change that, and a design of Warpine's own stays the way out where a source is missing (as in ADR-0122).
- The next WPS work (`WPObject` and the classes after it, persistence in OS2.INI, settings notebooks, drag and drop) needs no further choice of model.

# ADR-0129: The Workplace Shell classes are guest code in the SDK, built with somidl

- **Status:** Accepted; follows ADR-0127 (the WPS on SOM); extends ADR-0122 (somidl) and ADR-0072 (`somBuildClass`)
- **Date:** 2026-11-25
- **Sources:**
  - IBM's OS/2 Warp 4 Toolkit *Workplace Shell Programming Reference* (kept in `doc/references/`, cited only): the `WPObject` class (its description, method groups, setup keys and menu IDs) and the entries for the methods listed below; the `M_WPObject` class methods `wpclsNew`, `wpclsQueryTitle`, `wpclsQueryStyle`, `wpclsQueryDefaultView`, `wpclsInitData` and `wpclsUnInitData`.
  - The *System Object Model Programming Guide* for the `filestem` modifier.

## Context

ADR-0127 settled that the Workplace Shell is built on SOM. `WPObject` is the root of every Workplace class, with about 130 instance methods and 30 class methods. Programs subclass it in IDL and override its methods.

Warpine's root SOM classes (`SOMObject`, `SOMClass`, `SOMClassMgr`) are implemented in the host. Their methods are API thunks, and every call to a method a program may override runs as a call chain. `WPObject`'s methods call one another all the time:
- `wpclsNew` calls `somNew` (which calls `wpInitData`), then `wpSetTitle`, then `wpSetupOnce` (which calls `wpSetup`);
- `wpSetup` calls `wpScanSetupString`, `wpSetTitle` and `wpSetStyle`;
- `wpDelete` calls `wpConfirmDelete`, then `wpFree`.

Each of these may be overridden. Written in the host, every such sequence would be a chain of host continuations.

## Decision

1. **`WPObject` and `M_WPObject` are guest code in the SDK.**
   - The class is declared in `sdk/include/wpobject.idl`, with stand-in OS/2 types in `wptypes.idl`.
   - It is implemented in C in `sdk/wps/wpobject.c`.
   - `make -C sdk` runs `somidl` on the IDL (bindings into `$(OS2_BUILD)/include`, which `OS2_CFLAGS` searches) and builds `libwps.a` when `somidl` is built (`make wps`). A program links `$(OS2_LIBWPS)` before `$(OS2_LIBC)`.
   - SOM's ordinary dispatch replaces host chains, the kernel needs no new code for the class, and `somidl` gets a real user.
   - The class lives in each program for now. A shared copy in a Workplace Shell process comes with the shell itself (`WinCreateObject` and the objects of the desktop).
2. **The first part of `WPObject`** follows the Reference's signatures, in the OIDL call style (no `Environment`):
   - **setup:** `wpInitData`, `wpUnInitData`, `wpSetup`, `wpSetupOnce`, `wpScanSetupString`;
   - **title, style, default view, object ID, errors:** `wpSetTitle` / `wpQueryTitle`, `wpSetStyle` / `wpQueryStyle`, `wpSetDefaultView` / `wpQueryDefaultView`, `wpSetObjectID` / `wpQueryObjectID`, `wpSetError` / `wpQueryError`;
   - **memory:** `wpAllocMem` / `wpFreeMem`;
   - **save and restore:** `wpSaveState` / `wpRestoreState`, `wpSaveString` / `Long` / `Data`, `wpRestoreString` / `Long` / `Data`, `wpSaveImmediate`, `wpSaveDeferred`;
   - **lifetime and locking:** `wpFree`, `wpDelete`, `wpConfirmDelete`, `wpIsObjectInitialized`, `wpLockObject`, `wpUnlockObject`, `wpIsLocked`, `wpQueryFolder`;
   - **class methods (`M_WPObject`):** `wpclsNew`, `wpclsQueryTitle`, `wpclsQueryStyle`, `wpclsQueryDefaultView`, `wpclsInitData`, `wpclsUnInitData`.
3. **Behaviour.**
   - `wpInitData` runs from `WPObject`'s `somDefaultInit` and `wpUnInitData` from its `somDestruct`, inside Warpine's initializer protocol (ADR-0122), so each runs exactly once whichever class's `somNew` or `somFree` is used. `wpclsInitData` runs from `M_WPObject`'s `somDefaultInit`.
   - `wpclsNew` refuses `WPObject` itself, since the Reference says no object of that class can be created. Otherwise it:
     - makes the object;
     - gives it a title, the class's title when none is given;
     - locks it when asked;
     - runs `wpSetupOnce`, freeing the object and returning NULL when that fails;
     - marks it initialized.
   - A new object's style is its class's `wpclsQueryStyle`.
   - `wpQueryDefaultView` gives the class's view while the object's own is `OPEN_DEFAULT`.
   - `wpSetup` handles `WPObject`'s keys `TITLE`, `OBJECTID`, `DEFAULTVIEW` and the style keys:
     - the style keys are `NOCOPY`, `NODELETE`, `NODRAG`, `NODROP`, `NOLINK`, `NOSHADOW`, `NOMOVE`, `NOPRINT`, `NORENAME`, `NOSETTINGS`, `NOTVISIBLE`, `TEMPLATE` and `LOCKEDINPLACE`, each `YES` or `NO`;
     - the keys for views, icons, help and menus are accepted but not used yet.
   - `wpScanSetupString` finds a key in a `KEY=value;…` string, reading `^;` as a semicolon. Its caller's buffer works like this:
     - with a NULL buffer, it gives only the size needed;
     - with a buffer too small, it returns FALSE and the size needed.
   - `wpSaveString` / `Long` / `Data` keep what they are given with the object, by class name and key, for `wpRestoreString` / `Long` / `Data`. `wpSaveImmediate` calls `wpSaveState`, and `wpSaveDeferred` saves at once. Writing to OS2.INI belongs to the storage classes (`WPAbstract`), which come later.
   - `wpAllocMem`'s blocks are freed with the object.
   - `wpDelete` calls `wpConfirmDelete`, which gives `NO_DELETE` for `OBJSTYLE_NODELETE` (there is no confirmation dialog yet), and then calls `wpFree`.
4. **somidl** gains two features:
   - an interface that is only declared forward (`interface WPFolder;`) is an object type (`WPFolder *`), declared as a `SOMObject` in the usage bindings;
   - a parent or metaclass from an included IDL file whose implementation names a `filestem` has its usage bindings included (`#include <wpobject.h>`, or `.xh` for C++), as SOM's generated headers include their parents'.

   A program includes `<os2.h>` before a WPS class's header, as the Reference's examples do (`wptypes.idl`'s types stand for os2.h's).
5. **The kernel's `somBuildClass`** runs the metaclass's own `somDefaultInit` on the new class object, without a control block and before `somClassReady`, when the metaclass overrides it. `somNew` on a metaclass already did this; `somBuildClass` made class objects without it, so a metaclass could not initialize its class objects, and `wpclsInitData` never ran.

## Consequences

- Programs built with GCC or clang and the SDK can subclass `WPObject` in IDL (`samples/wps_object_test`). Open Watcom-built programs cannot yet, as with every SOM class (the SOM samples are built by GCC or clang).
- The rest of `WPObject`, and `WPAbstract` / `WPTransient` / `WPFileSystem`, `WPFolder` and `WPDesktop`, are added the same way. They include views (`wpOpen`, `wpClose`, `wpViewObject`), icons, pop-up menus, settings notebooks, drag and drop, OS2.INI persistence, a process-wide object-ID registry (`wpclsQueryObject`) and `WinCreateObject`.
- These classes are not binary-compatible with IBM's: the release order, the constants' values and the instance data are Warpine's. Source-level compatibility is the goal (ADR-0123).

## Warpine's choices (unverified)

The following are Warpine's own choices, not verified against IBM's Workplace Shell:
- the method release order and the instance data;
- the values of the `OBJSTYLE_*`, `CLSSTYLE_*`, `OPEN_*` and `OK_DELETE` / `NO_DELETE` / `CANCEL_DELETE` constants, and `WP_MAX_TITLE`;
- setup keys matched without regard to case;
- `NOSHADOW` as `OBJSTYLE_NOLINK`;
- `DEFAULTVIEW=<digit>` as `OPEN_USER` plus the digit;
- an object ID being `<NAME>`;
- `wpclsNew` refusing `WPObject`, and its order of steps;
- `wpclsQueryTitle` defaulting to the class name;
- `wpclsQueryStyle` defaulting to 0;
- `wpclsQueryDefaultView` defaulting to `OPEN_SETTINGS`;
- the save calls keeping their data with the object, a restore of another kind failing, and the buffer-size rules;
- `wpSaveDeferred` saving at once;
- `wpDelete` without a dialog;
- `wpclsInitData` called from `M_WPObject`'s `somDefaultInit`;
- `somBuildClass` initializing the class object after building it.

## Addendum (2026-11-25): WPAbstract and the user profile

- **`WPAbstract` / `M_WPAbstract`** (`sdk/include/wpabstract.idl`, `sdk/wps/wpabstract.c`) store their objects in the user profile (OS2.INI) through `PrfWriteProfileData`. The Reference describes `WPAbstract` as the storage class for such objects but does not give its format, so the format is Warpine's own:
  - application `WrpWPS:Objects` holds the records, each under its handle as eight hex digits;
  - a record is a magic number, a version, the class name and the object's saved values (`wpSaveString` / `Long` / `Data`), one class name, key, kind and data after another;
  - application `WrpWPS:ObjectIDs` maps an object ID to its handle;
  - `WrpWPS:Handles` / `Next` holds the next handle to give, from `0x00020001`;
  - `sdk/wps/wpsint.h` and `pm_winutil.rs` name these the same.
- **Lifetime of a stored object:**
  - it is written when `wpclsNew` has made it, and again whenever `wpSaveImmediate` runs;
  - `WPObject`'s setters (`wpSetTitle`, `wpSetStyle`, `wpSetDefaultView`, `wpSetObjectID`) call `wpSaveDeferred` once the object is initialized, as the Reference says;
  - `wpFree` removes the record and its ID, while `somFree` only makes the object dormant.
  - An object ID names one stored object: `wpSetObjectID` refuses an ID that another stored object has, so `wpclsNew` with such an ID returns NULL.
- **`WPObject`'s own state:** its `wpSaveState` saves its title, style, default view and object ID (class `WPObject`, keys 1–4). Its `wpRestoreState` restores them through the setters, which save nothing while it does.
- **Handles:**
  - `wpQueryHandle` gives a stored object's handle; an object without storage has none.
  - `M_WPObject::wpclsQueryObject(handle)` gives the awake object first (a list per process), else it wakes the stored one: it finds its class by name, which must be registered and descend from `WPAbstract`; makes the object; puts its saved values back; then calls `wpRestoreState`.
  - `WinQueryObject(id)` (PMWP 252, Open Watcom's ordinal) is a host call. It reads the ID map and returns the handle while the record exists, else NULLHANDLE.
- **Method order in C:** a program makes its classes (`<Class>NewClass`) before it calls a class method such as `_wpclsNew( _Card, … )`. C leaves open whether the method's token or the class object is read first, and the token is 0 until the class exists.

The following are Warpine's own choices, not verified against IBM's Workplace Shell:
- the record format, application names and handle numbering;
- the ID rule;
- saving on every change at once (there is no background saver);
- the restore not saving;
- awakening only registered classes;
- the dormant/free split (`somFree` keeps the record, `wpFree` removes it);
- `WinQueryObject` checking that the record exists.

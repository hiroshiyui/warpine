# ADR-0130: Workplace objects are created through the user profile until a shell holds them

- **Status:** Accepted; extends ADR-0129 (the WPS classes in the SDK, `WPAbstract`)
- **Date:** 2026-11-26
- **Sources:** IBM's OS/2 Warp 4 Toolkit *Workplace Shell Programming Reference* (kept in `doc/references/`, cited only): `WinCreateObject`, `WinDestroyObject`, `WinSetObjectData` and `WinQueryObject`. The PMWP ordinals 250, 251, 252 and 281 are from Open Watcom's import library.

## Context

On OS/2, the Workplace Shell process holds every Workplace object:
- a program calls `WinCreateObject(class, title, setup, location, flags)`, and the shell makes the object, of a class registered with it, in the folder `location`;
- `WinSetObjectData` sends the shell a setup string for an object;
- `WinDestroyObject` deletes an object.

The handles these calls return are good in every process.

In Warpine, the Workplace classes are guest code linked into the programs that use them (`libwps`, ADR-0129), and there is no shell process yet. The host can't make a guest SOM object of a class it doesn't have. What every process does share is the user profile, where `WPAbstract` already keeps its objects and their IDs.

## Decision

1. **The host records; a process makes.**
   - **`WinCreateObject` (PMWP 281)** creates no SOM object:
     - it gives a handle, from the same counter `WPAbstract` uses;
     - it writes a record of the class *without saved values*;
     - it maps the object ID found in the setup string to the handle;
     - it queues the title (as `TITLE=`) and the setup string under the handle, in application `WrpWPS:Pending`.
   - **`WinSetObjectData` (PMWP 250)** queues its setup string for an object that is stored.
   - **`WinDestroyObject` (PMWP 251)** removes the record, the queue and every ID that names the handle.
   - **`WinQueryObject` (PMWP 252)** is unchanged.
   - The handle is good at once, in every process.
2. **`libwps` makes or updates when asked.** `wpclsQueryObject(handle)` handles three cases:
   - **A record without saved values** is an object no process has made yet. `libwps` makes it as `wpclsNew` would: `somNew`, the class's title, then `wpSetupOnce` with what was queued (the creation ends, and the record goes, when that fails). It then saves the object, so the record now has its values.
   - **A stored object being awakened** gets its queue through `wpSetup` after `wpRestoreState`.
   - **An awake object** gets what was queued since, through `wpSetup`.

   The class is checked only then. It must be registered in the asking process and descend from `WPAbstract`; if it isn't, `wpclsQueryObject` gives NULL and the record stays for a process that has the class.
3. **Flags.** When the setup string's object ID already names a stored object:
   - `CO_FAILIFEXISTS`, the default (and any unknown flag), gives NULLHANDLE;
   - `CO_UPDATEIFEXISTS` queues the title and setup for that object and gives its handle;
   - `CO_REPLACEIFEXISTS` destroys it first.

   The Reference gives the names without values. Warpine's values are 0, 1 and 2.
4. **Setup strings queued in parts.** A queue is setup strings joined with `;`, so the later value of a key is the one that counts: `wpScanSetupString` (and the host's `wps_setup::scan`) now gives a key's *last* occurrence. The title goes first in the queue, so a `TITLE=` in the setup string, which comes later, wins.

   The format has one limit: a value ending in `^` cannot be followed by another item, because the `;` after it would read as the escape `^;`. `WinCreateObject` therefore drops carets at the end of a title.
5. **The location** is accepted and not used, since there are no folders yet.
6. **Parsing.** The host's part of the format and the setup-string rules are in `src/loader/wps_setup.rs`, without a `Loader`, and the `wps_setup` fuzz target checks them (ADR-0112). In its first minutes it found both setup-string problems above: a queued value ending in an escaped `^;` joining the next item, and a value ending in `^`.

## Consequences

- A program can create, change and delete Workplace objects by handle and ID as on OS/2. Another program that has the class makes them (`samples/wps_object_test`: "save" creates one without making it, and a later "load" makes it).
- An object exists as a SOM object only in a process that asked for it. A change through `WinSetObjectData` reaches a process holding the object awake when that process next asks for the object, not at once.
- When a Workplace Shell process comes, it can make every recorded object at its start, and these calls can then go to it. The record format stays the same.
- Two processes creating objects at the same moment can race on the handle counter. The profile has no cross-process lock yet.

## Warpine's choices (unverified)

The following are Warpine's own choices, not verified against IBM's Workplace Shell:
- deferred creation;
- the `WrpWPS:Pending` queue;
- the `CO_*` values, and an unknown flag failing;
- the title queued as `TITLE=`, with its trailing carets dropped;
- a key's last occurrence winning;
- the class checked only when the object is made;
- an object whose class is missing left recorded;
- the location unused.

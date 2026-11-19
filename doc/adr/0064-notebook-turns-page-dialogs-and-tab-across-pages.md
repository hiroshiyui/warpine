# ADR-0064: Notebook turns ask the owner first; the notebook loads page dialogs and owns its buttons; Tab crosses into embedded dialogs

- **Status:** Accepted; extends ADR-0042 (embedded dialogs) for Tab
- **Date:** 2026-11-19
- **Sources:**
  - The code: `src/loader/pm_notebook.rs`, `src/loader/pm_owner.rs` (`OwnerCallKind::NotebookPending`), `src/loader/pm_win.rs` (`load_dialog_template`), `src/loader/pm_types.rs` (`tab_scope`, `tab_items`, `next_tab_stop`), `src/loader/pm_button.rs` (`is_push_kind`).
  - Sample: `samples/pm_notebook_dlg_test`.
  - IBM *PM Programming Reference*, "Notebook Controls" (`BKN_PAGESELECTEDPENDING`, `BOOKPAGEINFO`, `BKM_SETNOTEBOOKBUTTONS`, `BKM_SETTABCOLOR`); the Warp 4 Toolkit's `<pmstddlg.h>` and `<pmwin.h>` (`BS_NOTEBOOKBUTTON`).
  - Earlier decisions: ADR-0025 (owner calls), ADR-0042 (embedded dialogs).

## Context

`WC_NOTEBOOK` worked, but Warp 4 programs and the coming Workplace Shell settings notebooks need more:

- **Refusing a turn.** Before a page turns, the owner gets `BKN_PAGESELECTEDPENDING` and may refuse it by setting `ulPageIdNew` to 0. The answer must come back before the notebook turns. A built-in control runs on the host, often inside another API, where no guest code can run.
- **Page dialogs from `BKM_SETPAGEINFO`.** The notebook loads a page's dialog itself, from a resource or a template in memory, either at once or when the page is first shown.
- **Notebook buttons.** These buttons are common to all pages, but their commands concern the page on top.
- **Tab.** In a dialog, Tab should move from the notebook into its top page and out again. The page is a dialog embedded in the notebook (ADR-0042), and each dialog cycled Tab through its own controls only.
- **Too many tabs.** Tabs that do not fit their band were drawn outside it.

## Decision

### Turning a page

- **A turn asks first.** It is an owner call (ADR-0025), `NotebookPending`, made on the owner's thread before the API that caused it returns.
  - When the call finishes, `notebook_turn_answered` reads the `PAGESELECTNOTIFY` as the owner left it:
    - `ulPageIdNew` 0 keeps the page;
    - anything else turns to the page asked for.
  - `BKN_PAGESELECTED` is then queued as a second owner call.
  - An owner with no procedure (a skipped call) gets the turn.
  - A turn that is out of date when answered is dropped: the top page moved on, or the page went away.
- **The same path for every turn.** Clicks, keys, page buttons and `BKM_TURNTOPAGE` all take it. `BKM_TURNTOPAGE` returns TRUE for an existing page even when the owner refuses.

### Page dialogs

- **Loaded by the notebook.** `BKM_SETPAGEINFO` with `BFA_PAGEFROMDLGRES` or `BFA_PAGEFROMDLGTEMPLATE` loads the dialog with the notebook as parent and owner, through the same path as `WinLoadDlg`.
  - `WM_INITDLG` is queued, with `pPageDlgCreateParams` as mp2.
  - A template in memory is read from its own size field, at most `MAX_DLG_TEMPLATE` bytes.
- **When.** `bLoadDlg` TRUE loads it at once. FALSE keeps a `PageDialog` in the page, and `notebook_place_pages` loads it the first time the page is on top.

### Notebook buttons

- **Plain buttons with a new kind.** `BKM_SETNOTEBOOKBUTTONS` makes `WC_BUTTON` children of the notebook of kind `BS_NOTEBOOKBUTTON`.
  - `is_push_kind` treats them as push buttons everywhere: clicking, the default button, `WM_QUERYDLGCODE`.
  - They stand in a row along the notebook's bottom.
  - A second call replaces them.
- **Commands go to the top page.** The buttons' owner is the top page's window, set at every turn (the notebook's owner when the page has none). So their `WM_COMMAND` reaches the page they act on.
- **Limits.** At most 32 buttons; ids up to `BKA_MAXBUTTONID`.

### Tab across embedded dialogs

- **The scope.** `next_tab_stop` cycles through the outermost dialog that the asking dialog is embedded in (`tab_scope`), not the asking dialog alone.
- **The items.** `tab_items` lists that dialog's controls in tree order, with the controls of the shown embedded dialogs in their place. So a notebook's top page follows the notebook, and hidden pages are passed over.
- **Getting the keys there.**
  - A notebook passes the keys it does not use to its owner, as a button does.
  - Tab on a focused notebook reaches the dialog, which moves the focus into the page.
  - Tab in the page's last control reaches the page's `WinDefDlgProc`, which moves the focus on to the control after the notebook.
- **Unchanged.** A dialog embedded in a client window has no dialog around it and keeps Tab to itself. Mnemonics, the default button and the first focus still use the dialog's own items.

### Tabs that do not fit

- **Scrolling.** A band of tabs that does not fit shows the tabs from `major_first` / `minor_first` and two arrows at its end. An arrow moves the band by one tab without turning a page.
- **Coming into view.** `notebook_place_pages` brings the current tabs into view after every turn (`reveal_tab`).

### Warpine's choices (unverified)

- `BKN_PAGESELECTEDPENDING` for `BKM_TURNTOPAGE` too.
- The page dialog's owner (the notebook).
- Where the buttons' commands go.
- `BKA_MAXBUTTONID` as a limit.
- `BKM_SETTABCOLOR` taking RGB.
- A tab bitmap replacing the text, stretched.
- The arrows scrolling without turning.

## Consequences

- **Refusing turns.** Owners can refuse page turns, for example on a page with unsaved input.
- **Programs that work now.** Programs that set their notebooks up with `BKM_SETPAGEINFO` alone get their page dialogs, and notebook buttons act on the page shown.
- **Keyboard use.** Notebooks in dialogs can be used from the keyboard, and the "Tab between an embedded dialog's controls and those around it" gap of ADR-0042 is closed for dialogs in dialogs.
- **Many tabs.** Notebooks with many tabs stay inside their band.
- **Tests:**
  - `pm_notebook.rs`: the pending turn, buttons, page dialogs, scrolling and key forwarding;
  - `pm_types.rs`: Tab across a page;
  - `samples/pm_notebook_dlg_test`: 35 checks, with a golden picture.

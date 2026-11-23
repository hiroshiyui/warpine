// SPDX-License-Identifier: GPL-3.0-or-later
//
//! SOM.DLL: the System Object Model runtime kernel (ADR-0059, ADR-0060),
//! from the public SOM documentation (*SOM Programming Guide* and
//! *Reference*, "SOMObject", "SOMClass", "SOMClassMgr", the SOM functions
//! and runtime variables; as on EDM/2).
//!
//! - **Objects.** The first word of every SOM object points to its class's
//!   method table; a class is itself an object (an instance of its
//!   metaclass, `SOMClass` by default) and carries its metaclass's instance
//!   data. The guest sees:
//!   - method tables: IBM's `somMethodTab` header, then the entries;
//!   - class objects (the metaclass's method table, then the metaclass's
//!     instance data);
//!   - the `ClassData` of the root classes (`SOMObjectClassData`,
//!     `SOMClassClassData`, `SOMClassMgrClassData`), `SOMClassMgrObject`
//!     and the runtime variables (`SOMMalloc` … `SOM_AssertLevel`),
//!     exported as data.
//! - **Host records.** Every class is also kept here ([`SomRuntime`]):
//!   name, parents, metaclass, its methods, instance layout. Method and
//!   data lookups (`somResolve`, `somClassResolve`, `somDataResolve`)
//!   answer from these records, so a guest that overwrites a method table
//!   cannot steer them.
//! - **Tokens.** A method token names one method introduced by one class
//!   (its index in the runtime's method list plus one); each class maps
//!   the tokens it supports to its procedures, so a token is valid for the
//!   introducing class and every descendant, however many parents they
//!   have. A data token is the introducing class's object: each class
//!   knows where every ancestor's instance data lies in its instances.
//! - **Multiple inheritance.** A class's methods start with its first
//!   parent's; later parents add the methods it lacks, and an override
//!   from a later parent replaces an implementation the earlier ones left
//!   at the introducer's. Instance data of an ancestor reached twice is
//!   laid out once.
//! - **Metaclasses.** `WrpSomMetaclassFor` gives a binding the metaclass
//!   for a new class: the requested one if it descends from the parent's
//!   metaclass, else the parent's if that descends from the requested one,
//!   else a derived metaclass with both as parents, made once.
//! - **Root classes.** The methods of `SOMObject`, `SOMClass` and
//!   `SOMClassMgr` are API thunks of this module, so their method tables
//!   hold thunk addresses like any guest procedure's. `somNew` calls the
//!   object's `somDefaultInit`, whose root implementation calls its
//!   `somInit`; `somFree` and `somDestruct` call its `somUninit` — guest
//!   overrides included — through call chains (ADR-0026).
//! - **Building classes** uses the documented `SOMClass` methods:
//!   `somNew` on a metaclass, `somInitClass` / `somInitMIClass`,
//!   `somAddStaticMethod`, `somOverrideSMethod`, `somClassReady`.
//! - **Memory.** Objects, method tables and `SOMMalloc` blocks come from a
//!   heap of 64 KB arenas ([`SomHeap`]); `somPrintf` formats as C does.
//!
//! - **Ordinals** (ADR-0070). 1–47 are IBM's SOM 1.0 exports as osFree
//!   lists them (`SOM/som/makefile` in osfree-project/osfree,
//!   BSD-3-Clause): data 1–19, functions 20–47. Warpine's own exports and
//!   the SOM 2.x functions whose IBM numbers are unknown use 3072–4095.
//!
//! - **IBM's layouts** (ADR-0072). The root classes' `ClassData` follow
//!   IBM's release orders, method tables have the `somMethodTab` header and
//!   `CClassData.parentMtab` is a `somParentMtabStruct`, as the OS/2 Warp 4
//!   Toolkit headers give them; root methods Warpine lacks raise
//!   `SOMERROR_NotImplementedYet` as a warning.
//!
//! Warpine's choices (unverified against IBM's SOM): the token encodings,
//! osFree's ordinals, the Toolkit copy the layouts come from, the fields of
//! `somParentMtabStruct` left 0, the resolution rule of multiple
//! inheritance, the derived metaclass's name, what `somPrintSelf`,
//! `somDumpSelf` and a kept `somAssert` print, the indentation of
//! `somPrefixLevel`, `somCheckArgs`'s levels, and `somConstructClass`,
//! `somSaveMetrics` and `somWriteMetrics`, which do nothing.

use std::collections::{HashMap, HashSet};

use super::constants::{MAGIC_API_BASE, SOM_BASE, STDOUT_HANDLE};
use super::pm_hook::{CallChain, CallStep, ChainThen};
use super::{ApiResult, Loader, MutexExt};

// SOM.DLL's ordinals 1–47 are IBM's SOM 1.0 exports as osFree lists them:
// the `EXPORTS` of `SOM/som/makefile` in osfree-project/osfree
// (https://github.com/osfree-project/osfree, BSD-3-Clause), the same
// numbers as Free Pascal's `som.pas` (ADR-0070). They are not verified
// against IBM's `som.lib`. Ordinals 48–3071 are left to IBM's later
// exports, which are not known yet; Warpine's own exports use 3072–4095
// (the SOM module is SOM_ORDINALS wide).

/// Data exports 1–19: their addresses, not thunks
/// (`Loader::som_data_address`).
pub const SOM_VAR_CALLOC: u32 = 1;
pub const SOM_CLASS_CCLASS_DATA: u32 = 2;
pub const SOM_CLASS_CLASS_DATA: u32 = 3;
pub const SOM_VAR_CLASS_INIT_FUNC_NAME: u32 = 4;
pub const SOM_CLASS_MGR_CCLASS_DATA: u32 = 5;
pub const SOM_CLASS_MGR_CLASS_DATA: u32 = 6;
pub const SOM_CLASS_MGR_OBJECT: u32 = 7;
pub const SOM_VAR_DELETE_MODULE: u32 = 8;
pub const SOM_VAR_ERROR: u32 = 9;
pub const SOM_VAR_FREE: u32 = 10;
pub const SOM_VAR_LOAD_MODULE: u32 = 11;
pub const SOM_VAR_MALLOC: u32 = 12;
pub const SOM_OBJECT_CCLASS_DATA: u32 = 13;
pub const SOM_OBJECT_CLASS_DATA: u32 = 14;
pub const SOM_VAR_OUT_CHAR: u32 = 15;
pub const SOM_VAR_REALLOC: u32 = 16;
pub const SOM_ASSERT_LEVEL: u32 = 17;
pub const SOM_TRACE_LEVEL: u32 = 18;
pub const SOM_WARN_LEVEL: u32 = 19;
/// The first and last data exports.
pub const SOM_FIRST_DATA: u32 = SOM_VAR_CALLOC;
pub const SOM_LAST_DATA: u32 = SOM_WARN_LEVEL;
/// The data exports, in the order of the data block.
const DATA_EXPORTS: usize = (SOM_LAST_DATA - SOM_FIRST_DATA + 1) as usize;

/// IBM's SOM 1.0 functions (osFree's numbers).
pub const SOM_CLASS_MGR_NEW_CLASS: u32 = 20;
pub const SOM_CLASS_NEW_CLASS: u32 = 21;
pub const SOM_OBJECT_NEW_CLASS: u32 = 22;
pub const SOM_ASSERT: u32 = 23;
pub const SOM_BEGIN_PERSISTENT_IDS: u32 = 24;
pub const SOM_CHECK_ARGS: u32 = 25;
pub const SOM_CHECK_ID: u32 = 26;
pub const SOM_COMPARE_IDS: u32 = 27;
pub const SOM_CONSTRUCT_CLASS: u32 = 28;
pub const SOM_END_PERSISTENT_IDS: u32 = 29;
pub const SOM_ENVIRONMENT_NEW: u32 = 30;
pub const SOM_ID_FROM_STRING: u32 = 31;
pub const SOM_L_PRINTF: u32 = 32;
pub const SOM_PARENT_RESOLVE: u32 = 33;
pub const SOM_PREFIX_LEVEL: u32 = 34;
pub const SOM_PRINTF: u32 = 35;
pub const SOM_REGISTER_ID: u32 = 36;
pub const SOM_RESOLVE: u32 = 37;
pub const SOM_SAVE_METRICS: u32 = 38;
pub const SOM_SET_EXPECTED_IDS: u32 = 39;
pub const SOM_STRING_FROM_ID: u32 = 40;
pub const SOM_TEST: u32 = 41;
pub const SOM_TEST_CLS: u32 = 42;
pub const SOM_TOTAL_REG_IDS: u32 = 43;
pub const SOM_UNIQUE_KEY: u32 = 44;
pub const SOM_VPRINTF: u32 = 45;
pub const SOM_WRITE_METRICS: u32 = 46;
pub const SOM_DATA_RESOLVE: u32 = 47;
/// The last of IBM's ordinals Warpine knows.
pub const SOM_LAST_IBM: u32 = SOM_DATA_RESOLVE;

/// The first ordinal of Warpine's own exports: below it, IBM's.
pub const SOM_FIRST_PRIVATE: u32 = 3072;
/// SOM 2.x functions whose IBM ordinals are not known: imported by name.
pub const SOM_RESOLVE_BY_NAME: u32 = 3072;
pub const SOM_CLASS_RESOLVE: u32 = 3073;
pub const SOM_GET_GLOBAL_ENVIRONMENT: u32 = 3074;
pub const SOM_IS_OBJ: u32 = 3075;
/// `somSetOutChar(routine)`: the routine `somPrintf` writes through.
pub const SOM_SET_OUT_CHAR: u32 = 3076;
/// `somParentNumResolve(parentMtab, n, token)` (SOM 2.x; ADR-0072).
pub const SOM_PARENT_NUM_RESOLVE: u32 = 3077;
/// `somBuildClass(inheritVars, sci, major, minor)` (SOM 2.x; ADR-0073).
pub const SOM_BUILD_CLASS: u32 = 3078;
/// The procedures the runtime variables `SOMMalloc` … `SOMFree` hold.
pub const SOM_MALLOC: u32 = 3080;
pub const SOM_CALLOC: u32 = 3081;
pub const SOM_REALLOC: u32 = 3082;
pub const SOM_FREE_MEM: u32 = 3083;
/// `WrpSomMetaclassFor(requested, parent)`: the metaclass of a new class.
pub const SOM_METACLASS_FOR: u32 = 3084;
/// The procedures the runtime variables `SOMLoadModule`,
/// `SOMDeleteModule`, `SOMError`, `SOMOutCharRoutine` and
/// `SOMClassInitFuncName` hold at first.
pub const SOM_DEFAULT_LOAD_MODULE: u32 = 3085;
pub const SOM_DEFAULT_DELETE_MODULE: u32 = 3086;
pub const SOM_DEFAULT_ERROR: u32 = 3087;
pub const SOM_DEFAULT_OUT_CHAR: u32 = 3088;
pub const SOM_DEFAULT_CLASS_INIT_FUNC_NAME: u32 = 3089;
/// What an unknown method token resolves to: a procedure returning 0.
pub const SOM_METHOD_ERROR: u32 = 3199;
/// The procedure of root methods Warpine does not implement yet: it
/// raises `SOMERROR_NotImplementedYet`, as a warning, and returns 0.
pub const SOM_METHOD_NOT_IMPLEMENTED: u32 = 3198;
// The root classes' methods follow: SOMObject's from 3200, SOMClass's
// from 3300, SOMClassMgr's from 3400; the module has SOM_ORDINALS.
const _: () = assert!(SOM_LAST_IBM < SOM_FIRST_PRIVATE && SOM_RESOLVE_BY_NAME >= SOM_FIRST_PRIVATE);
/// The last ordinal of Warpine's own exports (SOMClassMgr's last method).
pub const SOM_LAST_PRIVATE: u32 = 3407;
const _: () = assert!(SOM_LAST_PRIVATE < super::constants::SOM_ORDINALS);
/// A method of a root class Warpine does not implement yet: its token
/// resolves to [`SOM_METHOD_NOT_IMPLEMENTED`].
const NOT_IMPLEMENTED: u32 = 0;
/// The methods of `SOMObject`, `SOMClass` and `SOMClassMgr` in IBM's
/// release order (the `releaseorder` of `somobj.idl`, `somcls.idl` and
/// `somcm.idl` in the OS/2 Warp 4 Toolkit, ADR-0072), with the ordinal of
/// Warpine's procedure. Their `ClassData` hold the tokens in this order,
/// so a program built with IBM's bindings finds each method at IBM's
/// offset.
pub const SOMOBJECT_METHODS: &[(u32, &str)] = &[
    (3200, "somInit"), (3201, "somUninit"), (3202, "somFree"), (NOT_IMPLEMENTED, "somDefaultVCopyInit"),
    (3204, "somGetClassName"), (3203, "somGetClass"), (3206, "somIsA"), (3208, "somRespondsTo"),
    (3207, "somIsInstanceOf"), (3205, "somGetSize"), (3210, "somDumpSelf"), (3211, "somDumpSelfInt"),
    (3209, "somPrintSelf"), (NOT_IMPLEMENTED, "somDefaultConstVCopyInit"), (NOT_IMPLEMENTED, "somDispatchV"),
    (NOT_IMPLEMENTED, "somDispatchL"), (NOT_IMPLEMENTED, "somDispatchA"), (NOT_IMPLEMENTED, "somDispatchD"),
    (NOT_IMPLEMENTED, "somDispatch"), (NOT_IMPLEMENTED, "somClassDispatch"), (NOT_IMPLEMENTED, "somCastObj"),
    (NOT_IMPLEMENTED, "somResetObj"), (3212, "somDefaultInit"), (3213, "somDestruct"),
    (NOT_IMPLEMENTED, "somPrivate1"), (NOT_IMPLEMENTED, "somPrivate2"),
    (NOT_IMPLEMENTED, "somDefaultCopyInit"), (NOT_IMPLEMENTED, "somDefaultConstCopyInit"),
    (NOT_IMPLEMENTED, "somDefaultAssign"), (NOT_IMPLEMENTED, "somDefaultConstAssign"),
    (NOT_IMPLEMENTED, "somDefaultVAssign"), (NOT_IMPLEMENTED, "somDefaultConstVAssign"),
];
pub const SOMCLASS_METHODS: &[(u32, &str)] = &[
    (3300, "somNew"), (NOT_IMPLEMENTED, "somRenew"), (3310, "somInitClass"), (3313, "somClassReady"),
    (3302, "somGetName"), (3303, "somGetParent"), (3309, "somDescendedFrom"), (3317, "somCheckVersion"),
    (3321, "somFindMethod"), (3322, "somFindMethodOk"), (3315, "somSupportsMethod"),
    (3307, "somGetNumMethods"), (3304, "somGetInstanceSize"), (3320, "somGetInstanceOffset"),
    (3305, "somGetInstancePartSize"), (NOT_IMPLEMENTED, "somGetMethodIndex"),
    (3308, "somGetNumStaticMethods"), (NOT_IMPLEMENTED, "somGetPClsMtab"), (3319, "somGetClassMtab"),
    (3311, "somAddStaticMethod"), (3312, "somOverrideSMethod"), (NOT_IMPLEMENTED, "somAddDynamicMethod"),
    (NOT_IMPLEMENTED, "somcPrivate0"), (NOT_IMPLEMENTED, "somGetApplyStub"), (3323, "somFindSMethod"),
    (3324, "somFindSMethodOk"), (NOT_IMPLEMENTED, "somGetMethodDescriptor"),
    (NOT_IMPLEMENTED, "somGetNthMethodInfo"), (NOT_IMPLEMENTED, "somSetClassData"),
    (NOT_IMPLEMENTED, "somGetClassData"), (3301, "somNewNoInit"), (NOT_IMPLEMENTED, "somRenewNoInit"),
    (3306, "somGetInstanceToken"), (NOT_IMPLEMENTED, "somGetMemberToken"), (NOT_IMPLEMENTED, "somcPrivate1"),
    (NOT_IMPLEMENTED, "somGetMethodData"), (NOT_IMPLEMENTED, "somOverrideMtab"), (3314, "somGetMethodToken"),
    (NOT_IMPLEMENTED, "somGetParents"), (NOT_IMPLEMENTED, "somGetPClsMtabs"), (3316, "somInitMIClass"),
    (3318, "somGetVersionNumbers"), (3325, "somLookupMethod"),
    (NOT_IMPLEMENTED, "_get_somInstanceDataOffsets"), (NOT_IMPLEMENTED, "somRenewNoZero"),
    (NOT_IMPLEMENTED, "somRenewNoInitNoZero"), (3326, "somAllocate"), (3327, "somDeallocate"),
    (NOT_IMPLEMENTED, "somGetRdStub"), (NOT_IMPLEMENTED, "somGetNthMethodData"),
    (NOT_IMPLEMENTED, "somcPrivate2"), (NOT_IMPLEMENTED, "somcPrivate3"),
    (NOT_IMPLEMENTED, "_get_somDirectInitClasses"), (NOT_IMPLEMENTED, "somcPrivate5"),
    (NOT_IMPLEMENTED, "somcPrivate6"), (NOT_IMPLEMENTED, "somcPrivate7"), (NOT_IMPLEMENTED, "somcPrivate8"),
    (NOT_IMPLEMENTED, "somcPrivate9"), (NOT_IMPLEMENTED, "somcPrivate10"), (NOT_IMPLEMENTED, "somcPrivate11"),
    (NOT_IMPLEMENTED, "somcPrivate12"), (NOT_IMPLEMENTED, "somDefinedMethod"),
    (NOT_IMPLEMENTED, "somcPrivate13"), (NOT_IMPLEMENTED, "somcPrivate14"),
    (NOT_IMPLEMENTED, "somcPrivate15"), (NOT_IMPLEMENTED, "somcPrivate16"),
    (NOT_IMPLEMENTED, "somcPrivate17"), (NOT_IMPLEMENTED, "somcPrivate18"),
    (NOT_IMPLEMENTED, "somcPrivate19"), (NOT_IMPLEMENTED, "somcPrivate20"),
    (NOT_IMPLEMENTED, "_get_somDataAlignment"), (NOT_IMPLEMENTED, "somcPrivate21"),
    (NOT_IMPLEMENTED, "somcPrivate22"), (NOT_IMPLEMENTED, "somcPrivate23"),
    (NOT_IMPLEMENTED, "somcPrivate24"), (NOT_IMPLEMENTED, "somcPrivate25"),
];
pub const SOMCLASSMGR_METHODS: &[(u32, &str)] = &[
    (3403, "somFindClsInFile"), (3400, "somFindClass"), (3401, "somClassFromId"), (3402, "somRegisterClass"),
    (3406, "somUnregisterClass"), (3404, "somLocateClassFile"), (3405, "somLoadClassFile"),
    (3407, "somUnloadClassFile"), (NOT_IMPLEMENTED, "somGetInitFunction"), (NOT_IMPLEMENTED, "somMergeInto"),
    (NOT_IMPLEMENTED, "somGetRelatedClasses"), (NOT_IMPLEMENTED, "somSubstituteClass"),
    (NOT_IMPLEMENTED, "_get_somInterfaceRepository"), (NOT_IMPLEMENTED, "_set_somInterfaceRepository"),
    (NOT_IMPLEMENTED, "_get_somRegisteredClasses"), (NOT_IMPLEMENTED, "somBeginPersistentClasses"),
    (NOT_IMPLEMENTED, "somEndPersistentClasses"), (NOT_IMPLEMENTED, "somcmPrivate1"),
    (NOT_IMPLEMENTED, "somcmPrivate2"), (NOT_IMPLEMENTED, "somRegisterClassLibrary"),
    (NOT_IMPLEMENTED, "somJoinAffinityGroup"), (NOT_IMPLEMENTED, "somUnregisterClassLibrary"),
];

/// The token of `SOMObject` method `name`: its place in the release
/// order plus one (the runtime makes SOMObject's tokens first).
const fn root_token(name: &str) -> u32 {
    let mut i = 0;
    while i < SOMOBJECT_METHODS.len() {
        if str_eq(SOMOBJECT_METHODS[i].1, name) { return i as u32 + 1; }
        i += 1;
    }
    panic!("not a SOMObject method")
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] { return false; }
        i += 1;
    }
    true
}

/// Most classes a process defines (Warpine's bound).
pub const MAX_CLASSES: usize = 4096;
/// Most methods of one class, and most methods of all classes.
pub const MAX_METHODS: usize = 4096;
pub const MAX_TOKENS: usize = 65536;
/// Most parents of one class.
pub const MAX_PARENTS: usize = 16;
/// Most objects alive at once.
pub const MAX_INSTANCES: usize = 1 << 20;
/// Most distinct `somId`s.
pub const MAX_IDS: usize = 65536;
/// Largest instance data of one class, in bytes.
pub const MAX_INSTANCE_DATA: u32 = 16 << 20;
/// Longest class or method name, in bytes.
pub const MAX_NAME: usize = 255;
/// Largest `SOMMalloc` block, and most live blocks.
pub const MAX_ALLOC: u32 = 64 << 20;
pub const MAX_BLOCKS: usize = 1 << 20;
/// Longest `somPrintf` output, in bytes.
pub const MAX_PRINT: usize = 65536;
/// The initialization function `somFindClass` asks `SOMLoadModule` to
/// call in a class's DLL (else `<Class>NewClass`).
pub const SOM_INIT_MODULE: &str = "SOMInitModule";
/// The file name errors the kernel raises give `SOMError` (Warpine's).
pub const SOM_KERNEL_FILE: &str = "som.dll";
/// Most arguments `somCheckArgs` looks at.
const MAX_ARGS: u32 = 1024;
/// A SOM error code's last digit: its severity; 9 is fatal.
const SOM_FATAL: u32 = 9;
/// Bytes of an `Environment` (`_major`, exception id, its parameters).
const ENVIRONMENT_SIZE: u32 = 16;
/// Size of the heap's arenas; larger blocks get pages of their own.
const ARENA_SIZE: u32 = 64 << 10;
const SMALL_MAX: u32 = ARENA_SIZE / 2;
/// Tokens of the root methods the runtime calls itself.
const TOKEN_SOM_INIT: u32 = root_token("somInit");
const TOKEN_SOM_UNINIT: u32 = root_token("somUninit");
const TOKEN_SOM_DUMP_SELF_INT: u32 = root_token("somDumpSelfInt");
const TOKEN_SOM_DEFAULT_INIT: u32 = root_token("somDefaultInit");
const TOKEN_SOM_DESTRUCT: u32 = root_token("somDestruct");

/// One method a class supports: its token and procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub token: u32,
    pub proc: u32,
}

/// A class: its object, name, place in the hierarchy, methods and instance
/// layout.
#[derive(Debug, Clone, Default)]
pub struct SomClass {
    pub name: String,
    /// Guest copy of the name (`somGetName`).
    pub name_addr: u32,
    /// Parent class objects, left to right (none: `SOMObject` itself).
    pub parents: Vec<u32>,
    /// Metaclass object.
    pub meta: u32,
    /// The methods it supports, inherited ones first.
    pub slots: Vec<Slot>,
    /// Methods this class introduced.
    pub own_methods: usize,
    /// Where each class's instance data lies in an instance: (class
    /// object, offset), this class's last.
    pub parts: Vec<(u32, u32)>,
    /// Size of the class's own instance data.
    pub data_size: u32,
    /// Size of an instance.
    pub instance_size: u32,
    /// Guest method table (0 until `somClassReady`).
    pub mtab: u32,
    pub ready: bool,
    /// Major and minor version (`somInitClass`; 0.0: none given).
    pub version: (u32, u32),
}

impl SomClass {
    fn slot(&self, token: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.token == token)
    }

    fn offset_of(&self, cls: u32) -> Option<u32> {
        self.parts.iter().find(|p| p.0 == cls).map(|p| p.1)
    }
}

/// The runtime's heap: 64 KB arenas carved into 16-byte-rounded blocks,
/// freed blocks kept by size; large blocks are pages of their own.
#[derive(Debug, Default)]
pub struct SomHeap {
    /// Live blocks and their rounded sizes.
    pub blocks: HashMap<u32, u32>,
    /// Live blocks with pages of their own.
    big: HashSet<u32>,
    /// Freed blocks by rounded size.
    free: HashMap<u32, Vec<u32>>,
    cursor: u32,
    end: u32,
}

impl SomHeap {
    /// The rounded size of a request (`None`: too large).
    pub fn rounded(size: u32) -> Option<u32> {
        let r = size.max(1).checked_add(15)? & !15;
        (r <= MAX_ALLOC).then_some(r)
    }

    /// A small block from the free lists or the current arena.
    fn take(&mut self, rounded: u32) -> Option<u32> {
        if let Some(at) = self.free.get_mut(&rounded).and_then(Vec::pop) { return Some(at); }
        if self.end - self.cursor < rounded { return None; }
        let at = self.cursor;
        self.cursor += rounded;
        Some(at)
    }

    /// A new arena; the rest of the old one becomes a free block.
    fn add_arena(&mut self, at: u32) {
        if self.end > self.cursor { self.free.entry(self.end - self.cursor).or_default().push(self.cursor); }
        (self.cursor, self.end) = (at, at + ARENA_SIZE);
    }
}

/// The process's SOM runtime.
#[derive(Debug, Default)]
pub struct SomRuntime {
    pub initialized: bool,
    /// Classes by class object.
    pub classes: HashMap<u32, SomClass>,
    /// Ready classes by name.
    pub by_name: HashMap<String, u32>,
    /// Method tables → their class.
    pub mtabs: HashMap<u32, u32>,
    /// Every method: its introducing class and name (token = index + 1).
    pub methods: Vec<(u32, String)>,
    /// Derived metaclasses by (requested, parent's metaclass).
    pub derived: HashMap<(u32, u32), u32>,
    /// Objects `somNew` made and `somFree` has not freed.
    pub instances: HashSet<u32>,
    /// `somId`s by string.
    pub ids: HashMap<String, u32>,
    /// The root classes and the class manager.
    pub som_object: u32,
    pub som_class: u32,
    pub som_class_mgr: u32,
    pub mgr: u32,
    /// The global `Environment` and the exported data block.
    pub env: u32,
    pub data: u32,
    pub heap: SomHeap,
    /// Classes in the order they became ready (to tell which a module's
    /// loading made).
    pub ready_order: Vec<u32>,
    /// Classes made while the class manager loaded a module: its handle.
    pub modules: HashMap<u32, u32>,
    /// The guest copy of "SOMInitModule" the default
    /// `SOMClassInitFuncName` returns.
    pub init_func_name: u32,
    /// The guest copy of [`SOM_KERNEL_FILE`].
    pub kernel_file: u32,
    /// Data tokens of instance data at an offset inside a class's part
    /// (`somBuildClass`'s non-internal data): token → (class, offset).
    pub data_tokens: HashMap<u32, (u32, u32)>,
}

impl SomRuntime {
    /// Whether class `cls` is `ancestor` or descends from it.
    pub fn descends(&self, cls: u32, ancestor: u32) -> bool {
        let mut seen = HashSet::new();
        let mut todo = vec![cls];
        while let Some(c) = todo.pop() {
            if c == ancestor { return true; }
            if seen.len() >= MAX_CLASSES || !seen.insert(c) { continue; }
            if let Some(k) = self.classes.get(&c) { todo.extend(&k.parents); }
        }
        false
    }

    /// The procedure of method `token` of class `cls`.
    pub fn class_method(&self, cls: u32, token: u32) -> Option<u32> {
        self.classes.get(&cls)?.slot(token).map(|s| s.proc)
    }

    /// The token of method `name` as class `cls` supports it.
    pub fn token_of(&self, cls: u32, name: &str) -> Option<u32> {
        let c = self.classes.get(&cls)?;
        c.slots.iter().map(|s| s.token).find(|&t| {
            t.checked_sub(1).and_then(|i| self.methods.get(i as usize)).is_some_and(|m| m.1 == name)
        })
    }

    /// A new token for method `name` introduced by `cls`.
    fn new_token(&mut self, cls: u32, name: &str) -> Option<u32> {
        if self.methods.len() >= MAX_TOKENS { return None; }
        self.methods.push((cls, name.to_string()));
        Some(self.methods.len() as u32)
    }

    /// The introducer's own procedure of method `token`.
    fn default_proc(&self, token: u32) -> Option<u32> {
        let intro = self.methods.get(token.checked_sub(1)? as usize)?.0;
        self.class_method(intro, token)
    }

    /// Lay out class `cls` with `parents` (all ready) and `size` bytes of
    /// instance data of its own: methods and data parts inherited.
    fn inherit(&mut self, cls: u32, parents: &[u32], size: u32) -> Option<()> {
        let first = self.classes.get(parents.first()?)?;
        let mut slots = first.slots.clone();
        let mut parts = first.parts.clone();
        let mut end = first.instance_size;
        for p in &parents[1..] {
            let p = self.classes.get(p)?;
            for s in &p.slots {
                match slots.iter().position(|o| o.token == s.token) {
                    None => slots.push(*s),
                    Some(i) => {
                        let default = self.default_proc(s.token);
                        if Some(slots[i].proc) == default && Some(s.proc) != default { slots[i].proc = s.proc; }
                    }
                }
            }
            for &(pc, _) in &p.parts {
                if parts.iter().any(|q| q.0 == pc) { continue; }
                let at = end.checked_add(3)? & !3;
                parts.push((pc, at));
                end = at.checked_add(self.classes.get(&pc)?.data_size)?;
            }
        }
        if slots.len() > MAX_METHODS { return None; }
        let at = end.checked_add(3)? & !3;
        parts.push((cls, at));
        let instance_size = at.checked_add(size)?.checked_add(3)? & !3;
        let c = self.classes.get_mut(&cls)?;
        (c.parents, c.slots, c.parts, c.own_methods, c.data_size, c.instance_size) =
            (parents.to_vec(), slots, parts, 0, size, instance_size);
        Some(())
    }
}

/// Whether the first `inits` calls of a chain, `_DLL_InitTerm`s, all
/// succeeded (TRUE).
fn som_inits_ok(results: &[u32], inits: u32) -> bool {
    !results.iter().take(inits as usize).any(|&r| r == 0)
}

/// The `somStaticClassInfo` layout versions `somBuildClass` reads
/// (`SOM_SCILEVEL`, `somapi.h`).
const SOM_SCI_LEVEL: u32 = 4;
/// Word indices of the `somStaticClassInfo` fields used (`somapi.h`).
const SCI_LAYOUT_VERSION: u32 = 0;
const SCI_NUM_STATIC_METHODS: u32 = 1;
const SCI_NUM_STATIC_OVERRIDES: u32 = 2;
const SCI_NUM_NON_INTERNAL_DATA: u32 = 3;
const SCI_NUM_PROC_METHODS: u32 = 4;
const SCI_NUM_VARARGS_FUNCS: u32 = 5;
const SCI_INSTANCE_DATA_SIZE: u32 = 8;
const SCI_NUM_PARENTS: u32 = 10;
const SCI_CLASS_ID: u32 = 11;
const SCI_EXPLICIT_META_ID: u32 = 12;
const SCI_PARENTS: u32 = 14;
const SCI_CDS: u32 = 15;
const SCI_CCDS: u32 = 16;
const SCI_SMT: u32 = 17;
const SCI_OMT: u32 = 18;
const SCI_NIT_REFERENCE_BASE: u32 = 19;
const SCI_NIT: u32 = 20;
const SCI_PMT: u32 = 21;
const SCI_VFT: u32 = 22;
const SCI_CIF: u32 = 23;
/// Layout 3 extensions.
const SCI_NUM_METHODS: u32 = 27;
const SCI_MT: u32 = 28;
const SCI_NUM_INHERITED_METHODS: u32 = 31;
const SCI_IMT: u32 = 32;
/// `somMethods_t` actions (`type & 0xFF`).
const SCI_MT_STATIC: u32 = 0;
const SCI_MT_DYNAMIC: u32 = 1;
const SCI_MT_NONSTATIC: u32 = 2;
const SCI_MT_TOKEN_OVERRIDE: u32 = 6;

/// What `somBuildClass` takes from a `somStaticClassInfo`, its counts
/// bounded.
#[derive(Debug, Clone, Copy)]
struct SciInfo {
    num_static_methods: u32,
    num_overrides: u32,
    num_non_internal_data: u32,
    num_proc_methods: u32,
    num_varargs_funcs: u32,
    instance_data_size: u32,
    num_parents: u32,
    class_id: u32,
    explicit_meta_id: u32,
    parents: u32,
    cds: u32,
    ccds: u32,
    smt: u32,
    omt: u32,
    nit_reference_base: u32,
    nit: u32,
    pmt: u32,
    vft: u32,
    cif: u32,
    num_methods: u32,
    mt: u32,
    num_inherited_methods: u32,
    imt: u32,
}

/// The procedure of a root method: its thunk, or the not-implemented
/// procedure's.
fn root_proc(ord: u32) -> u32 {
    thunk(if ord == NOT_IMPLEMENTED { SOM_METHOD_NOT_IMPLEMENTED } else { ord })
}

/// The thunk address of SOM ordinal `ord`.
pub fn thunk(ord: u32) -> u32 {
    (MAGIC_API_BASE + (SOM_BASE + ord) as u64) as u32
}

/// Blanks `somPrefixLevel` writes for each level (Warpine's choice, ADR-0070).
pub const SOM_PREFIX_WIDTH: u32 = 2;
/// The deepest level `somPrefixLevel` indents to.
pub const SOM_MAX_PREFIX_LEVEL: u32 = 64;

/// The blanks before a line at `level` (`somPrefixLevel`, `somLPrintf`).
fn som_prefix(level: u32) -> Vec<u8> {
    let level = (level as i32).clamp(0, SOM_MAX_PREFIX_LEVEL as i32) as u32;
    vec![b' '; (level * SOM_PREFIX_WIDTH) as usize]
}

/// Bytes of a `somMethodTabList` entry: a method table, then the next entry.
const SOM_METHOD_TAB_LIST_SIZE: u32 = 8;

/// Bytes of a `<Class>CClassData`: `parentMtab`, then the class's
/// instance-data token (`somapi.h`, ADR-0072); `SOMObjectCClassData`
/// adds six `somDispatch*` procedures.
const CCLASS_DATA_SIZE: u32 = 8;
const SOMOBJECT_CCLASS_DATA_SIZE: u32 = CCLASS_DATA_SIZE + 6 * 4;
/// Bytes of a `somParentMtabStruct` (`somapi.h`): the class's own method
/// table, then `next` (the parents' tables), the class object, the renew
/// thunk, the instance size, the initializers, the resolved tokens, three
/// four-word control blocks and three counts. Warpine fills the first
/// five words and leaves the rest 0.
const PARENT_MTAB_STRUCT_SIZE: u32 = 22 * 4;
/// Bytes before the first entry of a `somMethodTab` (`somapi.h`):
/// `classObject`, `classInfo`, `className`, `instanceSize`,
/// `dataAlignment`, `mtabSize`, `protectedDataOffset`,
/// `protectedDataToken`, `embeddedObjs`.
pub const MTAB_HEADER_SIZE: u32 = 9 * 4;
/// The version of the root classes (`SOMObject_MajorVersion` … in the
/// Toolkit's `somobj.h`, `somcls.h`; `somcm.idl`).
const ROOT_VERSION: (u32, u32) = (1, 5);
/// The alignment of instance data Warpine lays out.
const DATA_ALIGNMENT: u32 = 4;
/// The SOM error codes the kernel raises (SOM Programming Guide, "SOM
/// Kernel Error Codes": 20000 + 10 × number + severity).
pub const SOMERROR_METHOD_NOT_FOUND: u32 = 20039;
pub const SOMERROR_TEST_OBJECT_FAILURE: u32 = 20109;
/// `SOMERROR_NotImplementedYet` (20179) as a warning: Warpine's choice,
/// so a program calling a method Warpine lacks goes on.
pub const SOMERROR_NOT_IMPLEMENTED_YET_WARN: u32 = 20171;
pub const SOMERROR_OUT_OF_MEMORY: u32 = 20099;
pub const SOMERROR_BAD_ARGUMENT: u32 = 20199;
pub const SOMERROR_NO_PARENT_CLASS: u32 = 20219;
pub const SOMERROR_NO_META_CLASS: u32 = 20229;
/// `somTest`'s failure: `SOMERROR_FailedTest` with the given severity.
const SOMERROR_FAILED_TEST_BASE: u32 = 20110;

/// Bytes of data export `ord`: a root class's `ClassData` (class object,
/// then a token per method of its own), a `CClassData`, else one word (a
/// runtime variable, `SOMClassMgrObject`).
fn data_export_size(ord: u32) -> u32 {
    match ord {
        SOM_OBJECT_CLASS_DATA => 4 + 4 * SOMOBJECT_METHODS.len() as u32,
        SOM_CLASS_CLASS_DATA => 4 + 4 * SOMCLASS_METHODS.len() as u32,
        SOM_CLASS_MGR_CLASS_DATA => 4 + 4 * SOMCLASSMGR_METHODS.len() as u32,
        SOM_OBJECT_CCLASS_DATA => SOMOBJECT_CCLASS_DATA_SIZE,
        SOM_CLASS_CCLASS_DATA | SOM_CLASS_MGR_CCLASS_DATA => CCLASS_DATA_SIZE,
        _ => 4,
    }
}

/// The offsets in the exported data block of data exports
/// `SOM_FIRST_DATA..=SOM_LAST_DATA`, in ordinal order.
fn data_offsets() -> [u32; DATA_EXPORTS] {
    let mut offs = [0u32; DATA_EXPORTS];
    let mut at = 0;
    for (i, o) in offs.iter_mut().enumerate() {
        *o = at;
        at += data_export_size(SOM_FIRST_DATA + i as u32);
    }
    offs
}

/// The offset of data export `ord` in the data block.
fn data_offset(ord: u32) -> u32 {
    data_offsets()[(ord - SOM_FIRST_DATA) as usize]
}

/// Bytes of the exported data block.
fn data_size() -> u32 {
    data_offset(SOM_LAST_DATA) + data_export_size(SOM_LAST_DATA)
}

/// Format `fmt` as C's `printf` does, taking each argument word from
/// `args` (doubles and `long long`s take two) and strings through
/// `string_at`. Conversions `d i u o x X c s p f F e E g G %`, flags,
/// width, precision and `h` / `l` / `ll` / `L` lengths; output at most
/// [`MAX_PRINT`] bytes.
pub fn c_format(fmt: &[u8], args: &mut dyn Iterator<Item = u32>, string_at: &dyn Fn(u32) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut next = || args.next().unwrap_or(0);
    let mut i = 0;
    while i < fmt.len() && out.len() < MAX_PRINT {
        let b = fmt[i];
        i += 1;
        if b != b'%' { out.push(b); continue; }
        let (mut left, mut zero, mut plus, mut space, mut alt) = (false, false, false, false, false);
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => left = true,
                b'0' => zero = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                _ => break,
            }
            i += 1;
        }
        let number = |i: &mut usize, next: &mut dyn FnMut() -> u32| -> Option<i64> {
            if fmt.get(*i) == Some(&b'*') { *i += 1; return Some(next() as i32 as i64); }
            let start = *i;
            let mut v = 0i64;
            while let Some(d) = fmt.get(*i).filter(|d| d.is_ascii_digit()) {
                v = (v * 10 + (d - b'0') as i64).min(4096);
                *i += 1;
            }
            (*i > start).then_some(v)
        };
        let mut width = number(&mut i, &mut next).unwrap_or(0);
        if width < 0 { left = true; width = -width; }
        let width = width.min(4096) as usize;
        let precision = if fmt.get(i) == Some(&b'.') {
            i += 1;
            let p = number(&mut i, &mut next).unwrap_or(0);
            (p >= 0).then_some(p.min(4096) as usize)
        } else { None };
        let (mut half, mut long2) = (false, false);
        while let Some(&l) = fmt.get(i) {
            match l {
                b'h' => half = true,
                b'l' if fmt.get(i + 1) == Some(&b'l') => { long2 = true; i += 1; }
                b'l' => {}
                b'L' => long2 = true,
                _ => break,
            }
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { out.push(b'%'); break };
        i += 1;
        let wide = |next: &mut dyn FnMut() -> u32| -> u64 {
            if long2 { let lo = next() as u64; lo | (next() as u64) << 32 } else { next() as u64 }
        };
        let (prefix, body): (Vec<u8>, Vec<u8>) = match conv {
            b'd' | b'i' => {
                let w = wide(&mut next);
                let v = if long2 { w as i64 } else if half { w as i16 as i64 } else { w as i32 as i64 };
                let sign = if v < 0 { "-" } else if plus { "+" } else if space { " " } else { "" };
                (sign.into(), digits(v.unsigned_abs().to_string(), precision))
            }
            b'u' | b'o' | b'x' | b'X' => {
                let w = wide(&mut next);
                let v = if long2 { w } else if half { w as u16 as u64 } else { w as u32 as u64 };
                let (s, pre) = match conv {
                    b'o' => (format!("{v:o}"), if alt { "0" } else { "" }),
                    b'x' => (format!("{v:x}"), if alt && v != 0 { "0x" } else { "" }),
                    b'X' => (format!("{v:X}"), if alt && v != 0 { "0X" } else { "" }),
                    _ => (v.to_string(), ""),
                };
                (pre.into(), digits(s, precision))
            }
            b'p' => (Vec::new(), format!("{:08X}", next()).into_bytes()),
            b'c' => (Vec::new(), vec![next() as u8]),
            b's' => {
                let p = next();
                let mut s = if p == 0 { b"(null)".to_vec() } else { string_at(p) };
                if let Some(n) = precision { s.truncate(n); }
                (Vec::new(), s)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let lo = next() as u64;
                let v = f64::from_bits(lo | (next() as u64) << 32);
                let sign = if v.is_sign_negative() { "-" } else if plus { "+" } else if space { " " } else { "" };
                (sign.into(), float(v.abs(), conv, precision.unwrap_or(6), alt).into_bytes())
            }
            b'%' => { out.push(b'%'); continue; }
            b'n' => { next(); continue; }
            other => { out.extend([b'%', other]); continue; }
        };
        let pad = width.saturating_sub(prefix.len() + body.len());
        let numeric = !matches!(conv, b's' | b'c' | b'p');
        let floating = matches!(conv, b'f' | b'F' | b'e' | b'E' | b'g' | b'G');
        if left {
            out.extend(&prefix);
            out.extend(&body);
            out.extend(std::iter::repeat_n(b' ', pad));
        } else if zero && numeric && (precision.is_none() || floating) {
            out.extend(&prefix);
            out.extend(std::iter::repeat_n(b'0', pad));
            out.extend(&body);
        } else {
            out.extend(std::iter::repeat_n(b' ', pad));
            out.extend(&prefix);
            out.extend(&body);
        }
    }
    out.truncate(MAX_PRINT);
    out
}

/// Integer digits with at least `precision` of them (`.0` of 0: none).
fn digits(s: String, precision: Option<usize>) -> Vec<u8> {
    match precision {
        Some(0) if s == "0" => Vec::new(),
        Some(p) if p > s.len() => format!("{}{s}", "0".repeat(p - s.len())).into_bytes(),
        _ => s.into_bytes(),
    }
}

/// A non-negative double as `%f`, `%e` or `%g` (upper case for `F E G`).
fn float(v: f64, conv: u8, precision: usize, alt: bool) -> String {
    if !v.is_finite() {
        let s = if v.is_nan() { "nan" } else { "inf" };
        return if conv.is_ascii_uppercase() { s.to_uppercase() } else { s.into() };
    }
    let exp = |v: f64, p: usize| -> String {
        let s = format!("{v:.p$e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs())
    };
    let s = match conv.to_ascii_lowercase() {
        b'f' => format!("{v:.precision$}"),
        b'e' => exp(v, precision),
        _ => {
            let p = precision.max(1);
            let x: i64 = format!("{v:.*e}", p - 1).split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0);
            let s = if x < p as i64 && x >= -4 {
                format!("{v:.*}", (p as i64 - 1 - x).max(0) as usize)
            } else {
                exp(v, p - 1)
            };
            if alt { s } else {
                let (m, e) = s.split_at(s.find('e').unwrap_or(s.len()));
                let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
                format!("{m}{e}")
            }
        }
    };
    if conv.is_ascii_uppercase() { s.to_uppercase() } else { s }
}

impl Loader {
    /// `SOMMalloc`: a zeroed block from the runtime's heap.
    fn som_malloc(&self, size: u32) -> Option<u32> {
        let rounded = SomHeap::rounded(size)?;
        let at = {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.heap.blocks.len() >= MAX_BLOCKS { return None; }
            let at = if rounded > SMALL_MAX {
                let at = self.shared.mem_mgr.lock_or_recover().alloc(rounded)?;
                rt.heap.big.insert(at);
                at
            } else if let Some(at) = rt.heap.take(rounded) {
                at
            } else {
                let arena = self.shared.mem_mgr.lock_or_recover().alloc(ARENA_SIZE)?;
                rt.heap.add_arena(arena);
                rt.heap.take(rounded)?
            };
            rt.heap.blocks.insert(at, rounded);
            at
        };
        if let Some(m) = self.guest_slice_mut(at, rounded as usize) { m.fill(0); }
        Some(at)
    }

    /// `SOMFree`: a block back to the heap (others are ignored).
    fn som_free_mem(&self, at: u32) -> bool {
        let mut rt = self.shared.som.lock_or_recover();
        let Some(size) = rt.heap.blocks.remove(&at) else { return false };
        if rt.heap.big.remove(&at) {
            self.shared.mem_mgr.lock_or_recover().free(at);
        } else {
            rt.heap.free.entry(size).or_default().push(at);
        }
        true
    }

    /// `SOMRealloc`: a block of `size` bytes holding what `at` held.
    fn som_realloc(&self, at: u32, size: u32) -> u32 {
        if at == 0 { return self.som_malloc(size).unwrap_or(0); }
        let Some(old) = self.shared.som.lock_or_recover().heap.blocks.get(&at).copied() else { return 0 };
        if size == 0 { self.som_free_mem(at); return 0; }
        let Some(new) = self.som_malloc(size) else { return 0 };
        if let Some(bytes) = self.guest_read_bytes(at, old.min(size) as usize) {
            let _ = self.guest_write_bytes(new, &bytes);
        }
        self.som_free_mem(at);
        new
    }

    /// A NUL-terminated guest copy of `s`.
    fn som_string(&self, s: &str) -> u32 {
        let Some(at) = self.som_malloc(s.len() as u32 + 1) else { return 0 };
        let _ = self.guest_write_bytes(at, s.as_bytes());
        at
    }

    /// The NUL-terminated bytes at `ptr` (at most `max`).
    fn som_bytes(&self, ptr: u32, max: usize) -> Option<Vec<u8>> {
        if ptr == 0 { return None; }
        let mut out = Vec::new();
        for i in 0..=max as u32 {
            match self.guest_read::<u8>(ptr.checked_add(i)?)? {
                0 => return Some(out),
                b => out.push(b),
            }
        }
        None
    }

    /// The name at `ptr` (at most [`MAX_NAME`] bytes).
    fn som_name(&self, ptr: u32) -> Option<String> {
        self.som_bytes(ptr, MAX_NAME).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The string of `somId` `id` (a pointer to the string's pointer).
    fn som_id_name(&self, id: u32) -> Option<String> {
        if id == 0 { return None; }
        self.som_name(self.guest_read::<u32>(id)?)
    }

    /// Write class `cls`'s method table into guest memory.
    /// IBM's layout (`somMethodTab`, ADR-0072): the header, then the
    /// entries; `classInfo`, the protected data and embedded objects are 0.
    fn som_write_mtab(&self, cls: u32) -> Option<u32> {
        let (procs, name_addr, instance_size) = {
            let rt = self.shared.som.lock_or_recover();
            let c = rt.classes.get(&cls)?;
            (c.slots.iter().map(|s| s.proc).collect::<Vec<u32>>(), c.name_addr, c.instance_size)
        };
        let size = MTAB_HEADER_SIZE.checked_add(4u32.checked_mul(procs.len() as u32)?)?;
        let at = self.som_malloc(size)?;
        for (i, v) in [cls, 0, name_addr, instance_size, DATA_ALIGNMENT, size].into_iter().enumerate() {
            self.guest_write::<u32>(at + 4 * i as u32, v)?;
        }
        for (i, p) in procs.iter().enumerate() {
            self.guest_write::<u32>(at + MTAB_HEADER_SIZE + 4 * i as u32, *p)?;
        }
        let mut rt = self.shared.som.lock_or_recover();
        if let Some(c) = rt.classes.get_mut(&cls) { (c.mtab, c.ready) = (at, true); }
        rt.mtabs.insert(at, cls);
        Some(at)
    }

    /// Start the runtime once: the root classes, their method tables and
    /// class objects, the class manager, the global environment and the
    /// exported data.
    pub(crate) fn som_init(&self) {
        {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.initialized { return; }
            rt.initialized = true;
        }
        let (Some(o), Some(c), Some(m), Some(mgr), Some(env), Some(data), Some(parent_list), Some(pmtabs)) = (
            self.som_malloc(4), self.som_malloc(4), self.som_malloc(4),
            self.som_malloc(4), self.som_malloc(ENVIRONMENT_SIZE), self.som_malloc(data_size()),
            self.som_malloc(SOM_METHOD_TAB_LIST_SIZE), self.som_malloc(3 * PARENT_MTAB_STRUCT_SIZE),
        ) else {
            log::warn!("SOM: no memory for the runtime");
            return;
        };
        let names: Vec<u32> = ["SOMObject", "SOMClass", "SOMClassMgr"].iter().map(|n| self.som_string(n)).collect();
        {
            let mut rt = self.shared.som.lock_or_recover();
            let mut root = Vec::new();
            for &(ord, n) in SOMOBJECT_METHODS {
                let token = rt.new_token(o, n).unwrap_or(0);
                root.push(Slot { token, proc: root_proc(ord) });
            }
            let mut with = |cls: u32, own: &[(u32, &str)]| -> Vec<Slot> {
                let mut slots = root.clone();
                for &(ord, n) in own { slots.push(Slot { token: rt.new_token(cls, n).unwrap_or(0), proc: root_proc(ord) }); }
                slots
            };
            let cls_slots = with(c, SOMCLASS_METHODS);
            let mgr_slots = with(m, SOMCLASSMGR_METHODS);
            for (obj, name, name_addr, parents, slots, own) in [
                (o, "SOMObject", names[0], vec![], root.clone(), SOMOBJECT_METHODS.len()),
                (c, "SOMClass", names[1], vec![o], cls_slots, SOMCLASS_METHODS.len()),
                (m, "SOMClassMgr", names[2], vec![o], mgr_slots, SOMCLASSMGR_METHODS.len()),
            ] {
                // SOMObject's "data" is the method-table word.
                let parts = if obj == o { vec![(o, 0)] } else { vec![(o, 0), (obj, 4)] };
                rt.classes.insert(obj, SomClass {
                    name: name.to_string(), name_addr, parents, meta: c, slots, own_methods: own, parts,
                    data_size: if obj == o { 4 } else { 0 }, instance_size: 4, mtab: 0, ready: false,
                    version: ROOT_VERSION,
                });
                rt.by_name.insert(name.to_string(), obj);
            }
            (rt.som_object, rt.som_class, rt.som_class_mgr, rt.mgr, rt.env, rt.data) = (o, c, m, mgr, env, data);
        }
        let mtabs: Vec<Option<u32>> = [o, c, m].iter().map(|&k| self.som_write_mtab(k)).collect();
        let (Some(obj_mtab), Some(cls_mtab), Some(mgr_mtab)) = (mtabs[0], mtabs[1], mtabs[2]) else { return };
        // Class objects are instances of SOMClass; the manager of SOMClassMgr.
        for k in [o, c, m] { let _ = self.guest_write::<u32>(k, cls_mtab); }
        let _ = self.guest_write::<u32>(mgr, mgr_mtab);
        self.shared.som.lock_or_recover().instances.extend([o, c, m, mgr]);
        // The exported data: ClassData and CClassData of the root
        // classes, the manager, the runtime variables.
        let tokens = |cls: u32| -> Vec<u32> {
            let rt = self.shared.som.lock_or_recover();
            (1..=rt.methods.len() as u32).filter(|&t| rt.methods[t as usize - 1].0 == cls).collect()
        };
        for (ord, cls) in [(SOM_OBJECT_CLASS_DATA, o), (SOM_CLASS_CLASS_DATA, c), (SOM_CLASS_MGR_CLASS_DATA, m)] {
            let base = data + data_offset(ord);
            let _ = self.guest_write::<u32>(base, cls);
            for (i, t) in tokens(cls).into_iter().enumerate() {
                let _ = self.guest_write::<u32>(base + 4 + 4 * i as u32, t);
            }
        }
        // Each root class's somParentMtabStruct: its own method table,
        // then the list of its parents' (SOMObject's for SOMClass and
        // SOMClassMgr, none for SOMObject), its class object and instance
        // size (ADR-0072).
        let _ = self.guest_write::<u32>(parent_list, obj_mtab);
        let _ = self.guest_write::<u32>(parent_list + 4, 0);
        for (i, (ord, own_mtab, parents, cls)) in [(SOM_OBJECT_CCLASS_DATA, obj_mtab, 0, o),
                                                  (SOM_CLASS_CCLASS_DATA, cls_mtab, parent_list, c),
                                                  (SOM_CLASS_MGR_CCLASS_DATA, mgr_mtab, parent_list, m)]
            .into_iter().enumerate()
        {
            let pm = pmtabs + i as u32 * PARENT_MTAB_STRUCT_SIZE;
            let (token, size) = self.shared.som.lock_or_recover().classes.get(&cls).map_or((0, 0), |k| {
                (if k.parts.iter().any(|&(p, _)| p == cls) { cls } else { 0 }, k.instance_size)
            });
            for (j, v) in [own_mtab, parents, cls, 0, size].into_iter().enumerate() {
                let _ = self.guest_write::<u32>(pm + 4 * j as u32, v);
            }
            let base = data + data_offset(ord);
            let _ = self.guest_write::<u32>(base, pm);
            let _ = self.guest_write::<u32>(base + 4, token);
        }
        // SOMObjectCClassData's somDispatch* procedures.
        for k in 0..6u32 {
            let at = data + data_offset(SOM_OBJECT_CCLASS_DATA) + CCLASS_DATA_SIZE + 4 * k;
            let _ = self.guest_write::<u32>(at, thunk(SOM_METHOD_NOT_IMPLEMENTED));
        }
        let _ = self.guest_write::<u32>(data + data_offset(SOM_CLASS_MGR_OBJECT), mgr);
        for (var, f) in [
            (SOM_VAR_MALLOC, SOM_MALLOC), (SOM_VAR_CALLOC, SOM_CALLOC), (SOM_VAR_REALLOC, SOM_REALLOC),
            (SOM_VAR_FREE, SOM_FREE_MEM), (SOM_VAR_OUT_CHAR, SOM_DEFAULT_OUT_CHAR),
            (SOM_VAR_LOAD_MODULE, SOM_DEFAULT_LOAD_MODULE), (SOM_VAR_DELETE_MODULE, SOM_DEFAULT_DELETE_MODULE),
            (SOM_VAR_ERROR, SOM_DEFAULT_ERROR), (SOM_VAR_CLASS_INIT_FUNC_NAME, SOM_DEFAULT_CLASS_INIT_FUNC_NAME),
        ] {
            let _ = self.guest_write::<u32>(data + data_offset(var), thunk(f));
        }
    }

    /// The address of SOM data export `ord` (`None`: not a data export).
    pub(crate) fn som_data_address(&self, ord: u32) -> Option<u32> {
        if !(SOM_FIRST_DATA..=SOM_LAST_DATA).contains(&ord) { return None; }
        self.som_init();
        let data = self.shared.som.lock_or_recover().data;
        (data != 0).then(|| data + data_offset(ord))
    }

    /// The class of object `obj` (from its method table).
    fn som_class_of(&self, obj: u32) -> Option<u32> {
        let mtab = self.guest_read::<u32>(obj)?;
        self.shared.som.lock_or_recover().mtabs.get(&mtab).copied()
    }

    /// The procedure of method `token` for object `obj`.
    fn som_resolve(&self, obj: u32, token: u32) -> u32 {
        let found = self.som_class_of(obj).and_then(|c| self.shared.som.lock_or_recover().class_method(c, token));
        found.unwrap_or_else(|| {
            log::warn!("SOM: no method {token} for object {obj:#x}");
            thunk(SOM_METHOD_ERROR)
        })
    }

    /// `somDataResolve`: where class `token`'s instance data lies in `obj`.
    fn som_data_resolve(&self, obj: u32, token: u32) -> u32 {
        let cls = self.som_class_of(obj);
        let off = cls.and_then(|c| {
            let rt = self.shared.som.lock_or_recover();
            let k = rt.classes.get(&c)?;
            // A class (its whole part), or a field inside one (ADR-0073).
            match rt.data_tokens.get(&token) {
                Some(&(part, extra)) => k.offset_of(part)?.checked_add(extra),
                None => k.offset_of(token),
            }
        });
        match off {
            Some(off) => obj.wrapping_add(off),
            None => {
                log::warn!("SOM: no instance data {token:#x} in object {obj:#x}");
                0
            }
        }
    }

    /// `somIdFromString`: the one `somId` of `name` (a pointer to a pointer
    /// to the string).
    fn som_id(&self, name: &str) -> u32 {
        if let Some(&id) = self.shared.som.lock_or_recover().ids.get(name) { return id; }
        if self.shared.som.lock_or_recover().ids.len() >= MAX_IDS { return 0; }
        let Some(id) = self.som_malloc(4 + name.len() as u32 + 1) else { return 0 };
        let _ = self.guest_write::<u32>(id, id + 4);
        let _ = self.guest_write_bytes(id + 4, name.as_bytes());
        self.shared.som.lock_or_recover().ids.insert(name.to_string(), id);
        id
    }

    /// `somIdFromString`'s result: an id of its own for the caller, which
    /// owns it and may free it with `SOMFree` (SOM Programming Reference:
    /// "Ownership of the somId returned by this function passes to the
    /// caller"). It points at the registered copy of the string, which
    /// stays when the caller's id is freed; 0 when the registry is full or
    /// memory runs out.
    fn som_caller_id(&self, name: &str) -> u32 {
        let registered = self.som_id(name);
        if registered == 0 { return 0; }
        let Some(id) = self.som_malloc(4) else { return 0 };
        let _ = self.guest_write::<u32>(id, registered + 4);
        id
    }

    /// A new instance of ready class `cls`, zeroed, its method-table word
    /// set (an instance of a metaclass is a new, uninitialized class).
    fn som_new_instance(&self, cls: u32) -> Option<u32> {
        let (size, mtab, is_class) = {
            let rt = self.shared.som.lock_or_recover();
            let c = rt.classes.get(&cls).filter(|c| c.ready)?;
            if rt.instances.len() >= MAX_INSTANCES { return None; }
            (c.instance_size, c.mtab, rt.descends(cls, rt.som_class))
        };
        let obj = self.som_malloc(size)?;
        self.guest_write::<u32>(obj, mtab)?;
        let mut rt = self.shared.som.lock_or_recover();
        rt.instances.insert(obj);
        if is_class && rt.classes.len() < MAX_CLASSES {
            rt.classes.insert(obj, SomClass { meta: cls, ..SomClass::default() });
        }
        Some(obj)
    }

    /// `somInitClass` / `somInitMIClass`: class object `cls`, not yet
    /// initialized, gets its name, parents (none: `SOMObject`) and `size`
    /// bytes of instance data.
    fn som_init_class(&self, cls: u32, name: u32, parents: &[u32], size: u32) -> u32 {
        let Some(name) = self.som_name(name) else { return 0 };
        if size > MAX_INSTANCE_DATA || parents.len() > MAX_PARENTS { return 0; }
        let name_addr = self.som_string(&name);
        let mut rt = self.shared.som.lock_or_recover();
        let mut parents: Vec<u32> = parents.iter().copied().filter(|&p| p != 0).collect();
        if parents.is_empty() { parents.push(rt.som_object); }
        if parents.iter().any(|p| !rt.classes.get(p).is_some_and(|p| p.ready)) { return 0; }
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && c.name.is_empty()) { return 0; }
        if rt.inherit(cls, &parents, size).is_none() { return 0; }
        if let Some(c) = rt.classes.get_mut(&cls) { (c.name, c.name_addr) = (name, name_addr); }
        0
    }

    /// `somAddStaticMethod(self, methodId, methodDescriptor, method,
    /// redispatchStub, applyStub)` → the method's token (0: failed).
    fn som_add_method(&self, cls: u32, id: u32, method: u32) -> u32 {
        let Some(name) = self.som_id_name(id) else { return 0 };
        let mut rt = self.shared.som.lock_or_recover();
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && !c.name.is_empty()) { return 0; }
        if let Some(t) = rt.token_of(cls, &name) {
            if let Some(s) = rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == t)) {
                s.proc = method;
            }
            return t;
        }
        if rt.classes.get(&cls).is_some_and(|c| c.slots.len() >= MAX_METHODS) { return 0; }
        let Some(token) = rt.new_token(cls, &name) else { return 0 };
        if let Some(c) = rt.classes.get_mut(&cls) {
            c.slots.push(Slot { token, proc: method });
            c.own_methods += 1;
        }
        token
    }

    /// `somOverrideSMethod(self, methodId, method)`.
    fn som_override(&self, cls: u32, id: u32, method: u32) -> u32 {
        let Some(name) = self.som_id_name(id) else { return 0 };
        let mut rt = self.shared.som.lock_or_recover();
        if !rt.classes.get(&cls).is_some_and(|c| !c.ready && !c.name.is_empty()) { return 0; }
        match rt.token_of(cls, &name) {
            Some(t) => {
                if let Some(s) = rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == t)) {
                    s.proc = method;
                }
            }
            None => log::warn!("SOM: no method {name} to override"),
        }
        0
    }

    /// `somClassReady(self)`: the class's method table is written and it
    /// is registered with the class manager.
    fn som_class_ready(&self, cls: u32) -> u32 {
        let name = {
            let rt = self.shared.som.lock_or_recover();
            match rt.classes.get(&cls) {
                Some(c) if !c.ready && !c.name.is_empty() => c.name.clone(),
                _ => return 0,
            }
        };
        if self.som_write_mtab(cls).is_some() {
            let mut rt = self.shared.som.lock_or_recover();
            rt.by_name.insert(name, cls);
            if rt.ready_order.len() < MAX_CLASSES { rt.ready_order.push(cls); }
        }
        0
    }

    /// `somUnregisterClass`: the class manager forgets class `cls`
    /// (`somFindClass` will look for it again); 0, or 1 if it was not
    /// registered.
    fn som_unregister(&self, cls: u32) -> u32 {
        let mut rt = self.shared.som.lock_or_recover();
        let Some(name) = rt.classes.get(&cls).map(|c| c.name.clone()) else { return 1 };
        if rt.by_name.get(&name) != Some(&cls) { return 1; }
        rt.by_name.remove(&name);
        0
    }

    /// `somUnloadClassFile`: every class the module of class `cls` made is
    /// unregistered and its objects' methods resolve to nothing (their
    /// code goes with the module), then `SOMDeleteModule` frees the
    /// module. Nonzero if the class did not come from a module the class
    /// manager loaded.
    fn som_unload_class_file(&self, vcpu: u32, cls: u32) -> ApiResult {
        let hmod = {
            let mut rt = self.shared.som.lock_or_recover();
            let Some(&hmod) = rt.modules.get(&cls) else { return ApiResult::Normal(1) };
            let made: Vec<u32> = rt.modules.iter().filter(|&(_, &m)| m == hmod).map(|(&c, _)| c).collect();
            for c in made {
                rt.modules.remove(&c);
                let Some((name, mtab)) = rt.classes.get(&c).map(|k| (k.name.clone(), k.mtab)) else { continue };
                if rt.by_name.get(&name) == Some(&c) { rt.by_name.remove(&name); }
                rt.mtabs.remove(&mtab);
                rt.ready_order.retain(|&k| k != c);
            }
            hmod
        };
        let deleter = self.som_var(SOM_VAR_DELETE_MODULE);
        if deleter == thunk(SOM_DEFAULT_DELETE_MODULE) || deleter == 0 {
            return self.dos_free_module(hmod);
        }
        self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(deleter, vec![hmod], false)], false, ChainThen::ReturnLast))
    }

    /// `somBuildClass(inheritVars, sci, major, minor)`: build the class
    /// that the static class information `sci` (`somStaticClassInfo`,
    /// `somapi.h`; ADR-0073) describes, as code from IBM's SOM compiler
    /// asks for it. Its `ClassData` already naming a class: that class.
    /// Parents and an explicit metaclass are found by name; the method,
    /// override, data, procedure and `va_list` tables are applied, writing
    /// the tokens and entries into the guest's `ClassData`; the class is
    /// made ready and its `ClassData` / `CClassData` filled; its class
    /// init function, if any, is called last. Every count and pointer
    /// comes from the guest and is bounded before use.
    fn som_build_class(&self, vcpu: u32, _inherit_vars: u32, sci: u32, version: (u32, u32)) -> ApiResult {
        let ok = ApiResult::Normal;
        let Some(info) = self.som_read_sci(sci) else {
            return self.som_raise(vcpu, SOMERROR_BAD_ARGUMENT, 0, 0, 0);
        };
        let existing = self.guest_read::<u32>(info.cds).unwrap_or(0);
        if existing != 0 { return ok(existing); }
        let Some(name_ptr) = self.guest_read::<u32>(info.class_id).filter(|&p| p != 0) else {
            return self.som_raise(vcpu, SOMERROR_BAD_ARGUMENT, 0, 0, 0);
        };
        // Parents and metaclass, by name.
        let mut parents = Vec::new();
        for i in 0..info.num_parents {
            let id = info.parents.checked_add(4 * i).and_then(|at| self.guest_read::<u32>(at)).unwrap_or(0);
            match self.som_class_named(id) {
                Some(p) => parents.push(p),
                None => return self.som_raise(vcpu, SOMERROR_NO_PARENT_CLASS, 0, 0, 0),
            }
        }
        let requested = if info.explicit_meta_id == 0 { 0 } else {
            match self.som_class_named(info.explicit_meta_id) {
                Some(m) => m,
                None => return self.som_raise(vcpu, SOMERROR_NO_META_CLASS, 0, 0, 0),
            }
        };
        let meta = self.som_metaclass_for(requested, parents.first().copied().unwrap_or(0));
        if meta == 0 { return self.som_raise(vcpu, SOMERROR_NO_META_CLASS, 0, 0, 0); }
        let Some(cls) = self.som_new_instance(meta) else {
            return self.som_raise(vcpu, SOMERROR_OUT_OF_MEMORY, 0, 0, 0);
        };
        self.som_init_class(cls, name_ptr, &parents, info.instance_data_size);
        if !self.shared.som.lock_or_recover().classes.get(&cls).is_some_and(|c| !c.name.is_empty()) {
            return self.som_raise(vcpu, SOMERROR_BAD_ARGUMENT, 0, 0, 0);
        }
        self.som_apply_sci(cls, &info, &parents);
        self.som_class_ready(cls);
        let size = {
            let mut rt = self.shared.som.lock_or_recover();
            let Some(c) = rt.classes.get_mut(&cls) else { return ok(0) };
            c.version = version;
            c.instance_size
        };
        // ClassData.classObject, CClassData.parentMtab / instanceDataToken.
        let _ = self.guest_write::<u32>(info.cds, cls);
        if info.ccds != 0 {
            let pm = self.som_parent_mtab_struct(cls, &parents, size);
            let token = if info.instance_data_size > 0 { cls } else { 0 };
            let _ = self.guest_write::<u32>(info.ccds, pm);
            let _ = self.guest_write::<u32>(info.ccds.wrapping_add(4), token);
        }
        if info.cif == 0 { return ok(cls); }
        let step = CallStep::hook(info.cif, vec![cls], false);
        self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::Return(cls)))
    }

    /// The registered class the somId `id` names.
    fn som_class_named(&self, id: u32) -> Option<u32> {
        let name = self.som_id_name(id)?;
        let rt = self.shared.som.lock_or_recover();
        rt.by_name.get(&name).copied().filter(|c| rt.classes.get(c).is_some_and(|c| c.ready))
    }

    /// The fields of the `somStaticClassInfo` at `sci` this runtime uses,
    /// read and bounded (`None`: unreadable, an unknown layout version or
    /// no `ClassData`).
    fn som_read_sci(&self, sci: u32) -> Option<SciInfo> {
        let word = |i: u32| sci.checked_add(4 * i).and_then(|at| self.guest_read::<u32>(at));
        let layout = word(SCI_LAYOUT_VERSION)?;
        if !(1..=SOM_SCI_LEVEL).contains(&layout) { return None; }
        let count = |i: u32, max: usize| word(i).map(|n| n.min(max as u32));
        let v3 = layout >= 3;
        let info = SciInfo {
            num_static_methods: count(SCI_NUM_STATIC_METHODS, MAX_METHODS)?,
            num_overrides: count(SCI_NUM_STATIC_OVERRIDES, MAX_METHODS)?,
            num_non_internal_data: count(SCI_NUM_NON_INTERNAL_DATA, MAX_METHODS)?,
            num_proc_methods: count(SCI_NUM_PROC_METHODS, MAX_METHODS)?,
            num_varargs_funcs: count(SCI_NUM_VARARGS_FUNCS, MAX_METHODS)?,
            instance_data_size: word(SCI_INSTANCE_DATA_SIZE)?.min(MAX_INSTANCE_DATA),
            num_parents: count(SCI_NUM_PARENTS, MAX_PARENTS)?,
            class_id: word(SCI_CLASS_ID)?,
            explicit_meta_id: word(SCI_EXPLICIT_META_ID)?,
            parents: word(SCI_PARENTS)?,
            cds: word(SCI_CDS).filter(|&p| p != 0)?,
            ccds: word(SCI_CCDS)?,
            smt: word(SCI_SMT)?,
            omt: word(SCI_OMT)?,
            nit_reference_base: word(SCI_NIT_REFERENCE_BASE)?,
            nit: word(SCI_NIT)?,
            pmt: word(SCI_PMT)?,
            vft: word(SCI_VFT)?,
            cif: word(SCI_CIF)?,
            num_methods: if v3 { count(SCI_NUM_METHODS, MAX_METHODS)? } else { 0 },
            mt: if v3 { word(SCI_MT)? } else { 0 },
            num_inherited_methods: if v3 { count(SCI_NUM_INHERITED_METHODS, MAX_METHODS)? } else { 0 },
            imt: if v3 { word(SCI_IMT)? } else { 0 },
        };
        Some(info)
    }

    /// Apply the tables of `info` to class `cls` (initialized, not ready):
    /// static methods (tokens written into `ClassData`), general methods,
    /// overrides, inherited implementations, non-internal data tokens,
    /// procedure and `va_list` entries.
    fn som_apply_sci(&self, cls: u32, info: &SciInfo, parents: &[u32]) {
        let word = |at: u32| self.guest_read::<u32>(at).unwrap_or(0);
        // Word `field` of entry `i` of a table of `size`-byte entries.
        let entry = |table: u32, size: u32, i: u32, field: u32| {
            i.checked_mul(size).and_then(|o| table.checked_add(o)?.checked_add(4 * field)).map_or(0, word)
        };
        // somStaticMethod_t: classData, methodId (somId *), descriptor,
        // method, redispatch stub, apply stub.
        for i in 0..info.num_static_methods {
            let (slot, id_ptr, method) = (entry(info.smt, 24, i, 0), entry(info.smt, 24, i, 1), entry(info.smt, 24, i, 3));
            let token = self.som_add_method(cls, word(id_ptr), method);
            if slot != 0 && token != 0 { let _ = self.guest_write::<u32>(slot, token); }
        }
        // somMethods_t (layout 3): type, classData, methodId, descriptor,
        // method, stubs. Static, dynamic and nonstatic methods are added
        // as static ones; an override by token replaces that method.
        for i in 0..info.num_methods {
            let kind = entry(info.mt, 28, i, 0) & 0xFF;
            let (slot, id_ptr, method) = (entry(info.mt, 28, i, 1), entry(info.mt, 28, i, 2), entry(info.mt, 28, i, 4));
            match kind {
                SCI_MT_STATIC | SCI_MT_DYNAMIC | SCI_MT_NONSTATIC => {
                    let token = self.som_add_method(cls, word(id_ptr), method);
                    if slot != 0 && token != 0 { let _ = self.guest_write::<u32>(slot, token); }
                }
                SCI_MT_TOKEN_OVERRIDE => self.som_override_token(cls, word(id_ptr), method),
                other => log::debug!("SOM: somBuildClass method action {other} not used"),
            }
        }
        // somOverrideMethod_t: methodId (somId *, maybe "Class::method"),
        // method.
        for i in 0..info.num_overrides {
            let (id_ptr, method) = (entry(info.omt, 8, i, 0), entry(info.omt, 8, i, 1));
            if let Some(name) = self.som_id_name(word(id_ptr)) {
                let simple = name.rsplit("::").next().unwrap_or(&name).to_string();
                self.som_override(cls, self.som_id(&simple), method);
            }
        }
        // somInheritedMethod_t: methodId, parentNum (from 1), mToken —
        // that parent's implementation of the method.
        for i in 0..info.num_inherited_methods {
            let (id_ptr, n) = (entry(info.imt, 12, i, 0), entry(info.imt, 12, i, 1));
            let Some(&parent) = n.checked_sub(1).and_then(|k| parents.get(k as usize)) else { continue };
            let Some(name) = self.som_id_name(word(id_ptr)) else { continue };
            let mut rt = self.shared.som.lock_or_recover();
            let (Some(t), Some(pt)) = (rt.token_of(cls, &name), rt.token_of(parent, &name)) else { continue };
            let Some(proc) = rt.class_method(parent, pt) else { continue };
            if let Some(s) = rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == t)) {
                s.proc = proc;
            }
        }
        // somNonInternalData_t: classData (somDToken *), the field's
        // address in a model of the instance data (nitReferenceBase).
        for i in 0..info.num_non_internal_data {
            let (slot, basis) = (entry(info.nit, 8, i, 0), entry(info.nit, 8, i, 1));
            let Some(off) = basis.checked_sub(info.nit_reference_base).filter(|&o| o < info.instance_data_size.max(1)) else { continue };
            let token = self.som_data_token(cls, off);
            if slot != 0 && token != 0 { let _ = self.guest_write::<u32>(slot, token); }
        }
        // somProcMethods_t and somVarargsFuncs_t: an entry written into
        // the ClassData member they point to.
        for (table, n) in [(info.pmt, info.num_proc_methods), (info.vft, info.num_varargs_funcs)] {
            for i in 0..n {
                let (slot, value) = (entry(table, 8, i, 0), entry(table, 8, i, 1));
                if slot != 0 { let _ = self.guest_write::<u32>(slot, value); }
            }
        }
    }

    /// Override the method that `token` names in class `cls` (not ready).
    fn som_override_token(&self, cls: u32, token: u32, method: u32) {
        let mut rt = self.shared.som.lock_or_recover();
        match rt.classes.get_mut(&cls).and_then(|c| c.slots.iter_mut().find(|s| s.token == token)) {
            Some(s) => s.proc = method,
            None => log::warn!("SOM: no method {token} to override"),
        }
    }

    /// A data token for the field at `offset` in class `cls`'s part.
    fn som_data_token(&self, cls: u32, offset: u32) -> u32 {
        if self.shared.som.lock_or_recover().data_tokens.len() >= MAX_TOKENS { return 0; }
        let Some(token) = self.som_malloc(8) else { return 0 };
        let _ = self.guest_write::<u32>(token, cls);
        let _ = self.guest_write::<u32>(token + 4, offset);
        self.shared.som.lock_or_recover().data_tokens.insert(token, (cls, offset));
        token
    }

    /// A `somParentMtabStruct` for ready class `cls`: its own method
    /// table, a list of its parents' tables, its class object and
    /// instance size (the rest 0).
    fn som_parent_mtab_struct(&self, cls: u32, parents: &[u32], size: u32) -> u32 {
        let mtabs: Vec<u32> = {
            let rt = self.shared.som.lock_or_recover();
            let parents = if parents.is_empty() { vec![rt.som_object] } else { parents.to_vec() };
            std::iter::once(cls).chain(parents).map(|c| rt.classes.get(&c).map_or(0, |k| k.mtab)).collect()
        };
        let Some(pm) = self.som_malloc(PARENT_MTAB_STRUCT_SIZE) else { return 0 };
        // The list after the class's own table, last entry first.
        let mut next = 0;
        for &m in mtabs[1..].iter().rev() {
            let Some(node) = self.som_malloc(SOM_METHOD_TAB_LIST_SIZE) else { return 0 };
            let _ = self.guest_write::<u32>(node, m);
            let _ = self.guest_write::<u32>(node + 4, next);
            next = node;
        }
        for (j, v) in [mtabs[0], next, cls, 0, size].into_iter().enumerate() {
            let _ = self.guest_write::<u32>(pm + 4 * j as u32, v);
        }
        pm
    }

    /// `WrpSomMetaclassFor(requested, parent)`: the metaclass of a new
    /// class with parent `parent` (0: `SOMObject`) that asked for
    /// `requested` (0: none); 0 if `requested` is no metaclass.
    fn som_metaclass_for(&self, requested: u32, parent: u32) -> u32 {
        let (requested, parent_meta, som_class) = {
            let rt = self.shared.som.lock_or_recover();
            let parent = if parent == 0 { rt.som_object } else { parent };
            let parent_meta = rt.classes.get(&parent).map_or(rt.som_class, |p| p.meta);
            (if requested == 0 { parent_meta } else { requested }, parent_meta, rt.som_class)
        };
        let names = {
            let rt = self.shared.som.lock_or_recover();
            let ready = |c: u32| rt.classes.get(&c).is_some_and(|c| c.ready);
            if !ready(requested) || !ready(parent_meta) || !rt.descends(requested, som_class) { return 0; }
            if rt.descends(requested, parent_meta) { return requested; }
            if rt.descends(parent_meta, requested) { return parent_meta; }
            if let Some(&d) = rt.derived.get(&(requested, parent_meta)) { return d; }
            if rt.derived.len() >= MAX_CLASSES { return 0; }
            [requested, parent_meta].map(|c| rt.classes.get(&c).map(|c| c.name.clone()).unwrap_or_default())
        };
        // A derived metaclass: both as parents, no data of its own.
        let Some(d) = self.som_new_instance(som_class) else { return 0 };
        let name = format!("{}+{}", names[0], names[1]);
        let name_addr = self.som_string(&name);
        {
            let mut rt = self.shared.som.lock_or_recover();
            if rt.inherit(d, &[requested, parent_meta], 0).is_none() { return 0; }
            if let Some(c) = rt.classes.get_mut(&d) { (c.name, c.name_addr) = (name, name_addr); }
            rt.derived.insert((requested, parent_meta), d);
        }
        self.som_class_ready(d);
        d
    }

    /// `somPrintf(fmt, …)`: formatted to standard output; at most 9
    /// argument words.
    fn som_printf(&self, vcpu: u32, a: &[u32; 10]) -> ApiResult {
        if a[0] == 0 { return ApiResult::Normal(0); }
        let out = self.som_format(a[0], &mut a[1..].iter().copied());
        self.som_output(vcpu, out)
    }

    /// The guest format string at `fmt` formatted with `args`; empty when
    /// it cannot be read.
    fn som_format(&self, fmt: u32, args: &mut dyn Iterator<Item = u32>) -> Vec<u8> {
        let Some(fmt) = self.som_bytes(fmt, MAX_PRINT) else { return Vec::new() };
        let string_at = |p: u32| self.som_bytes(p, MAX_PRINT).unwrap_or_default();
        let mut out = c_format(&fmt, args, &string_at);
        out.truncate(MAX_PRINT);
        out
    }

    /// Write `out` through `SOMOutCharRoutine`; the count of characters.
    fn som_output(&self, vcpu: u32, out: Vec<u8>) -> ApiResult {
        // A program's own SOMOutCharRoutine gets every character.
        let routine = self.som_var(SOM_VAR_OUT_CHAR);
        if out.is_empty() { return ApiResult::Normal(0); }
        if routine == thunk(SOM_DEFAULT_OUT_CHAR) || routine == 0 {
            return ApiResult::Normal(self.write_handle(STDOUT_HANDLE, &out).unwrap_or(0));
        }
        let steps = out.iter().map(|&c| CallStep::hook(routine, vec![c as u32], false)).collect();
        self.start_chain(CallChain::new(vcpu, steps, false, ChainThen::Return(out.len() as u32)))
    }

    /// `somParentNumResolve(parentMtab, n, token)`: the procedure of
    /// method `token` in the `n`th parent's method table (from 1) of the
    /// list `parentMtab` — a `somParentMtabStruct`, whose head is the
    /// class's own table and whose `next` entries are its parents'
    /// (ADR-0072). `somParentResolve` is `n` = 1. The table's class is
    /// answered from the runtime's records, so a guest's table contents
    /// cannot steer it.
    fn som_parent_resolve(&self, parent_mtab: u32, n: u32, token: u32) -> u32 {
        let mut node = parent_mtab;
        for _ in 0..n.min(MAX_PARENTS as u32 + 1) {
            if node == 0 { break; }
            node = self.guest_read::<u32>(node.wrapping_add(4)).unwrap_or(0);
        }
        let mtab = if node == 0 || n == 0 || n > MAX_PARENTS as u32 { None } else { self.guest_read::<u32>(node) };
        let rt = self.shared.som.lock_or_recover();
        mtab.and_then(|m| rt.mtabs.get(&m).copied())
            .and_then(|cls| rt.class_method(cls, token))
            .unwrap_or_else(|| thunk(SOM_METHOD_ERROR))
    }

    /// The procedure of the method named by somId `id` that class `cls`
    /// supports.
    fn som_find_method(&self, cls: u32, id: u32) -> Option<u32> {
        let name = self.som_id_name(id)?;
        let rt = self.shared.som.lock_or_recover();
        let token = rt.token_of(cls, &name)?;
        rt.class_method(cls, token)
    }

    /// Raise SOM error `code` through `SOMError` (a program's own, else
    /// the default: a message, and the end of the process for a fatal
    /// code), then return `result`. `file` 0 names the SOM kernel.
    fn som_raise(&self, vcpu: u32, code: u32, file: u32, line: u32, result: u32) -> ApiResult {
        let routine = self.som_var(SOM_VAR_ERROR);
        if routine == 0 || routine == thunk(SOM_DEFAULT_ERROR) {
            return match self.som_default_error(vcpu, code, file, line) {
                ApiResult::Normal(_) => ApiResult::Normal(result),
                other => other,
            };
        }
        let file = if file == 0 { self.som_kernel_file() } else { file };
        let step = CallStep::hook(routine, vec![code, file, line], false);
        self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::Return(result)))
    }

    /// The default `SOMError(code, file, line)`: a message on standard
    /// error; a fatal code (last digit 9) ends the process.
    fn som_default_error(&self, vcpu: u32, code: u32, file: u32, line: u32) -> ApiResult {
        let file = if file == 0 { SOM_KERNEL_FILE.to_string() } else { self.som_name(file).unwrap_or_default() };
        let msg = format!("SOM error {} in file {file}, line {}\n", code as i32, line as i32);
        let _ = self.write_handle(super::constants::STDERR_HANDLE, msg.as_bytes());
        if code % 10 == SOM_FATAL {
            return self.dos_exit(vcpu, super::threads::EXIT_PROCESS, code);
        }
        ApiResult::Normal(0)
    }

    /// The guest copy of the file name kernel errors give `SOMError`.
    fn som_kernel_file(&self) -> u32 {
        let known = self.shared.som.lock_or_recover().kernel_file;
        if known != 0 { return known; }
        let at = self.som_string(SOM_KERNEL_FILE);
        self.shared.som.lock_or_recover().kernel_file = at;
        at
    }

    /// `somCheckArgs(argc, argv)`: `-somt` / `-somtc` set `SOM_TraceLevel`
    /// to 1 / 2, `-soma` / `-somac` `SOM_AssertLevel` to 1 / 2, `-somw`
    /// `SOM_WarnLevel` to 1 (the flags from `somcdev.h`; the values are
    /// Warpine's choice, ADR-0072). `argv` is not changed.
    fn som_check_args(&self, argc: u32, argv: u32) {
        for i in 0..argc.min(MAX_ARGS) {
            let Some(arg) = argv.checked_add(4 * i).and_then(|at| self.guest_read::<u32>(at)) else { break };
            let Some(arg) = self.som_name(arg) else { continue };
            let (var, level) = match arg.as_str() {
                "-somt" => (SOM_TRACE_LEVEL, 1),
                "-somtc" => (SOM_TRACE_LEVEL, 2),
                "-soma" => (SOM_ASSERT_LEVEL, 1),
                "-somac" => (SOM_ASSERT_LEVEL, 2),
                "-somw" => (SOM_WARN_LEVEL, 1),
                _ => continue,
            };
            if let Some(at) = self.som_data_address(var) { let _ = self.guest_write::<u32>(at, level); }
        }
    }

    /// What the default `SOMClassInitFuncName` returns: "SOMInitModule".
    fn som_init_func_name(&self) -> u32 {
        let known = self.shared.som.lock_or_recover().init_func_name;
        if known != 0 { return known; }
        let at = self.som_string(SOM_INIT_MODULE);
        self.shared.som.lock_or_recover().init_func_name = at;
        at
    }

    /// The value of runtime variable `ord` (`SOMOutCharRoutine` …).
    fn som_var(&self, ord: u32) -> u32 {
        self.som_data_address(ord).and_then(|at| self.guest_read::<u32>(at)).unwrap_or(0)
    }

    /// Load `file` for class `class` as the default `SOMLoadModule` does:
    /// the module, the `_DLL_InitTerm`s of it and the modules loaded with
    /// it, then its initialization function `func(major, minor,
    /// class_ptr)` or, without one, `<class>NewClass(major, minor)`.
    /// Gives the module, the calls to make and how many of them are
    /// `_DLL_InitTerm`s; or an OS/2 error.
    fn som_module_steps(&self, file: &str, func: Option<&str>, class: &str, class_ptr: u32,
                        major: u32, minor: u32) -> Result<(u32, Vec<CallStep>, u32), u32> {
        let Some((module, path)) = self.find_module_file(file) else {
            log::warn!("SOM: no class file {file} for class {class}");
            return Err(super::constants::ERROR_MOD_NOT_FOUND);
        };
        let hmod = self.load_dll(&module, &path).map_err(|e| {
            log::warn!("SOM: loading {file} for class {class} failed: {e}");
            super::constants::ERROR_MOD_NOT_FOUND
        })?;
        let pending = self.shared.dll_mgr.lock_or_recover().take_pending_init();
        let mut steps: Vec<CallStep> = pending.iter().map(|&(m, e)| CallStep::hook(e, vec![m, 0], false)).collect();
        let inits = steps.len() as u32;
        let export = |name: &str| {
            let dlls = self.shared.dll_mgr.lock_or_recover();
            dlls.find_by_handle(hmod)?.exports_by_name.get(&name.to_ascii_uppercase()).copied()
        };
        if let Some(addr) = func.and_then(export) {
            steps.push(CallStep::hook(addr, vec![major, minor, class_ptr], false));
        } else if let Some(addr) = export(&format!("{class}NewClass")) {
            steps.push(CallStep::hook(addr, vec![major, minor], false));
        }
        Ok((hmod, steps, inits))
    }

    /// `somFindClass` / `somFindClsInFile` / `somLoadClassFile`: class
    /// `id` if it is registered (unless `load`), else loaded from `file`
    /// (none: the class's name) through `SOMLoadModule`.
    fn som_find_class(&self, vcpu: u32, id: u32, major: u32, minor: u32, file: Option<u32>, load: bool) -> ApiResult {
        let Some(name) = self.som_id_name(id) else { return ApiResult::Normal(0) };
        if !load && let Some(&cls) = self.shared.som.lock_or_recover().by_name.get(&name) {
            return ApiResult::Normal(cls);
        }
        let file = match file {
            Some(p) => match self.som_name(p) { Some(f) => f, None => return ApiResult::Normal(0) },
            None => name.clone(),
        };
        // A block for the strings and the module handle SOMLoadModule takes.
        let strings = [name.as_str(), file.as_str(), SOM_INIT_MODULE];
        let size = 4 + strings.iter().map(|s| s.len() as u32 + 1).sum::<u32>();
        let Some(block) = self.som_malloc(size) else { return ApiResult::Normal(0) };
        let mut at = block + 4;
        let mut ptrs = Vec::new();
        for s in strings {
            let _ = self.guest_write_bytes(at, s.as_bytes());
            let _ = self.guest_write::<u8>(at + s.len() as u32, 0);
            ptrs.push(at);
            at += s.len() as u32 + 1;
        }
        let loader = self.som_var(SOM_VAR_LOAD_MODULE);
        let first = self.shared.som.lock_or_recover().ready_order.len() as u32;
        if loader == thunk(SOM_DEFAULT_LOAD_MODULE) {
            match self.som_module_steps(&file, Some(SOM_INIT_MODULE), &name, ptrs[0], major, minor) {
                Ok((hmod, steps, inits)) => {
                    let _ = self.guest_write::<u32>(block, hmod);
                    self.start_chain(CallChain::new(vcpu, steps, false, ChainThen::SomClassLoaded { id, block, inits, first }))
                }
                Err(_) => { self.som_free_mem(block); ApiResult::Normal(0) }
            }
        } else {
            // The program's own SOMLoadModule; 0 is success, the module
            // handle in the block's first word.
            let step = CallStep::hook(loader, vec![ptrs[0], ptrs[1], ptrs[2], major, minor, block], false);
            self.start_chain(CallChain::new(vcpu, vec![step], false,
                                            ChainThen::SomClassLoaded { id, block, inits: u32::MAX, first }))
        }
    }

    /// The calls loading a class's file returned: the class, if the
    /// loading succeeded and made it (`inits` `_DLL_InitTerm`s first, or
    /// `u32::MAX`: a program's `SOMLoadModule`, its result last).
    /// The classes that became ready since `first` came from the module
    /// whose handle is in the block's first word.
    pub(crate) fn som_class_loaded(&self, id: u32, block: u32, inits: u32, first: u32, chain: &CallChain) -> u32 {
        let hmod = self.guest_read::<u32>(block).unwrap_or(0);
        self.som_free_mem(block);
        let loaded = if inits == u32::MAX { chain.last == 0 } else { som_inits_ok(&chain.results, inits) };
        if loaded && hmod != 0 {
            let mut rt = self.shared.som.lock_or_recover();
            let made: Vec<u32> = rt.ready_order.iter().skip(first as usize).copied().collect();
            for c in made { rt.modules.insert(c, hmod); }
        }
        let Some(name) = self.som_id_name(id) else { return 0 };
        let cls = self.shared.som.lock_or_recover().by_name.get(&name).copied();
        match (loaded, cls) {
            (true, Some(cls)) => cls,
            (true, None) => { log::warn!("SOM: the class file of {name} did not make the class"); 0 }
            (false, _) => { log::warn!("SOM: loading the class file of {name} failed"); 0 }
        }
    }

    /// The default `SOMLoadModule` made its calls: the module handle goes
    /// to `phandle` and 0 is returned when every `_DLL_InitTerm` succeeded.
    pub(crate) fn som_module_loaded(&self, hmod: u32, phandle: u32, inits: u32, chain: &CallChain) -> u32 {
        if !som_inits_ok(&chain.results, inits) {
            return super::constants::ERROR_INIT_ROUTINE_FAILED;
        }
        if phandle != 0 { let _ = self.guest_write::<u32>(phandle, hmod); }
        0
    }

    /// The text `somPrintSelf` and `somDumpSelf` give an object.
    fn som_describe(&self, obj: u32) -> String {
        let cls = self.som_class_of(obj).unwrap_or(0);
        let name = self.shared.som.lock_or_recover().classes.get(&cls).map(|c| c.name.clone()).unwrap_or_default();
        format!("{{An instance of class {name} at address {obj:08X}}}")
    }

    /// A SOM function or root-class method by module ordinal `ord`, with
    /// its arguments `a` (the receiver first for methods).
    pub(crate) fn som_call(&self, vcpu: u32, ord: u32, a: &[u32; 10]) -> ApiResult {
        self.som_init();
        let ok = ApiResult::Normal;
        let class_of = |obj: u32| self.som_class_of(obj).unwrap_or(0);
        let info = |cls: u32, f: &dyn Fn(&SomClass) -> u32| {
            self.shared.som.lock_or_recover().classes.get(&cls).map_or(0, f)
        };
        match ord {
            SOM_ENVIRONMENT_NEW => ok(self.shared.som.lock_or_recover().mgr),
            SOM_RESOLVE => ok(self.som_resolve(a[0], a[1])),
            SOM_RESOLVE_BY_NAME => {
                let name = self.som_name(a[1]).unwrap_or_default();
                let cls = class_of(a[0]);
                let rt = self.shared.som.lock_or_recover();
                ok(rt.token_of(cls, &name).and_then(|t| rt.class_method(cls, t)).unwrap_or(0))
            }
            SOM_CLASS_RESOLVE => ok(self.shared.som.lock_or_recover().class_method(a[0], a[1]).unwrap_or_else(|| thunk(SOM_METHOD_ERROR))),
            SOM_DATA_RESOLVE => ok(self.som_data_resolve(a[0], a[1])),
            SOM_GET_GLOBAL_ENVIRONMENT => ok(self.shared.som.lock_or_recover().env),
            SOM_ID_FROM_STRING => ok(self.som_name(a[0]).map_or(0, |n| self.som_caller_id(&n))),
            SOM_STRING_FROM_ID => ok(if a[0] == 0 { 0 } else { self.guest_read::<u32>(a[0]).unwrap_or(0) }),
            SOM_COMPARE_IDS => ok((self.som_id_name(a[0]).is_some() && self.som_id_name(a[0]) == self.som_id_name(a[1])) as u32),
            SOM_IS_OBJ => ok(self.shared.som.lock_or_recover().instances.contains(&a[0]) as u32),
            SOM_PRINTF => self.som_printf(vcpu, a),
            SOM_MALLOC => ok(self.som_malloc(a[0]).unwrap_or(0)),
            SOM_CALLOC => ok(a[0].checked_mul(a[1]).and_then(|n| self.som_malloc(n)).unwrap_or(0)),
            SOM_REALLOC => ok(self.som_realloc(a[0], a[1])),
            SOM_FREE_MEM => { self.som_free_mem(a[0]); ok(0) }
            SOM_METACLASS_FOR => ok(self.som_metaclass_for(a[0], a[1])),
            SOM_DEFAULT_LOAD_MODULE => {
                // SOMLoadModule(className, fileName, functionName, major,
                // minor, modHandle *): 0 when loaded.
                let (Some(class), Some(file)) = (self.som_name(a[0]), self.som_name(a[1])) else {
                    return ok(super::constants::ERROR_INVALID_PARAMETER);
                };
                let func = if a[2] == 0 { None } else { self.som_name(a[2]) };
                match self.som_module_steps(&file, func.as_deref(), &class, a[0], a[3], a[4]) {
                    Ok((hmod, steps, inits)) => self.start_chain(CallChain::new(vcpu, steps, false,
                        ChainThen::SomModuleLoaded { hmod, phandle: a[5], inits })),
                    Err(e) => ok(e),
                }
            }
            SOM_DEFAULT_DELETE_MODULE => self.dos_free_module(a[0]),
            // SOMError(code, fileName, lineNum).
            SOM_DEFAULT_ERROR => self.som_default_error(vcpu, a[0], a[1], a[2]),
            SOM_DEFAULT_OUT_CHAR => {
                let _ = self.write_handle(STDOUT_HANDLE, &[a[0] as u8]);
                ok(1)
            }
            SOM_SET_OUT_CHAR => {
                let routine = if a[0] == 0 { thunk(SOM_DEFAULT_OUT_CHAR) } else { a[0] };
                if let Some(at) = self.som_data_address(SOM_VAR_OUT_CHAR) { let _ = self.guest_write::<u32>(at, routine); }
                ok(0)
            }
            SOM_METHOD_ERROR => ok(0),
            // ── IBM's SOM 1.0 functions (ADR-0070) ──
            SOM_OBJECT_NEW_CLASS | SOM_CLASS_NEW_CLASS | SOM_CLASS_MGR_NEW_CLASS => {
                // `<Class>NewClass(major, minor)`: the root classes are
                // made with the runtime; every version is accepted.
                let rt = self.shared.som.lock_or_recover();
                ok(match ord {
                    SOM_OBJECT_NEW_CLASS => rt.som_object,
                    SOM_CLASS_NEW_CLASS => rt.som_class,
                    _ => rt.som_class_mgr,
                })
            }
            SOM_PARENT_RESOLVE => ok(self.som_parent_resolve(a[0], 1, a[1])),
            SOM_PARENT_NUM_RESOLVE => ok(self.som_parent_resolve(a[0], a[1], a[2])),
            SOM_BUILD_CLASS => self.som_build_class(vcpu, a[0], a[1], (a[2], a[3])),
            SOM_METHOD_NOT_IMPLEMENTED => self.som_raise(vcpu, SOMERROR_NOT_IMPLEMENTED_YET_WARN, 0, 0, 0),
            SOM_TEST_CLS => {
                // somTestCls(obj, class, file, line): the object, when it is
                // one and an instance of the class (or a descendant's);
                // else SOMERROR_TestObjectFailure (ADR-0072).
                let fine = self.som_class_of(a[0]).is_some_and(|c| self.shared.som.lock_or_recover().descends(c, a[1]));
                if fine { ok(a[0]) } else { self.som_raise(vcpu, SOMERROR_TEST_OBJECT_FAILURE, a[2], a[3], a[0]) }
            }
            SOM_TEST => {
                // somTest(condition, severity, file, line, msg).
                if a[0] != 0 { return ok(0); }
                self.som_raise(vcpu, SOMERROR_FAILED_TEST_BASE + (a[1] % 10), a[2], a[3], 0)
            }
            SOM_ASSERT => {
                // somAssert(condition, code, file, line, msg): a kept
                // assertion is noted when SOM_AssertLevel is above 0; a
                // failed one raises its code.
                if a[0] == 0 { return self.som_raise(vcpu, a[1], a[2], a[3], 0); }
                if self.som_var(SOM_ASSERT_LEVEL) as i32 <= 0 { return ok(0); }
                let msg = self.som_name(a[4]).unwrap_or_default();
                self.som_output(vcpu, format!("Assertion passed: {msg}\n").into_bytes())
            }
            SOM_CHECK_ARGS => { self.som_check_args(a[0], a[1]); ok(0) }
            SOM_CHECK_ID => {
                // Registers the id and gives it back.
                if let Some(name) = self.som_id_name(a[0]) { self.som_id(&name); }
                ok(a[0])
            }
            SOM_REGISTER_ID => {
                // TRUE when the id was not registered before.
                let Some(name) = self.som_id_name(a[0]) else { return ok(0) };
                let known = self.shared.som.lock_or_recover().ids.contains_key(&name);
                ok((!known && self.som_id(&name) != 0) as u32)
            }
            SOM_UNIQUE_KEY => ok(self.som_id_name(a[0]).map_or(0, |n| self.som_id(&n))),
            SOM_TOTAL_REG_IDS => ok(self.shared.som.lock_or_recover().ids.len() as u32),
            // Hints and intervals with no effect here.
            SOM_SET_EXPECTED_IDS | SOM_BEGIN_PERSISTENT_IDS | SOM_END_PERSISTENT_IDS => ok(0),
            SOM_PREFIX_LEVEL => self.som_output(vcpu, som_prefix(a[0])),
            SOM_L_PRINTF => {
                let mut out = som_prefix(a[0]);
                out.extend(self.som_format(a[1], &mut a[2..].iter().copied()));
                self.som_output(vcpu, out)
            }
            SOM_VPRINTF => {
                // The arguments are words in guest memory from `ap` on.
                let ap = a[1];
                let mut args = (0u32..).map_while(|i| {
                    ap.checked_add(i.checked_mul(4)?).and_then(|at| self.guest_read::<u32>(at))
                });
                let out = self.som_format(a[0], &mut args);
                self.som_output(vcpu, out)
            }
            SOM_DEFAULT_CLASS_INIT_FUNC_NAME => ok(self.som_init_func_name()),
            // Entries no source describes (metrics), and somConstructClass,
            // left to the class-building stage: accepted, no effect.
            SOM_SAVE_METRICS | SOM_WRITE_METRICS | SOM_CONSTRUCT_CLASS => {
                log::debug!("SOM: undocumented entry {ord} accepted, no effect");
                ok(0)
            }
            // ── SOMObject ──
            3200 | 3201 | 3211 => ok(0), // somInit / somUninit / somDumpSelfInt: nothing at the root
            3202 => {
                // somFree: the object's somDestruct with doFree set and a
                // null control ("somFree calls somDestruct", SOM
                // Programming Reference); the root's runs somUninit, then
                // frees the memory, as an override's chain to it does.
                if !self.shared.som.lock_or_recover().instances.contains(&a[0]) { return ok(0); }
                let destruct = self.som_resolve(a[0], TOKEN_SOM_DESTRUCT);
                if destruct != thunk(3213) {
                    return self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(destruct, vec![a[0], 1, 0], false)],
                                                           false, ChainThen::Return(0)));
                }
                let uninit = self.som_resolve(a[0], TOKEN_SOM_UNINIT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(uninit, vec![a[0]], false)], false,
                                                ChainThen::SomFree { obj: a[0] }))
            }
            3203 => ok(class_of(a[0])),
            3204 => ok(info(class_of(a[0]), &|c| c.name_addr)),
            3205 => ok(info(class_of(a[0]), &|c| c.instance_size)),
            3206 => {
                // The class first: class_of takes the runtime's lock too.
                let cls = class_of(a[0]);
                ok(self.shared.som.lock_or_recover().descends(cls, a[1]) as u32)
            }
            3207 => ok((class_of(a[0]) == a[1] && a[1] != 0) as u32),
            3208 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                let cls = class_of(a[0]);
                ok(self.shared.som.lock_or_recover().token_of(cls, &name).is_some() as u32)
            }
            3209 => {
                let _ = self.write_handle(STDOUT_HANDLE, format!("{}\n", self.som_describe(a[0])).as_bytes());
                ok(a[0])
            }
            3210 => {
                // somDumpSelf(level): a line, then the object's somDumpSelfInt.
                let indent = "  ".repeat((a[1] as i32).clamp(0, 32) as usize);
                let _ = self.write_handle(STDOUT_HANDLE, format!("{indent}{}\n", self.som_describe(a[0])).as_bytes());
                let int = self.som_resolve(a[0], TOKEN_SOM_DUMP_SELF_INT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(int, vec![a[0], a[1]], false)], false,
                                                ChainThen::Return(0)))
            }
            3212 => {
                // somDefaultInit(ctrl): at the root, the object's somInit
                // (classes written for SOM 1.x initialize there).
                let init = self.som_resolve(a[0], TOKEN_SOM_INIT);
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(init, vec![a[0]], false)], false,
                                                ChainThen::Return(0)))
            }
            3213 => {
                // somDestruct(doFree, ctrl): the object's somUninit, then
                // its memory when doFree (an octet) is set.
                if !self.shared.som.lock_or_recover().instances.contains(&a[0]) { return ok(0); }
                let uninit = self.som_resolve(a[0], TOKEN_SOM_UNINIT);
                let then = if a[1] & 0xFF != 0 { ChainThen::SomFree { obj: a[0] } } else { ChainThen::Return(0) };
                self.start_chain(CallChain::new(vcpu, vec![CallStep::hook(uninit, vec![a[0]], false)], false, then))
            }
            // ── SOMClass (the receiver is a class object) ──
            3300 | 3301 => {
                let Some(obj) = self.som_new_instance(a[0]) else { return ok(0) };
                if ord == 3301 { return ok(obj); }
                // somNew = somNewNoInit + the object's somDefaultInit; the
                // root's goes straight to somInit.
                let default_init = self.som_resolve(obj, TOKEN_SOM_DEFAULT_INIT);
                let step = if default_init == thunk(3212) {
                    CallStep::hook(self.som_resolve(obj, TOKEN_SOM_INIT), vec![obj], false)
                } else {
                    CallStep::hook(default_init, vec![obj, 0], false)
                };
                self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::Return(obj)))
            }
            3302 => ok(info(a[0], &|c| c.name_addr)),
            3303 => ok(info(a[0], &|c| c.parents.first().copied().unwrap_or(0))),
            3304 => ok(info(a[0], &|c| c.instance_size)),
            3305 => ok(info(a[0], &|c| c.data_size)),
            // The data token of a class is the class itself.
            3306 => ok(info(a[0], &|c| if c.parts.is_empty() { 0 } else { a[0] })),
            3307 => ok(info(a[0], &|c| c.slots.len() as u32)),
            // somGetNumStaticMethods: the static methods the class has,
            // inherited ones included (SOM Programming Reference).
            3308 => ok(info(a[0], &|c| c.slots.len() as u32)),
            3309 => ok(self.shared.som.lock_or_recover().descends(a[0], a[1]) as u32),
            3310 => {
                // somInitClass(self, className, parent, dataSize,
                // maxStaticMethods, majorVersion, minorVersion).
                let r = self.som_init_class(a[0], a[1], &[a[2]], a[3]);
                if let Some(c) = self.shared.som.lock_or_recover().classes.get_mut(&a[0]) { c.version = (a[5], a[6]); }
                ok(r)
            }
            3317 => {
                // somCheckVersion(major, minor): the same major version and
                // at least the minor one; 0.0 asks for none, and a class
                // without a version accepts any (ADR-0072).
                let v = self.shared.som.lock_or_recover().classes.get(&a[0]).map_or((0, 0), |c| c.version);
                ok(((a[1] == 0 && a[2] == 0) || v == (0, 0) || (v.0 == a[1] && v.1 >= a[2])) as u32)
            }
            3318 => {
                // somGetVersionNumbers(&major, &minor).
                let v = self.shared.som.lock_or_recover().classes.get(&a[0]).map_or((0, 0), |c| c.version);
                if a[1] != 0 { let _ = self.guest_write::<u32>(a[1], v.0); }
                if a[2] != 0 { let _ = self.guest_write::<u32>(a[2], v.1); }
                ok(0)
            }
            3319 => ok(info(a[0], &|c| c.mtab)),
            3320 => ok(info(a[0], &|c| c.offset_of(a[0]).unwrap_or(0))),
            3321 | 3322 => {
                // somFindMethod(Ok)(id, &m): the procedure in *m, TRUE (all of
                // Warpine's methods are static); not found: NULL, and the
                // Ok form raises SOMERROR_MethodNotFound.
                let proc = self.som_find_method(a[0], a[1]);
                if a[2] != 0 { let _ = self.guest_write::<u32>(a[2], proc.unwrap_or(0)); }
                match proc {
                    Some(_) => ok(1),
                    None if ord == 3322 => self.som_raise(vcpu, SOMERROR_METHOD_NOT_FOUND, 0, 0, 0),
                    None => ok(0),
                }
            }
            3323..=3325 => {
                // somFindSMethod / somFindSMethodOk / somLookupMethod(id).
                match self.som_find_method(a[0], a[1]) {
                    Some(p) => ok(p),
                    None if ord == 3323 => ok(0),
                    None => self.som_raise(vcpu, SOMERROR_METHOD_NOT_FOUND, 0, 0, 0),
                }
            }
            3326 => ok(self.som_malloc(a[1]).unwrap_or(0)),
            // somDeallocate: the block goes; an object freed this way (a
            // destructor's own deallocation) is no object any more.
            3327 => { self.som_forget_object(a[1]); self.som_free_mem(a[1]); ok(0) }
            3311 => ok(self.som_add_method(a[0], a[1], a[3])),
            3312 => ok(self.som_override(a[0], a[1], a[2])),
            3313 => ok(self.som_class_ready(a[0])),
            3314 | 3315 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                let token = self.shared.som.lock_or_recover().token_of(a[0], &name).unwrap_or(0);
                ok(if ord == 3314 { token } else { (token != 0) as u32 })
            }
            3316 => {
                // somInitMIClass(self, inherit_vars, className, parentClasses
                // (a sequence by value: _maximum, _length, _buffer),
                // dataSize, dataAlignment, maxStaticMethods, major, minor).
                let n = a[4] as usize;
                if n == 0 || n > MAX_PARENTS { return ok(0); }
                let parents: Option<Vec<u32>> = (0..n as u32).map(|i| self.guest_read::<u32>(a[5].checked_add(4 * i)?)).collect();
                let Some(parents) = parents else { return ok(0) };
                ok(self.som_init_class(a[0], a[2], &parents, a[6]))
            }
            // ── SOMClassMgr ──
            3401 => {
                let name = self.som_id_name(a[1]).unwrap_or_default();
                ok(self.shared.som.lock_or_recover().by_name.get(&name).copied().unwrap_or(0))
            }
            3400 => self.som_find_class(vcpu, a[1], a[2], a[3], None, false),
            3403 => self.som_find_class(vcpu, a[1], a[2], a[3], Some(a[4]), false),
            // somLocateClassFile: without an interface repository, the
            // class's name.
            3404 => ok(self.som_id_name(a[1]).map_or(0, |n| self.som_string(&n))),
            3405 => self.som_find_class(vcpu, a[1], a[2], a[3], Some(a[4]), true),
            3406 => ok(self.som_unregister(a[1])),
            3407 => self.som_unload_class_file(vcpu, a[1]),
            3402 => {
                let mut rt = self.shared.som.lock_or_recover();
                if let Some(name) = rt.classes.get(&a[1]).filter(|c| c.ready).map(|c| c.name.clone()) {
                    rt.by_name.insert(name, a[1]);
                }
                ok(0)
            }
            _ => ok(0),
        }
    }

    /// `somFree`'s chain called `somUninit`: the object's memory goes.
    pub(crate) fn som_free_done(&self, obj: u32) -> u32 {
        if self.som_forget_object(obj) { self.som_free_mem(obj); }
        0
    }

    /// Drop the records of object `obj` before its memory goes: whether it
    /// was one.
    fn som_forget_object(&self, obj: u32) -> bool {
        let mut rt = self.shared.som.lock_or_recover();
        rt.classes.remove(&obj);
        rt.instances.remove(&obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(r: ApiResult) -> u32 {
        match r { ApiResult::Normal(v) => v, other => panic!("{other:?}") }
    }

    fn call(l: &Loader, ord: u32, args: &[u32]) -> ApiResult {
        let mut a = [0u32; 10];
        a[..args.len()].copy_from_slice(args);
        l.som_call(0, ord, &a)
    }

    /// Run a chain the way a guest would: each step "returns" 0.
    fn finish(l: &Loader, r: ApiResult) -> u32 {
        match r {
            ApiResult::CallChain(mut c) => {
                while c.steps.pop_front().is_some() { c.returned(0, false); }
                match l.finish_chain(&c) {
                    super::super::pm_hook::ChainEnd::Return(v) => v,
                    other => panic!("{other:?}"),
                }
            }
            ApiResult::Normal(v) => v,
            other => panic!("{other:?}"),
        }
    }

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, format!("{s}\0").as_bytes()).unwrap();
        at
    }

    fn root(l: &Loader, ord: u32) -> u32 {
        l.guest_read::<u32>(l.som_data_address(ord).unwrap()).unwrap()
    }

    /// The token of root method `n` in its class's exported ClassData.
    /// The token of method `name` in root `ClassData` export `ord`, at its
    /// place in IBM's release order.
    fn named_token(l: &Loader, ord: u32, name: &str) -> u32 {
        let table = match ord {
            SOM_OBJECT_CLASS_DATA => SOMOBJECT_METHODS,
            SOM_CLASS_CLASS_DATA => SOMCLASS_METHODS,
            _ => SOMCLASSMGR_METHODS,
        };
        root_token(l, ord, table.iter().position(|m| m.1 == name).expect("a root method"))
    }

    fn root_token(l: &Loader, ord: u32, n: usize) -> u32 {
        l.guest_read::<u32>(l.som_data_address(ord).unwrap() + 4 + 4 * n as u32).unwrap()
    }

    /// A class built as a binding does: an instance of `meta`, its
    /// initialization, methods added or overridden, ready.
    fn new_class(l: &Loader, meta: u32, name: &str, parents: &[u32], size: u32, methods: &[(&str, u32)]) -> u32 {
        let cls = finish(l, call(l, 3300, &[meta]));
        assert_ne!(cls, 0);
        let n = put(l, 0x3400, name);
        if parents.len() <= 1 {
            ok(call(l, 3310, &[cls, n, parents.first().copied().unwrap_or(0), size, 0, 0, 0]));
        } else {
            for (i, p) in parents.iter().enumerate() { l.guest_write::<u32>(0x3800 + 4 * i as u32, *p).unwrap(); }
            let k = parents.len() as u32;
            ok(call(l, 3316, &[cls, 0, n, k, k, 0x3800, size, 4, 0, 0]));
        }
        for &(m, p) in methods {
            let id = ok(call(l, SOM_ID_FROM_STRING, &[put(l, 0x3900, m)]));
            if ok(call(l, 3314, &[cls, id])) != 0 {
                ok(call(l, 3312, &[cls, id, p]));
            } else {
                assert_ne!(ok(call(l, 3311, &[cls, id, 0, p, 0, 0])), 0);
            }
        }
        ok(call(l, 3313, &[cls]));
        cls
    }

    fn token(l: &Loader, cls: u32, name: &str) -> u32 {
        let id = ok(call(l, SOM_ID_FROM_STRING, &[put(l, 0x3900, name)]));
        ok(call(l, 3314, &[cls, id]))
    }

    #[test]
    fn the_root_classes_and_their_exported_data() {
        let l = Loader::new_mock();
        let som_object = root(&l, SOM_OBJECT_CLASS_DATA);
        assert_ne!(som_object, 0);
        assert_eq!(named_token(&l, SOM_OBJECT_CLASS_DATA, "somInit"), TOKEN_SOM_INIT);
        assert_eq!(named_token(&l, SOM_OBJECT_CLASS_DATA, "somUninit"), TOKEN_SOM_UNINIT);
        assert_eq!(named_token(&l, SOM_OBJECT_CLASS_DATA, "somDumpSelfInt"), TOKEN_SOM_DUMP_SELF_INT);
        let mgr = root(&l, SOM_CLASS_MGR_OBJECT);
        assert_eq!(ok(call(&l, SOM_ENVIRONMENT_NEW, &[])), mgr);
        // The manager is an instance of SOMClassMgr, itself a SOMObject.
        let mgr_class = ok(call(&l, 3203, &[mgr]));
        assert_eq!(l.read_guest_string(ok(call(&l, 3204, &[mgr]))), "SOMClassMgr");
        assert_eq!(ok(call(&l, 3206, &[mgr, som_object])), 1, "somIsA SOMObject");
        assert_eq!(ok(call(&l, 3303, &[mgr_class])), som_object, "its parent");
        // somResolve answers from the class's methods.
        let get_class = named_token(&l, SOM_OBJECT_CLASS_DATA, "somGetClass");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, get_class])), thunk(3203), "somGetClass");
        let find = named_token(&l, SOM_CLASS_MGR_CLASS_DATA, "somFindClass");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, find])), thunk(3400), "somFindClass");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[som_object, find])), thunk(SOM_METHOD_ERROR), "not a SOMClassMgr's");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, 99_999])), thunk(SOM_METHOD_ERROR));
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[0x1234_0000, 1])), thunk(SOM_METHOD_ERROR), "not an object");
        // The runtime variables hold the allocation procedures.
        assert_eq!(root(&l, SOM_VAR_MALLOC), thunk(SOM_MALLOC));
        assert_eq!(root(&l, SOM_VAR_FREE), thunk(SOM_FREE_MEM));
        assert_eq!(root(&l, SOM_TRACE_LEVEL), 0);
        let span = l.som_data_address(SOM_LAST_DATA).unwrap() + 4 - l.som_data_address(SOM_FIRST_DATA).unwrap();
        assert_eq!(span, data_size());
    }

    /// A class built as a binding does: a new class object from SOMClass,
    /// somInitClass, a static method, an override, somClassReady; then an
    /// instance, its data, a parent call and somFree.
    #[test]
    fn a_class_is_built_and_used() {
        let l = Loader::new_mock();
        let som_class = root(&l, SOM_CLASS_CLASS_DATA);
        let som_object = root(&l, SOM_OBJECT_CLASS_DATA);
        let cls = new_class(&l, som_class, "Hello", &[som_object], 8, &[("sayHello", 0x0060_0000), ("somInit", 0x0060_1000)]);
        let tok = token(&l, cls, "sayHello");
        assert_eq!(tok as usize, SOMOBJECT_METHODS.len() + SOMCLASS_METHODS.len() + SOMCLASSMGR_METHODS.len() + 1);
        assert_eq!(ok(call(&l, 3304, &[cls])), 12, "4 + its 8 bytes");
        assert_eq!(ok(call(&l, 3306, &[cls])), cls, "its data token");
        // An instance: somNew calls its somInit (the override).
        let r = call(&l, 3300, &[cls]);
        let ApiResult::CallChain(ref c) = r else { panic!() };
        assert!(matches!(c.steps.front(), Some(CallStep::Guest { proc_addr: 0x0060_1000, .. })));
        let obj = finish(&l, r);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, tok])), 0x0060_0000);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, cls])), obj + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, som_class])), 0, "not its ancestor");
        assert_eq!(ok(call(&l, SOM_CLASS_RESOLVE, &[som_object, TOKEN_SOM_INIT])), thunk(3200), "the parent's somInit");
        assert_eq!(l.read_guest_string(ok(call(&l, 3204, &[obj]))), "Hello");
        let id = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3100, "sayHello")]));
        let again = ok(call(&l, SOM_ID_FROM_STRING, &[0x3100]));
        assert_ne!(again, id, "each caller owns its id");
        assert_eq!(ok(call(&l, SOM_COMPARE_IDS, &[id, again])), 1, "for the same string");
        assert_eq!(ok(call(&l, 3208, &[obj, id])), 1, "somRespondsTo sayHello");
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1);
        // somFree: somUninit, then the memory.
        assert_eq!(finish(&l, call(&l, 3202, &[obj])), 0);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        // A ready class takes no more methods; the manager finds it.
        assert_eq!(ok(call(&l, 3311, &[cls, id, 0, 0x0060_2000, 0, 0])), 0);
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        assert_eq!(ok(call(&l, 3400, &[mgr, ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3200, "Hello")])), 0, 0])), cls);
    }

    /// The SOM 2.1 initializers the C++ bindings use: somNew calls the
    /// object's somDefaultInit (straight to somInit at the root),
    /// somDefaultInit calls somInit, somDestruct calls somUninit and frees
    /// only when asked.
    #[test]
    fn default_init_and_destruct() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        assert_eq!(named_token(&l, SOM_OBJECT_CLASS_DATA, "somDefaultInit"), TOKEN_SOM_DEFAULT_INIT);
        let first = |r: &ApiResult| match r {
            ApiResult::CallChain(c) => match c.steps.front() {
                Some(CallStep::Guest { proc_addr, args, .. }) => (*proc_addr, args.clone()),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        // Only somInit overridden: somNew goes to it.
        let plain = new_class(&l, som_class, "Plain", &[som_object], 0, &[("somInit", 0x0061_0000), ("somUninit", 0x0061_1000)]);
        let r = call(&l, 3300, &[plain]);
        assert_eq!(first(&r).0, 0x0061_0000);
        let obj = finish(&l, r);
        // somDefaultInit at the root: the object's somInit.
        let r = call(&l, 3212, &[obj, 0]);
        assert_eq!(first(&r), (0x0061_0000, vec![obj]));
        assert_eq!(finish(&l, r), 0);
        // somDestruct without doFree: somUninit, the object stays.
        let r = call(&l, 3213, &[obj, 0, 0]);
        assert_eq!(first(&r), (0x0061_1000, vec![obj]));
        finish(&l, r);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1);
        // With doFree (only its low byte counts): the memory goes too.
        finish(&l, call(&l, 3213, &[obj, 0x0100, 0]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1, "doFree is an octet");
        finish(&l, call(&l, 3213, &[obj, 1, 0]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        assert_eq!(ok(call(&l, 3213, &[obj, 1, 0])), 0, "not twice");
        // somDestruct overridden: somFree calls it with doFree set and a
        // null control, and leaves the freeing to it.
        let tidy = new_class(&l, som_class, "Tidy", &[som_object], 0, &[("somDestruct", 0x0063_0000)]);
        let obj = finish(&l, call(&l, 3300, &[tidy]));
        let r = call(&l, 3202, &[obj]);
        assert_eq!(first(&r), (0x0063_0000, vec![obj, 1, 0]));
        finish(&l, r);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1, "freed by the override's chain, not by somFree");
        // Its chain to the root's somDestruct frees it.
        finish(&l, call(&l, 3213, &[obj, 1, 0]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        // somDefaultInit overridden: somNew calls it with a null control.
        let modern = new_class(&l, som_class, "Modern", &[som_object], 0, &[("somDefaultInit", 0x0062_0000)]);
        let r = call(&l, 3300, &[modern]);
        let (proc, args) = first(&r);
        assert_eq!(proc, 0x0062_0000);
        assert_eq!(args[1], 0);
        let obj = finish(&l, r);
        assert_eq!(args[0], obj);
    }

    /// The replaceable runtime variables: somPrintf through a program's
    /// SOMOutCharRoutine, somSetOutChar, SOMError's default, and their
    /// initial procedures.
    #[test]
    fn output_and_error_routines() {
        let l = Loader::new_mock();
        for (var, f) in [(SOM_VAR_OUT_CHAR, SOM_DEFAULT_OUT_CHAR), (SOM_VAR_LOAD_MODULE, SOM_DEFAULT_LOAD_MODULE),
                         (SOM_VAR_DELETE_MODULE, SOM_DEFAULT_DELETE_MODULE), (SOM_VAR_ERROR, SOM_DEFAULT_ERROR)] {
            assert_eq!(root(&l, var), thunk(f));
        }
        // The default routine writes directly.
        let fmt = put(&l, 0x3400, "n=%d\n");
        assert_eq!(ok(call(&l, SOM_PRINTF, &[fmt, 42])), 5);
        // A program's routine gets every character, in order.
        assert_eq!(ok(call(&l, SOM_SET_OUT_CHAR, &[0x0063_0000])), 0);
        assert_eq!(root(&l, SOM_VAR_OUT_CHAR), 0x0063_0000);
        let r = call(&l, SOM_PRINTF, &[fmt, 42]);
        let ApiResult::CallChain(ref c) = r else { panic!("{r:?}") };
        let chars: Vec<u32> = c.steps.iter().map(|s| match s {
            CallStep::Guest { proc_addr: 0x0063_0000, args, .. } => args[0],
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(chars, b"n=42\n".iter().map(|&b| b as u32).collect::<Vec<_>>());
        assert_eq!(finish(&l, r), 5);
        // somSetOutChar(0) puts the default back.
        ok(call(&l, SOM_SET_OUT_CHAR, &[0]));
        assert_eq!(root(&l, SOM_VAR_OUT_CHAR), thunk(SOM_DEFAULT_OUT_CHAR));
        assert_eq!(ok(call(&l, SOM_DEFAULT_OUT_CHAR, &[b'x' as u32])), 1);
        // SOMError: a warning returns, a fatal code (last digit 9) ends
        // the process with it.
        let file = put(&l, 0x3500, "x.c");
        assert_eq!(ok(call(&l, SOM_DEFAULT_ERROR, &[20012, file, 7])), 0);
        assert!(matches!(call(&l, SOM_DEFAULT_ERROR, &[20019, file, 7]), ApiResult::ExitProcess(20019)));
    }

    /// Class loading: a registered class is found at once; an unknown one
    /// is looked for in a file named after it; a program's SOMLoadModule
    /// is called with the class, the file, "SOMInitModule", the versions
    /// and a handle cell, and the class it made is returned.
    #[test]
    fn classes_are_found_and_loaded() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        let id = |n: &str, at: u32| ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, at, n)]));
        let known = new_class(&l, som_class, "Known", &[som_object], 0, &[]);
        assert_eq!(finish(&l, call(&l, 3400, &[mgr, id("Known", 0x3600), 0, 0])), known);
        // somLocateClassFile: the class's name.
        let missing = id("Nowhere", 0x3620);
        assert_eq!(l.read_guest_string(ok(call(&l, 3404, &[mgr, missing, 0, 0]))), "Nowhere");
        // No such file: no class, nothing leaked into a chain.
        assert_eq!(ok(call(&l, 3400, &[mgr, missing, 0, 0])), 0);
        let file = put(&l, 0x3640, "NOFILE");
        assert_eq!(ok(call(&l, 3403, &[mgr, missing, 0, 0, file])), 0);
        // A program's SOMLoadModule.
        let var = l.som_data_address(SOM_VAR_LOAD_MODULE).unwrap();
        l.guest_write::<u32>(var, 0x0064_0000).unwrap();
        let lazy = id("Lazy", 0x3660);
        let r = call(&l, 3403, &[mgr, lazy, 1, 2, file]);
        let ApiResult::CallChain(ref c) = r else { panic!("{r:?}") };
        let Some(CallStep::Guest { proc_addr: 0x0064_0000, args, .. }) = c.steps.front() else { panic!() };
        let args = args.clone();
        assert_eq!(l.read_guest_string(args[0]), "Lazy");
        assert_eq!(l.read_guest_string(args[1]), "NOFILE");
        assert_eq!(l.read_guest_string(args[2]), SOM_INIT_MODULE);
        assert_eq!(&args[3..5], &[1, 2]);
        assert_ne!(args[5], 0, "a handle cell");
        // It "loaded" without making the class: no class.
        assert_eq!(finish(&l, r), 0);
        // It made the class (as SOMInitModule would): that class.
        let r = call(&l, 3405, &[mgr, lazy, 0, 0, file]);
        let made = new_class(&l, som_class, "Lazy", &[som_object], 0, &[]);
        assert_eq!(finish(&l, r), made);
        // A failing SOMLoadModule (non-zero): no class, though one exists.
        let r = call(&l, 3405, &[mgr, lazy, 0, 0, file]);
        let ApiResult::CallChain(mut c) = r else { panic!() };
        c.steps.pop_front();
        c.returned(1, false);
        assert!(matches!(l.finish_chain(&c), super::super::pm_hook::ChainEnd::Return(0)));
        // The default SOMLoadModule without such a file.
        let (cls_name, file_name) = (put(&l, 0x3680, "Lazy"), put(&l, 0x36A0, "NOFILE"));
        assert_eq!(ok(call(&l, SOM_DEFAULT_LOAD_MODULE, &[cls_name, file_name, 0, 0, 0, 0x36C0])),
                   crate::loader::constants::ERROR_MOD_NOT_FOUND);
        assert!(som_inits_ok(&[1, 5, 0], 2));
        assert!(!som_inits_ok(&[1, 0, 7], 2));
    }

    /// somUnregisterClass forgets a class; somUnloadClassFile forgets the
    /// classes of the module that made one, withdraws their method tables
    /// (their objects resolve nothing) and frees the module.
    #[test]
    fn classes_are_unregistered_and_unloaded() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let mgr = ok(call(&l, SOM_ENVIRONMENT_NEW, &[]));
        let a = new_class(&l, som_class, "ModA", &[som_object], 0, &[("work", 0x0065_0000)]);
        let b = new_class(&l, som_class, "ModB", &[som_object], 0, &[]);
        assert_eq!(ok(call(&l, 3406, &[mgr, b])), 0);
        assert_eq!(ok(call(&l, 3406, &[mgr, b])), 1, "not registered any more");
        assert_eq!(ok(call(&l, 3407, &[mgr, a])), 1, "no module made it");
        // As if a module 0x1234 had made both.
        l.shared.som.lock_or_recover().modules.extend([(a, 0x1234), (b, 0x1234)]);
        let obj = finish(&l, call(&l, 3300, &[a]));
        let work = token(&l, a, "work");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, work])), 0x0065_0000);
        // The default SOMDeleteModule: DosFreeModule of an unknown handle.
        assert_ne!(ok(call(&l, 3407, &[mgr, a])), 0);
        let rt = l.shared.som.lock_or_recover();
        assert!(!rt.by_name.contains_key("ModA") && rt.modules.is_empty());
        drop(rt);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, work])), thunk(SOM_METHOD_ERROR), "its code is gone");
        assert_eq!(ok(call(&l, 3400, &[mgr, ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3700, "ModA")])), 0, 0])), 0,
                   "looked for again (no file here)");
    }

    /// Two parents: methods of both, the right one's override where the
    /// left one kept the introducer's, each ancestor's data once.
    #[test]
    fn multiple_inheritance_merges_methods_and_data() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let a = new_class(&l, som_class, "A", &[som_object], 4, &[("fa", 0xA000), ("shared", 0xA100)]);
        let b = new_class(&l, som_class, "B", &[a], 8, &[("fb", 0xB000)]);
        let c = new_class(&l, som_class, "C", &[a], 12, &[("fc", 0xC000), ("shared", 0xC100), ("somInit", 0xC200)]);
        let d = new_class(&l, som_class, "D", &[b, c], 0, &[]);
        for (m, p) in [("fa", 0xA000), ("fb", 0xB000), ("fc", 0xC000), ("shared", 0xC100), ("somInit", 0xC200)] {
            assert_eq!(ok(call(&l, SOM_CLASS_RESOLVE, &[d, token(&l, d, m)])), p, "{m}");
        }
        assert_eq!(token(&l, d, "shared"), token(&l, a, "shared"), "one token, introduced by A");
        // D: SOMObject 0..4, A 4..8, B 8..16, C 16..28.
        assert_eq!(ok(call(&l, 3304, &[d])), 28);
        let obj = finish(&l, call(&l, 3300, &[d]));
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, a])), obj + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, b])), obj + 8);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, c])), obj + 16);
        assert_eq!(ok(call(&l, 3206, &[obj, c])), 1, "somIsA its right parent");
        assert_eq!(ok(call(&l, 3309, &[d, b])), 1);
        assert_eq!(ok(call(&l, 3303, &[d])), b, "somGetParent: the first");
    }

    /// A metaclass with data and a somInit override; a class whose
    /// requested metaclass is unrelated to its parent's gets a derived one.
    #[test]
    fn metaclasses_and_derived_metaclasses() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let m_animal = new_class(&l, som_class, "M_Animal", &[som_class], 12, &[("newAnimal", 0x7000), ("somInit", 0x7100)]);
        let kennel = new_class(&l, som_class, "Kennel", &[som_class], 4, &[("newDog", 0x8000), ("somNew", 0x8100)]);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[m_animal, som_object])), m_animal);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, som_object])), som_class, "the parent's");
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[som_object, som_object])), 0, "not a metaclass");
        // somNew of a metaclass runs the metaclass's somInit on the new class.
        let r = call(&l, 3300, &[m_animal]);
        let ApiResult::CallChain(ref c) = r else { panic!() };
        assert!(matches!(c.steps.front(), Some(CallStep::Guest { proc_addr: 0x7100, .. })));
        drop(r);
        // A class object of M_Animal carries M_Animal's data.
        let animal = new_class(&l, m_animal, "Animal", &[som_object], 8, &[("talk", 0x9000)]);
        assert_eq!(ok(call(&l, 3203, &[animal])), m_animal);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[animal, m_animal])), animal + 4);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, animal])), m_animal, "inherited");
        // Dog asks for Kennel: Kennel + M_Animal, made once.
        let derived = ok(call(&l, SOM_METACLASS_FOR, &[kennel, animal]));
        assert!(derived != 0 && derived != kennel && derived != m_animal);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[kennel, animal])), derived);
        assert_eq!(l.read_guest_string(ok(call(&l, 3302, &[derived]))), "Kennel+M_Animal");
        let dog = new_class(&l, derived, "Dog", &[animal], 8, &[]);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, kennel, "newDog")])), 0x8000);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, m_animal, "newAnimal")])), 0x7000);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, TOKEN_SOM_INIT])), 0x7100, "M_Animal's somInit");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[dog, token(&l, som_class, "somNew")])), 0x8100, "Kennel's somNew");
        // Kennel's data, then M_Animal's, in the Dog class object.
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[dog, kennel])), dog + 4);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[dog, m_animal])), dog + 8);
        assert_eq!(ok(call(&l, SOM_METACLASS_FOR, &[0, dog])), derived);
        // A Dog is an Animal; its class descends from both metaclasses.
        let rex = finish(&l, call(&l, 3301, &[dog]));
        assert_eq!(ok(call(&l, 3206, &[rex, animal])), 1);
        assert_eq!(ok(call(&l, 3206, &[dog, m_animal])), 1);
        assert_eq!(ok(call(&l, 3206, &[dog, kennel])), 1);
    }

    #[test]
    fn the_heap_allocates_frees_and_reallocates() {
        let l = Loader::new_mock();
        let a = ok(call(&l, SOM_MALLOC, &[20]));
        let b = ok(call(&l, SOM_MALLOC, &[20]));
        assert!(a != 0 && b == a + 32, "16-byte rounding in one arena");
        l.guest_write_bytes(a, b"abcdef").unwrap();
        let r = ok(call(&l, SOM_REALLOC, &[a, 100]));
        assert_eq!(l.guest_read_bytes(r, 6).unwrap(), b"abcdef");
        assert_eq!(l.shared.som.lock_or_recover().heap.blocks.get(&a), None, "the old block is gone");
        assert_eq!(ok(call(&l, SOM_MALLOC, &[20])), a, "and reused");
        let z = ok(call(&l, SOM_CALLOC, &[4, 8]));
        assert_eq!(l.guest_read_bytes(z, 32).unwrap(), vec![0; 32]);
        assert_eq!(ok(call(&l, SOM_CALLOC, &[u32::MAX, 2])), 0, "overflow");
        assert_eq!(ok(call(&l, SOM_MALLOC, &[MAX_ALLOC + 1])), 0);
        let big = ok(call(&l, SOM_MALLOC, &[100_000]));
        assert_eq!(big % 4096, 0, "pages of its own");
        ok(call(&l, SOM_FREE_MEM, &[big]));
        ok(call(&l, SOM_FREE_MEM, &[0x1234_5678]));
        assert_eq!(ok(call(&l, SOM_REALLOC, &[0x1234_5678, 8])), 0, "not a block");
        assert_eq!(ok(call(&l, SOM_REALLOC, &[r, 0])), 0, "freed");
        assert_eq!(l.shared.som.lock_or_recover().heap.blocks.get(&r), None);
    }

    #[test]
    fn c_format_follows_printf() {
        let strs = |p: u32| -> Vec<u8> { if p == 1 { b"dog".to_vec() } else { b"cat".to_vec() } };
        let f = |fmt: &str, args: &[u32]| String::from_utf8(c_format(fmt.as_bytes(), &mut args.iter().copied(), &strs)).unwrap();
        assert_eq!(f("\tThe %s says %d%%\n", &[1, 42]), "\tThe dog says 42%\n");
        assert_eq!(f("[%5d|%-5d|%05d|%+d|% d]", &[7, 7, 7, 7, 7]), "[    7|7    |00007|+7| 7]");
        assert_eq!(f("%d %u %x %X %#x %o %c", &[(-3i32) as u32, (-1i32) as u32, 255, 255, 255, 8, b'A' as u32]),
                   "-3 4294967295 ff FF 0xff 10 A");
        assert_eq!(f("%.3d|%.2s|%*d|%-*d|%s", &[5, 2, 4, 9, 3, 1, 0]), "005|ca|   9|1  |(null)");
        assert_eq!(f("%hd %lld %p", &[0x1_FFFF, 1, 1, 0xABCD]), "-1 4294967297 0000ABCD");
        let d = 3.25f64.to_bits();
        let (lo, hi) = (d as u32, (d >> 32) as u32);
        assert_eq!(f("%f %.1f %e %g %G", &[lo, hi, lo, hi, lo, hi, lo, hi, lo, hi]),
                   "3.250000 3.2 3.250000e+00 3.25 3.25");
        let big = 1.5e20f64.to_bits();
        assert_eq!(f("%g", &[big as u32, (big >> 32) as u32]), "1.5e+20");
        assert_eq!(f("%y|100%", &[]), "%y|100%", "unknown conversions are kept");
        assert_eq!(f("%d", &[]), "0", "missing arguments are 0");
        assert!(c_format(b"%4096d%4096d", &mut std::iter::empty(), &strs).len() <= MAX_PRINT);
    }

    #[test]
    fn bad_input_is_refused() {
        let l = Loader::new_mock();
        let som_class = root(&l, SOM_CLASS_CLASS_DATA);
        let cls = finish(&l, call(&l, 3300, &[som_class]));
        // Instance data too large; an unready parent; no name.
        assert_eq!(ok(call(&l, 3310, &[cls, put(&l, 0x3000, "Big"), 0, MAX_INSTANCE_DATA + 1, 0, 0, 0])), 0);
        let other = finish(&l, call(&l, 3300, &[som_class]));
        ok(call(&l, 3310, &[cls, put(&l, 0x3000, "Kid"), other, 4, 0, 0, 0]));
        assert_eq!(ok(call(&l, 3313, &[cls])), 0);
        assert_eq!(ok(call(&l, 3300, &[cls])), 0, "not ready: no instances");
        // Too many parents, or none, for somInitMIClass.
        assert_eq!(ok(call(&l, 3316, &[cls, 0, 0x3000, 99, 99, 0x3800, 0, 4, 0, 0])), 0);
        assert_eq!(ok(call(&l, 3316, &[cls, 0, 0x3000, 0, 0, 0x3800, 0, 4, 0, 0])), 0);
        // somFree of something that is not an object frees nothing.
        assert_eq!(ok(call(&l, 3202, &[0x3000])), 0);
        assert_eq!(l.som_data_address(55), None);
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[0x3000, som_class])), 0, "not an object");
    }

    /// IBM's SOM 1.0 exports as osFree lists them (`SOM/som/makefile` in
    /// osfree-project/osfree, BSD-3-Clause; ADR-0070), in ordinal order.
    const OSFREE_SOM_EXPORTS: [&str; 47] = [
        "SOMCalloc", "SOMClassCClassData", "SOMClassClassData", "SOMClassInitFuncName",
        "SOMClassMgrCClassData", "SOMClassMgrClassData", "SOMClassMgrObject", "SOMDeleteModule",
        "SOMError", "SOMFree", "SOMLoadModule", "SOMMalloc", "SOMObjectCClassData",
        "SOMObjectClassData", "SOMOutCharRoutine", "SOMRealloc", "SOM_AssertLevel",
        "SOM_TraceLevel", "SOM_WarnLevel", "SOMClassMgrNewClass", "SOMClassNewClass",
        "SOMObjectNewClass", "somAssert", "somBeginPersistentIds", "somCheckArgs", "somCheckId",
        "somCompareIds", "somConstructClass", "somEndPersistentIds", "somEnvironmentNew",
        "somIdFromString", "somLPrintf", "somParentResolve", "somPrefixLevel", "somPrintf",
        "somRegisterId", "somResolve", "somSaveMetrics", "somSetExpectedIds", "somStringFromId",
        "somTest", "somTestCls", "somTotalRegIds", "somUniqueKey", "somVprintf", "somWriteMetrics",
        "somDataResolve",
    ];

    /// Ordinals 1–47 are osFree's; every other SOM export is Warpine's own,
    /// at 3072 or above (ADR-0071), so IBM's later numbers cannot collide with them.
    #[test]
    fn ordinals_follow_osfree_and_keep_ibm_s_range_free() {
        use super::super::api_registry::{all, find_by_module_and_name};
        for (i, name) in OSFREE_SOM_EXPORTS.iter().enumerate() {
            assert_eq!(find_by_module_and_name("SOM", name), Some(i as u32 + 1), "{name}");
        }
        for e in all().iter().filter(|e| e.module == "SOM") {
            let ord = e.ordinal - SOM_BASE;
            assert!(ord <= SOM_LAST_IBM || ord >= SOM_FIRST_PRIVATE, "{} at {ord}", e.name);
            assert!(ord < super::super::constants::SOM_ORDINALS, "{} past the module's ordinals", e.name);
        }
    }

    /// Data exports are ordinals 1–19, each at its own address; the
    /// functions after them are thunks.
    #[test]
    fn data_exports_are_the_first_nineteen() {
        let l = Loader::new_mock();
        let mut seen = HashSet::new();
        for ord in SOM_FIRST_DATA..=SOM_LAST_DATA {
            let at = l.som_data_address(ord).expect("data");
            assert!(seen.insert(at), "ordinal {ord} shares an address");
        }
        assert_eq!(l.som_data_address(SOM_CLASS_MGR_NEW_CLASS), None);
        assert_eq!(root(&l, SOM_CLASS_MGR_OBJECT), l.shared.som.lock_or_recover().mgr);
        // SOMClassInitFuncName holds a procedure giving "SOMInitModule".
        assert_eq!(root(&l, SOM_VAR_CLASS_INIT_FUNC_NAME), thunk(SOM_DEFAULT_CLASS_INIT_FUNC_NAME));
        let name = ok(call(&l, SOM_DEFAULT_CLASS_INIT_FUNC_NAME, &[]));
        assert_eq!(l.read_guest_string(name), SOM_INIT_MODULE);
        assert_eq!(ok(call(&l, SOM_DEFAULT_CLASS_INIT_FUNC_NAME, &[])), name, "one copy");
    }

    /// `<Class>NewClass`, `CClassData.parentMtab` and `somParentResolve`.
    #[test]
    fn new_class_procedures_and_parent_resolution() {
        let l = Loader::new_mock();
        l.som_init();
        let (o, c, m) = {
            let rt = l.shared.som.lock_or_recover();
            (rt.som_object, rt.som_class, rt.som_class_mgr)
        };
        assert_eq!(ok(call(&l, SOM_OBJECT_NEW_CLASS, &[1, 4])), o);
        assert_eq!(ok(call(&l, SOM_CLASS_NEW_CLASS, &[0, 0])), c);
        assert_eq!(ok(call(&l, SOM_CLASS_MGR_NEW_CLASS, &[2, 0])), m);
        // parentMtab: a somParentMtabStruct — the class's own method
        // table, its parents' list, its class object, its instance size.
        let mtab_of = |cls: u32| l.shared.som.lock_or_recover().classes[&cls].mtab;
        let word = |at: u32| l.guest_read::<u32>(at).unwrap();
        let obj_pm = root(&l, SOM_OBJECT_CCLASS_DATA);
        assert_eq!((word(obj_pm), word(obj_pm + 4), word(obj_pm + 8)), (mtab_of(o), 0, o), "SOMObject: no parents");
        let parents = root(&l, SOM_CLASS_CCLASS_DATA);
        assert_eq!((word(parents), word(parents + 8), word(parents + 16)), (mtab_of(c), c, 4));
        let list = word(parents + 4);
        assert_eq!((word(list), word(list + 4)), (mtab_of(o), 0), "SOMObject's table, one entry");
        let mgr_pm = root(&l, SOM_CLASS_MGR_CCLASS_DATA);
        assert_eq!((word(mgr_pm), word(mgr_pm + 4)), (mtab_of(m), list));
        // SOMObjectCClassData's somDispatch* procedures.
        let dispatch = l.som_data_address(SOM_OBJECT_CCLASS_DATA).unwrap() + 8;
        assert_eq!(word(dispatch), thunk(SOM_METHOD_NOT_IMPLEMENTED));
        // The instance-data token follows parentMtab.
        let token_at = l.som_data_address(SOM_CLASS_CCLASS_DATA).unwrap() + 4;
        assert_eq!(l.guest_read::<u32>(token_at), Some(c));
        let init = named_token(&l, SOM_OBJECT_CLASS_DATA, "somInit");
        assert_eq!(ok(call(&l, SOM_PARENT_RESOLVE, &[parents, init])), thunk(3200), "SOMObject's somInit");
        assert_eq!(ok(call(&l, SOM_PARENT_RESOLVE, &[0, init])), thunk(SOM_METHOD_ERROR));
        let find = named_token(&l, SOM_CLASS_MGR_CLASS_DATA, "somFindClass");
        assert_eq!(ok(call(&l, SOM_PARENT_RESOLVE, &[parents, find])), thunk(SOM_METHOD_ERROR),
                   "not a method of the parent");
        assert_eq!(ok(call(&l, SOM_PARENT_NUM_RESOLVE, &[parents, 1, init])), thunk(3200), "parent 1");
        assert_eq!(ok(call(&l, SOM_PARENT_NUM_RESOLVE, &[parents, 2, init])), thunk(SOM_METHOD_ERROR), "no parent 2");
        assert_eq!(ok(call(&l, SOM_PARENT_NUM_RESOLVE, &[parents, 0, init])), thunk(SOM_METHOD_ERROR));
        assert_eq!(ok(call(&l, SOM_PARENT_RESOLVE, &[obj_pm, init])), thunk(SOM_METHOD_ERROR), "SOMObject has none");
    }

    /// The id registry: `somCheckId`, `somRegisterId`, `somUniqueKey`,
    /// `somTotalRegIds`; the hints, the metrics and somConstructClass do nothing.
    #[test]
    fn ids_are_registered_and_counted() {
        let l = Loader::new_mock();
        // A program's own somId: a pointer to the string's pointer.
        let id = 0x3600;
        l.guest_write::<u32>(id, put(&l, 0x3610, "Fresh")).unwrap();
        let before = ok(call(&l, SOM_TOTAL_REG_IDS, &[]));
        assert_eq!(ok(call(&l, SOM_REGISTER_ID, &[id])), 1, "not registered before");
        assert_eq!(ok(call(&l, SOM_REGISTER_ID, &[id])), 0, "registered now");
        assert_eq!(ok(call(&l, SOM_TOTAL_REG_IDS, &[])), before + 1);
        assert_eq!(ok(call(&l, SOM_CHECK_ID, &[id])), id, "gives the id back");
        let other = 0x3700;
        l.guest_write::<u32>(other, put(&l, 0x3710, "Fresh")).unwrap();
        let key = ok(call(&l, SOM_UNIQUE_KEY, &[id]));
        assert_ne!(key, 0);
        assert_eq!(ok(call(&l, SOM_UNIQUE_KEY, &[other])), key, "one key per string");
        assert_eq!(ok(call(&l, SOM_CHECK_ID, &[0])), 0);
        assert_eq!(ok(call(&l, SOM_REGISTER_ID, &[0])), 0);
        for ord in [SOM_SET_EXPECTED_IDS, SOM_BEGIN_PERSISTENT_IDS, SOM_END_PERSISTENT_IDS, SOM_SAVE_METRICS,
                    SOM_WRITE_METRICS, SOM_CONSTRUCT_CLASS] {
            assert_eq!(ok(call(&l, ord, &[1, 2, 3])), 0, "ordinal {ord}");
        }
        assert_eq!(ok(call(&l, SOM_TOTAL_REG_IDS, &[])), before + 1);
    }

    /// `somIdFromString` gives the caller an id of its own, which it may
    /// free with `SOMFree` (SOM Programming Reference) without harming the
    /// registry: the string is found again afterwards.
    #[test]
    fn caller_ids_can_be_freed() {
        let l = Loader::new_mock();
        let name = put(&l, 0x3100, "Owned");
        let id = ok(call(&l, SOM_ID_FROM_STRING, &[name]));
        assert_eq!(l.read_guest_string(ok(call(&l, SOM_STRING_FROM_ID, &[id]))), "Owned");
        let key = ok(call(&l, SOM_UNIQUE_KEY, &[id]));
        let count = ok(call(&l, SOM_TOTAL_REG_IDS, &[]));
        assert!(l.som_free_mem(id), "the caller's id is a SOMMalloc block");
        let again = ok(call(&l, SOM_ID_FROM_STRING, &[name]));
        assert_eq!(l.read_guest_string(ok(call(&l, SOM_STRING_FROM_ID, &[again]))), "Owned");
        assert_eq!(ok(call(&l, SOM_UNIQUE_KEY, &[again])), key, "the same key");
        assert_eq!(ok(call(&l, SOM_TOTAL_REG_IDS, &[])), count, "registered once");
    }

    /// `somGetNumStaticMethods` counts inherited static methods too (SOM
    /// Programming Reference: "including inherited ones").
    #[test]
    fn static_method_counts_include_inherited_ones() {
        let l = Loader::new_mock();
        let (som_class, som_object) = (root(&l, SOM_CLASS_CLASS_DATA), root(&l, SOM_OBJECT_CLASS_DATA));
        let count = |c: u32| ok(call(&l, 3308, &[c]));
        assert_eq!(count(som_object) as usize, SOMOBJECT_METHODS.len());
        assert_eq!(count(som_class) as usize, SOMOBJECT_METHODS.len() + SOMCLASS_METHODS.len());
        let cls = new_class(&l, som_class, "Counted", &[som_object], 0, &[("one", 0x0060_0000), ("somInit", 0x0060_1000)]);
        assert_eq!(count(cls), count(som_object) + 1, "the new method, not the override");
        assert_eq!(count(cls), ok(call(&l, 3307, &[cls])), "every method is static: somGetNumMethods agrees");
    }

    /// `somPrefixLevel`, `somLPrintf` and `somVprintf` write through
    /// `SOMOutCharRoutine` as `somPrintf` does.
    #[test]
    fn prefixed_and_va_list_printing() {
        let l = Loader::new_mock();
        ok(call(&l, SOM_SET_OUT_CHAR, &[0x0063_0000]));
        let text = |r: ApiResult| -> String {
            let ApiResult::CallChain(ref c) = r else { panic!("{r:?}") };
            c.steps.iter().map(|s| match s {
                CallStep::Guest { args, .. } => args[0] as u8 as char,
                other => panic!("{other:?}"),
            }).collect()
        };
        assert_eq!(text(call(&l, SOM_PREFIX_LEVEL, &[2])), "    ");
        let fmt = put(&l, 0x3400, "n=%d %s");
        let s = put(&l, 0x3480, "ok");
        assert_eq!(text(call(&l, SOM_L_PRINTF, &[1, fmt, 7, s])), "  n=7 ok");
        // A va_list: the arguments as words in memory.
        l.guest_write::<u32>(0x3500, 9).unwrap();
        l.guest_write::<u32>(0x3504, s).unwrap();
        assert_eq!(text(call(&l, SOM_VPRINTF, &[fmt, 0x3500])), "n=9 ok");
        // Level 0 and negative levels write nothing; huge ones are capped.
        assert_eq!(ok(call(&l, SOM_PREFIX_LEVEL, &[0])), 0);
        assert_eq!(ok(call(&l, SOM_PREFIX_LEVEL, &[u32::MAX])), 0);
        let r = call(&l, SOM_PREFIX_LEVEL, &[1_000_000]);
        assert_eq!(text(r).len(), (SOM_MAX_PREFIX_LEVEL * SOM_PREFIX_WIDTH) as usize);
    }

    /// The root classes' `ClassData` hold a token for every method of
    /// IBM's release order, at IBM's offsets; a method Warpine lacks
    /// raises `SOMERROR_NotImplementedYet` as a warning and returns 0
    /// (ADR-0072).
    #[test]
    fn class_data_follows_ibm_s_release_order() {
        let l = Loader::new_mock();
        l.som_init();
        let mgr = root(&l, SOM_CLASS_MGR_OBJECT);
        assert_eq!(data_export_size(SOM_OBJECT_CLASS_DATA), 4 + 4 * 32);
        assert_eq!(data_export_size(SOM_CLASS_CLASS_DATA), 4 + 4 * 76);
        assert_eq!(data_export_size(SOM_CLASS_MGR_CLASS_DATA), 4 + 4 * 22);
        // IBM's SOMObject order: somInit, somUninit, somFree,
        // somDefaultVCopyInit, somGetClassName, somGetClass, …
        assert_eq!(SOMOBJECT_METHODS[4].1, "somGetClassName");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, root_token(&l, SOM_OBJECT_CLASS_DATA, 4)])), thunk(3204));
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, root_token(&l, SOM_OBJECT_CLASS_DATA, 5)])), thunk(3203));
        // somFindClsInFile comes first in SOMClassMgr's.
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[mgr, root_token(&l, SOM_CLASS_MGR_CLASS_DATA, 0)])), thunk(3403));
        // A method Warpine lacks: a warning through SOMError, then 0.
        let copy = named_token(&l, SOM_OBJECT_CLASS_DATA, "somDefaultCopyInit");
        let proc = ok(call(&l, SOM_RESOLVE, &[mgr, copy]));
        assert_eq!(proc, thunk(SOM_METHOD_NOT_IMPLEMENTED));
        assert_eq!(ok(call(&l, SOM_METHOD_NOT_IMPLEMENTED, &[mgr])), 0);
        // Every token is distinct and resolves to a procedure.
        let all: HashSet<u32> = (0..32).map(|i| root_token(&l, SOM_OBJECT_CLASS_DATA, i)).collect();
        assert_eq!(all.len(), 32);
    }

    /// Method tables have IBM's header (`somMethodTab`): class object,
    /// class name, instance size, alignment and size, then the entries.
    #[test]
    fn method_tables_have_ibm_s_header() {
        let l = Loader::new_mock();
        l.som_init();
        let mgr = root(&l, SOM_CLASS_MGR_OBJECT);
        let word = |at: u32| l.guest_read::<u32>(at).unwrap();
        let mtab = word(mgr);
        let cls = l.shared.som.lock_or_recover().som_class_mgr;
        assert_eq!(word(mtab), cls, "classObject");
        assert_eq!(word(mtab + 4), 0, "classInfo");
        assert_eq!(l.read_guest_string(word(mtab + 8)), "SOMClassMgr");
        assert_eq!(word(mtab + 12), 4, "instanceSize");
        assert_eq!(word(mtab + 16), DATA_ALIGNMENT);
        let n = SOMOBJECT_METHODS.len() + SOMCLASSMGR_METHODS.len();
        assert_eq!(word(mtab + 20), MTAB_HEADER_SIZE + 4 * n as u32, "mtabSize");
        // The entries start after the header, in token order.
        assert_eq!(word(mtab + MTAB_HEADER_SIZE), thunk(3200), "somInit");
        assert_eq!(ok(call(&l, 3319, &[cls])), mtab, "somGetClassMtab");
    }

    /// The SOMClass methods added with IBM's release order: versions,
    /// method lookup by id, the instance offset, allocation.
    #[test]
    fn class_versions_lookup_and_allocation() {
        let l = Loader::new_mock();
        l.som_init();
        let (c, mgr) = { let rt = l.shared.som.lock_or_recover(); (rt.som_class_mgr, rt.mgr) };
        // Versions: the root classes are 1.5.
        assert_eq!(ok(call(&l, 3317, &[c, 1, 4])), 1, "1.4 is met by 1.5");
        assert_eq!(ok(call(&l, 3317, &[c, 1, 6])), 0);
        assert_eq!(ok(call(&l, 3317, &[c, 2, 0])), 0);
        assert_eq!(ok(call(&l, 3317, &[c, 0, 0])), 1, "0.0 asks for nothing");
        ok(call(&l, 3318, &[c, 0x3000, 0x3004]));
        assert_eq!((l.guest_read::<u32>(0x3000), l.guest_read::<u32>(0x3004)), (Some(1), Some(5)));
        // Lookup by somId.
        let id = |s: &str, at: u32| ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, at, s)]));
        let find = id("somFindClass", 0x3100);
        let missing = id("noSuchMethod", 0x3200);
        assert_eq!(ok(call(&l, 3321, &[c, find, 0x3300])), 1);
        assert_eq!(l.guest_read::<u32>(0x3300), Some(thunk(3400)));
        assert_eq!(ok(call(&l, 3321, &[c, missing, 0x3300])), 0);
        assert_eq!(l.guest_read::<u32>(0x3300), Some(0));
        assert_eq!(ok(call(&l, 3323, &[c, find])), thunk(3400), "somFindSMethod");
        assert_eq!(ok(call(&l, 3323, &[c, missing])), 0);
        // The Ok forms raise SOMERROR_MethodNotFound, which is fatal.
        assert!(matches!(call(&l, 3324, &[c, missing]), ApiResult::ExitProcess(SOMERROR_METHOD_NOT_FOUND)));
        assert!(matches!(call(&l, 3325, &[c, missing]), ApiResult::ExitProcess(SOMERROR_METHOD_NOT_FOUND)));
        assert!(matches!(call(&l, 3322, &[c, missing, 0x3300]), ApiResult::ExitProcess(SOMERROR_METHOD_NOT_FOUND)));
        // A program's own SOMError is called instead, and the call returns.
        let at = l.som_data_address(SOM_VAR_ERROR).unwrap();
        l.guest_write::<u32>(at, 0x0064_0000).unwrap();
        let r = call(&l, 3325, &[c, missing]);
        let ApiResult::CallChain(ref chain) = r else { panic!("{r:?}") };
        assert!(matches!(chain.steps.front(), Some(CallStep::Guest { proc_addr: 0x0064_0000, args, .. })
                         if args[0] == SOMERROR_METHOD_NOT_FOUND));
        assert_eq!(finish(&l, r), 0);
        // The instance offset of SOMClassMgr's own data; allocation.
        assert_eq!(ok(call(&l, 3320, &[c])), 4);
        let block = ok(call(&l, 3326, &[c, 64]));
        assert_ne!(block, 0);
        assert_eq!(ok(call(&l, 3327, &[c, block])), 0);
        // somDeallocate of an object (a destructor freeing it): no object
        // any more.
        let o = l.shared.som.lock_or_recover().som_object;
        let obj = ok(call(&l, 3301, &[o]));
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 1);
        assert_eq!(ok(call(&l, 3327, &[o, obj])), 0);
        assert_eq!(ok(call(&l, SOM_IS_OBJ, &[obj])), 0);
        let _ = mgr;
    }

    /// somTestCls, somTest, somAssert and somCheckArgs as `somapi.h` and
    /// `somcdev.h` declare them (ADR-0072).
    #[test]
    fn testing_entries_and_argument_flags() {
        let l = Loader::new_mock();
        l.som_init();
        let (o, c, mgr) = { let rt = l.shared.som.lock_or_recover(); (rt.som_object, rt.som_class_mgr, rt.mgr) };
        let file = put(&l, 0x3000, "t.c");
        assert_eq!(ok(call(&l, SOM_TEST_CLS, &[mgr, c, file, 7])), mgr, "an instance: the object itself");
        assert_eq!(ok(call(&l, SOM_TEST_CLS, &[mgr, o, file, 7])), mgr, "of a descendant too");
        assert!(matches!(call(&l, SOM_TEST_CLS, &[0x3100, c, file, 7]), ApiResult::ExitProcess(SOMERROR_TEST_OBJECT_FAILURE)));
        // somTest: a kept condition does nothing; a failed one raises
        // SOMERROR_FailedTest with the given severity.
        assert_eq!(ok(call(&l, SOM_TEST, &[1, 9, file, 8, 0])), 0);
        assert_eq!(ok(call(&l, SOM_TEST, &[0, 1, file, 8, 0])), 0, "a warning returns");
        assert!(matches!(call(&l, SOM_TEST, &[0, 9, file, 8, 0]), ApiResult::ExitProcess(20119)));
        // somAssert: a failed one raises its code.
        assert_eq!(ok(call(&l, SOM_ASSERT, &[1, 20019, file, 9, 0])), 0);
        assert!(matches!(call(&l, SOM_ASSERT, &[0, 20019, file, 9, 0]), ApiResult::ExitProcess(20019)));
        // somCheckArgs: the flags set the levels.
        let args = [put(&l, 0x3200, "prog"), put(&l, 0x3210, "-somtc"), put(&l, 0x3220, "-soma"), put(&l, 0x3230, "-somw")];
        for (i, a) in args.iter().enumerate() { l.guest_write::<u32>(0x3300 + 4 * i as u32, *a).unwrap(); }
        ok(call(&l, SOM_CHECK_ARGS, &[4, 0x3300]));
        assert_eq!((root(&l, SOM_TRACE_LEVEL), root(&l, SOM_ASSERT_LEVEL), root(&l, SOM_WARN_LEVEL)), (2, 1, 1));
        // A kept assertion is noted once SOM_AssertLevel is above 0.
        ok(call(&l, SOM_SET_OUT_CHAR, &[0x0063_0000]));
        let r = call(&l, SOM_ASSERT, &[1, 20019, file, 9, put(&l, 0x3400, "x > 0")]);
        let ApiResult::CallChain(ref chain) = r else { panic!("{r:?}") };
        let text: String = chain.steps.iter().map(|s| match s {
            CallStep::Guest { args, .. } => args[0] as u8 as char,
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(text, "Assertion passed: x > 0\n");
    }

    /// A `somStaticClassInfo` in guest memory at `at`, layout 3: the class
    /// named `name` with `parents` (somIds at `ids`), its ClassData at
    /// `cds`, CClassData at `ccds`, the given tables (address, count) and
    /// class init function.
    #[allow(clippy::too_many_arguments)]
    fn put_sci(l: &Loader, at: u32, name_id: u32, parents: &[u32], ids: u32, size: u32, cds: u32, ccds: u32,
               tables: [(u32, u32); 6], nit_base: u32, cif: u32, meta_id: u32) -> u32 {
        // tables: smt, omt, nit, pmt, vft, imt.
        for (i, &p) in parents.iter().enumerate() { l.guest_write::<u32>(ids + 4 * i as u32, p).unwrap(); }
        let mut w = [0u32; 41];
        w[0] = 3;
        (w[1], w[2], w[3], w[4], w[5]) = (tables[0].1, tables[1].1, tables[2].1, tables[3].1, tables[4].1);
        (w[6], w[7], w[8], w[10]) = (1, 2, size, parents.len() as u32);
        (w[11], w[12], w[14], w[15], w[16]) = (name_id, meta_id, ids, cds, ccds);
        (w[17], w[18], w[19], w[20], w[21], w[22], w[23]) = (tables[0].0, tables[1].0, nit_base, tables[2].0, tables[3].0, tables[4].0, cif);
        (w[31], w[32]) = (tables[5].1, tables[5].0);
        for (i, v) in w.iter().enumerate() { l.guest_write::<u32>(at + 4 * i as u32, *v).unwrap(); }
        at
    }

    /// A somId of `s` stored at `cell` (the static tables hold `somId *`).
    fn id_cell(l: &Loader, cell: u32, text_at: u32, s: &str) -> u32 {
        let id = ok(call(l, SOM_ID_FROM_STRING, &[put(l, text_at, s)]));
        l.guest_write::<u32>(cell, id).unwrap();
        cell
    }

    /// somBuildClass from IBM's static class information (ADR-0073):
    /// static methods (tokens into ClassData), an override by descriptor,
    /// a non-internal data token, a procedure entry, CClassData, a
    /// subclass with an inherited implementation and a class init
    /// function; a second call gives the class already built.
    #[test]
    fn build_class_from_static_class_info() {
        let l = Loader::new_mock();
        l.som_init();
        let word = |at: u32| l.guest_read::<u32>(at).unwrap();
        // Animal: talk, eat; somInit overridden; data field at offset 4.
        let animal_id = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3000, "Animal")]));
        let (cds, ccds) = (0x5000, 0x5100);
        let smt = 0x5200;
        for (i, (name, proc, slot)) in [("talk", 0x7000u32, cds + 4), ("eat", 0x7100, cds + 8)].iter().enumerate() {
            let e = smt + 24 * i as u32;
            let cell = id_cell(&l, 0x5800 + 4 * i as u32, 0x3100 + 0x20 * i as u32, name);
            for (k, v) in [*slot, cell, 0, *proc, 0, 0].iter().enumerate() { l.guest_write::<u32>(e + 4 * k as u32, *v).unwrap(); }
        }
        let omt = 0x5300;
        l.guest_write::<u32>(omt, id_cell(&l, 0x5810, 0x3200, "SOMObject::somDumpSelfInt")).unwrap();
        l.guest_write::<u32>(omt + 4, 0x7200).unwrap();
        let nit = 0x5400;
        l.guest_write::<u32>(nit, cds + 12).unwrap();
        l.guest_write::<u32>(nit + 4, 0x6004).unwrap();
        let pmt = 0x5500;
        l.guest_write::<u32>(pmt, cds + 16).unwrap();
        l.guest_write::<u32>(pmt + 4, 0x7300).unwrap();
        let sci = put_sci(&l, 0x4000, animal_id, &[], 0x5900, 8, cds, ccds,
                          [(smt, 2), (omt, 1), (nit, 1), (pmt, 1), (0, 0), (0, 0)], 0x6000, 0, 0);
        let animal = ok(call(&l, SOM_BUILD_CLASS, &[0, sci, 1, 2]));
        assert_ne!(animal, 0);
        assert_eq!(word(cds), animal, "ClassData.classObject");
        let (talk, eat) = (word(cds + 4), word(cds + 8));
        assert!(talk != 0 && eat != 0 && talk != eat);
        assert_eq!(word(cds + 16), 0x7300, "the procedure entry");
        assert_eq!(ok(call(&l, SOM_BUILD_CLASS, &[0, sci, 1, 2])), animal, "built once");
        assert_eq!(l.read_guest_string(ok(call(&l, 3302, &[animal]))), "Animal");
        assert_eq!(ok(call(&l, 3317, &[animal, 1, 2])), 1, "its version");
        // An instance: its methods, the override, the data field.
        let obj = finish(&l, call(&l, 3300, &[animal]));
        assert_ne!(obj, 0);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, talk])), 0x7000);
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[obj, TOKEN_SOM_DUMP_SELF_INT])), 0x7200, "overridden");
        let part = ok(call(&l, SOM_DATA_RESOLVE, &[obj, animal]));
        assert_eq!(ok(call(&l, SOM_DATA_RESOLVE, &[obj, word(cds + 12)])), part + 4, "the field");
        // CClassData: parentMtab (own table, then SOMObject's), data token.
        let pm = word(ccds);
        let o = l.shared.som.lock_or_recover().som_object;
        let mtab_of = |c: u32| l.shared.som.lock_or_recover().classes[&c].mtab;
        assert_eq!((word(pm), word(pm + 8)), (mtab_of(animal), animal));
        assert_eq!(word(word(pm + 4)), mtab_of(o));
        assert_eq!(word(ccds + 4), animal, "instanceDataToken");

        // Dog(Animal): talk overridden by descriptor, eat overridden and
        // then given back Animal's through the inherited-method table; a
        // class init function is called with the class.
        let dog_id = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3300, "Dog")]));
        let (dcds, dccds) = (0x5a00, 0x5b00);
        let domt = 0x5c00;
        for (i, (name, proc)) in [("Animal::talk", 0x7400u32), ("eat", 0x7500)].iter().enumerate() {
            let cell = id_cell(&l, 0x5d00 + 4 * i as u32, 0x3400 + 0x20 * i as u32, name);
            l.guest_write::<u32>(domt + 8 * i as u32, cell).unwrap();
            l.guest_write::<u32>(domt + 8 * i as u32 + 4, *proc).unwrap();
        }
        let imt = 0x5e00;
        for (k, v) in [id_cell(&l, 0x5d10, 0x3500, "eat"), 1, 0].iter().enumerate() {
            l.guest_write::<u32>(imt + 4 * k as u32, *v).unwrap();
        }
        let dsci = put_sci(&l, 0x4400, dog_id, &[animal_id], 0x5f00, 0, dcds, dccds,
                           [(0, 0), (domt, 2), (0, 0), (0, 0), (0, 0), (imt, 1)], 0, 0x7600, 0);
        let r = call(&l, SOM_BUILD_CLASS, &[0, dsci, 1, 0]);
        let ApiResult::CallChain(ref chain) = r else { panic!("{r:?}") };
        let dog = word(dcds);
        assert!(matches!(chain.steps.front(), Some(CallStep::Guest { proc_addr: 0x7600, args, .. }) if args[0] == dog));
        assert_eq!(finish(&l, r), dog);
        let rex = finish(&l, call(&l, 3300, &[dog]));
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[rex, talk])), 0x7400, "Dog's talk");
        assert_eq!(ok(call(&l, SOM_RESOLVE, &[rex, eat])), 0x7100, "Animal's eat, inherited");
        assert_eq!(ok(call(&l, SOM_PARENT_NUM_RESOLVE, &[word(dccds), 1, talk])), 0x7000, "the parent's talk");
        assert_eq!(ok(call(&l, 3206, &[rex, animal])), 1, "somIsA Animal");
    }

    /// somBuildClass refuses what it cannot build: an unknown layout, a
    /// missing parent or metaclass (SOMERROR_BadArgument / NoParentClass /
    /// NoMetaClass, fatal).
    #[test]
    fn build_class_errors() {
        let l = Loader::new_mock();
        l.som_init();
        let id = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3000, "Lost")]));
        let missing = ok(call(&l, SOM_ID_FROM_STRING, &[put(&l, 0x3100, "NoSuchParent")]));
        let none = [(0, 0); 6];
        let sci = put_sci(&l, 0x4000, id, &[missing], 0x5900, 0, 0x5000, 0, none, 0, 0, 0);
        assert!(matches!(call(&l, SOM_BUILD_CLASS, &[0, sci, 0, 0]), ApiResult::ExitProcess(SOMERROR_NO_PARENT_CLASS)));
        let sci = put_sci(&l, 0x4000, id, &[], 0x5900, 0, 0x5000, 0, none, 0, 0, missing);
        assert!(matches!(call(&l, SOM_BUILD_CLASS, &[0, sci, 0, 0]), ApiResult::ExitProcess(SOMERROR_NO_META_CLASS)));
        l.guest_write::<u32>(0x4000, 99).unwrap();
        assert!(matches!(call(&l, SOM_BUILD_CLASS, &[0, 0x4000, 0, 0]), ApiResult::ExitProcess(SOMERROR_BAD_ARGUMENT)));
        assert!(matches!(call(&l, SOM_BUILD_CLASS, &[0, 0, 0, 0]), ApiResult::ExitProcess(SOMERROR_BAD_ARGUMENT)));
        // Huge counts are capped, not trusted: no read past the tables.
        let sci = put_sci(&l, 0x4000, id, &[], 0x5900, 0, 0x5000, 0, [(0x0FFF_FFF0, u32::MAX); 6], 0, 0, 0);
        let r = call(&l, SOM_BUILD_CLASS, &[0, sci, 0, 0]);
        assert!(matches!(r, ApiResult::Normal(c) if c != 0), "{r:?}");
    }
}

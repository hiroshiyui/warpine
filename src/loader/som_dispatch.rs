// SPDX-License-Identifier: GPL-3.0-or-later
//
//! SOM dynamic dispatch (ADR-0126): `somApply`, `SOMObject`'s
//! `somDispatch` / `somClassDispatch` and the `somDispatchV` / `L` / `A`
//! forms, and the `somVaBuf_*` and `somvalist*Target` functions (SOM
//! Programming Reference).
//!
//! - **Apply stubs.** A method can be called from a va_list only through
//!   an apply stub, which knows its arguments and result. Warpine's are
//!   written by somidl, one per method: `stub(retVal, method, args)` reads
//!   the arguments (widened, as in a va_list) from `args`, calls `method`
//!   and stores the result at `retVal`. A class registers it through
//!   `somAddStaticMethod`'s `applyStub` as a `somApplyInfo` record
//!   (`SOM_APPLY_INFO_MAGIC`, then the stub); anything else there is not
//!   used, so a method without one (an IBM-built class's) is not applied
//!   and `somApply` returns FALSE, as the reference allows "in support of
//!   old class binaries".
//! - **Root methods** (Warpine's thunks) need no stub: their arguments
//!   are the registry's `argc` words, their result a word.
//! - **va_lists.** Compilers pass a `va_list` differently (GCC and clang:
//!   a pointer to the arguments; Open Watcom: a pointer to that pointer),
//!   so the kernel takes whichever form leads to the target object (or to
//!   a `somVaBuf`'s buffer).
//! - **somDispatchD** returns a `double` in the FPU, which a host call
//!   cannot give: it stays not implemented.

use super::constants::{MAGIC_API_BASE, SOM_BASE, SOM_ORDINALS};
use super::pm_hook::{CallChain, CallStep, ChainThen};
use super::som::thunk;
use super::{ApiResult, Loader, MutexExt};

/// `somApplyInfo.magic`: "WAPS".
pub const SOM_APPLY_INFO_MAGIC: u32 = u32::from_le_bytes(*b"WAPS");
/// A `somVaBuf`'s magic: "WVAB".
const VA_BUF_MAGIC: u32 = u32::from_le_bytes(*b"WVAB");
/// A `somVaBuf`: magic, buffer, capacity, bytes used, owned by the kernel.
const VA_BUF_SIZE: u32 = 20;
/// First capacity of a buffer, and the largest one.
const VA_FIRST_CAPACITY: u32 = 64;
pub const MAX_VA_BUF: u32 = 64 * 1024;
/// The most argument words a root method takes: what one call-chain step
/// passes (a method with more is not applied: its later arguments would be
/// lost).
const MAX_ROOT_ARGS: u32 = super::pm_hook::MAX_CHAIN_ARGS as u32;

/// The `TCKind`s `somVaBuf_add` takes, in the reference's order
/// (`TypeCode_kind`; C numbers them from 0).
pub mod tk {
    pub const SHORT: u32 = 2;
    pub const LONG: u32 = 3;
    pub const USHORT: u32 = 4;
    pub const ULONG: u32 = 5;
    pub const FLOAT: u32 = 6;
    pub const DOUBLE: u32 = 7;
    pub const BOOLEAN: u32 = 8;
    pub const CHAR: u32 = 9;
    pub const OCTET: u32 = 10;
    pub const TYPECODE: u32 = 12;
    pub const ENUM: u32 = 17;
    pub const STRING: u32 = 18;
    pub const POINTER: u32 = 21;
}

/// The widened bytes `somVaBuf_add` appends for an argument of `kind`
/// whose bytes start with `raw` (at least 8 bytes read); None for a kind
/// it does not take.
pub fn widen(kind: u32, raw: &[u8; 8]) -> Option<Vec<u8>> {
    let word = |v: u32| v.to_le_bytes().to_vec();
    Some(match kind {
        tk::SHORT => word(i16::from_le_bytes([raw[0], raw[1]]) as i32 as u32),
        tk::USHORT => word(u32::from(u16::from_le_bytes([raw[0], raw[1]]))),
        tk::CHAR => word(raw[0] as i8 as i32 as u32),
        tk::BOOLEAN | tk::OCTET => word(u32::from(raw[0])),
        tk::LONG | tk::ULONG | tk::ENUM | tk::STRING | tk::POINTER | tk::TYPECODE =>
            raw[..4].to_vec(),
        tk::FLOAT => f64::from(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])).to_le_bytes().to_vec(),
        tk::DOUBLE => raw.to_vec(),
        _ => return None,
    })
}

/// Of a va_list's two possible forms — `ap` pointing at the arguments, or
/// at a pointer to them — the one whose first argument `ok` accepts.
pub fn va_args(ap: u32, read: &dyn Fn(u32) -> Option<u32>, ok: &dyn Fn(u32, u32) -> bool) -> Option<u32> {
    if ap == 0 { return None; }
    if read(ap).is_some_and(|first| ok(ap, first)) { return Some(ap); }
    let inner = read(ap)?;
    (inner != 0 && read(inner).is_some_and(|first| ok(inner, first))).then_some(inner)
}

impl Loader {
    /// The arguments a va_list `ap` leads to, the first being a SOM object
    /// (`target`, when given, that one), or a `somVaBuf`'s buffer.
    fn som_va_args(&self, ap: u32, target: Option<u32>) -> Option<u32> {
        let read = |at: u32| self.guest_word(at);
        let ok = |args: u32, first: u32| {
            if self.shared.som.lock_or_recover().va_buffers.contains_key(&args) { return true; }
            match target {
                Some(t) => first == t,
                None => self.som_object_class(first).is_some(),
            }
        };
        va_args(ap, &read, &ok)
    }

    /// Call `method` (token `token`) with the arguments at `args`, its
    /// result to `ret` (0: none wanted); then `then`. None when it cannot
    /// be applied (no apply stub, not a root method).
    fn som_apply_chain(&self, vcpu: u32, ret: u32, token: u32, method: u32, args: u32) -> Option<ApiResult> {
        let stub = self.shared.som.lock_or_recover().apply.get(&token).copied();
        if let Some(stub) = stub {
            let step = CallStep::hook(stub, vec![ret, method, args], false);
            return Some(self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::Return(1))));
        }
        // A root method: the thunk of a SOMObject, SOMClass or SOMClassMgr
        // method. Not any of this module's thunks: somApply's own, given
        // as its method, would apply itself without end, each level a
        // host frame (security audit 2026-11-25).
        let first = thunk(0);
        if method < first || method - first >= SOM_ORDINALS { return None; }
        let ord = method - (MAGIC_API_BASE as u32 + SOM_BASE);
        if !super::som::is_root_method(ord) { return None; }
        let entry = super::api_registry::find(SOM_BASE + ord)?;
        let argc = entry.argc as u32;
        if argc > MAX_ROOT_ARGS { return None; }
        let bytes = self.guest_read_buffer(args, 4 * argc as usize)?;
        let words = bytes.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
        let step = CallStep::hook(method, words, false);
        Some(self.start_chain(CallChain::new(vcpu, vec![step], false, ChainThen::SomStoreLast { at: ret })))
    }

    /// `somApply(objPtr, retVal, mdPtr, args)`: the method the
    /// `somMethodData` at `md` names (its token and procedure), applied to
    /// the arguments of `args` (objPtr first); FALSE when it cannot be.
    pub(crate) fn som_apply(&self, vcpu: u32, a: &[u32; 10]) -> ApiResult {
        let (obj, ret, md, ap) = (a[0], a[1], a[2], a[3]);
        let fail = ApiResult::Normal(0);
        if ret == 0 || md == 0 { return fail; }
        let word = |o: u32| md.checked_add(o).and_then(|at| self.guest_word(at));
        let (Some(token), Some(method)) = (word(12), word(16)) else { return fail };
        let Some(args) = self.som_va_args(ap, Some(obj)) else { return fail };
        self.som_apply_chain(vcpu, ret, token, method, args).unwrap_or(fail)
    }

    /// `somDispatch(self, retValue, methodId, ap)` (`cls` 0) and
    /// `somClassDispatch(self, clsObj, retValue, methodId, ap)`: the method
    /// named `id` as `self`'s class (or `cls`) resolves it, applied to the
    /// arguments of `ap` (the target first). FALSE when the method is not
    /// found or cannot be applied.
    pub(crate) fn som_dispatch(&self, vcpu: u32, obj: u32, cls: u32, ret: u32, id: u32, ap: u32) -> ApiResult {
        let fail = ApiResult::Normal(0);
        let cls = if cls != 0 { cls } else { match self.som_object_class(obj) { Some(c) => c, None => return fail } };
        let Some(name) = self.som_method_name(id) else { return fail };
        let resolved = {
            let rt = self.shared.som.lock_or_recover();
            rt.token_of(cls, &name).and_then(|t| rt.class_method(cls, t).map(|m| (t, m)))
        };
        let Some((token, method)) = resolved else { return fail };
        let Some(args) = self.som_va_args(ap, None) else { return fail };
        self.som_apply_chain(vcpu, ret, token, method, args).unwrap_or(fail)
    }

    /// `somDispatchV` / `L` / `A(self, methodId, descriptor, ap)`: the
    /// object's own `somDispatch` (an override too), its result returned
    /// (V: none).
    pub(crate) fn som_dispatch_x(&self, vcpu: u32, a: &[u32; 10], value: bool) -> ApiResult {
        let (obj, id, ap) = (a[0], a[1], a[3]);
        let Some(cls) = self.som_object_class(obj) else { return ApiResult::Normal(0) };
        let dispatch = {
            let rt = self.shared.som.lock_or_recover();
            rt.token_of(cls, "somDispatch").and_then(|t| rt.class_method(cls, t))
        };
        let Some(dispatch) = dispatch else { return ApiResult::Normal(0) };
        let scratch = if value { match self.som_scratch(8) { Some(s) => s, None => return ApiResult::Normal(0) } } else { 0 };
        let step = CallStep::hook(dispatch, vec![obj, scratch, id, ap], false);
        let then = if value { ChainThen::SomReturnScratch { at: scratch } } else { ChainThen::Return(0) };
        self.start_chain(CallChain::new(vcpu, vec![step], false, then))
    }

    /// The end of a root method's apply: its result word to `at`; TRUE.
    pub(crate) fn som_store_last(&self, at: u32, last: u32) -> u32 {
        if at != 0 { let _ = self.guest_write::<u32>(at, last); }
        1
    }

    /// The end of `somDispatchL` / `A`: the word at `at`, which is freed.
    pub(crate) fn som_return_scratch(&self, at: u32) -> u32 {
        let v = self.guest_read::<u32>(at).unwrap_or(0);
        self.som_release(at);
        v
    }

    // ── somVaBuf ─────────────────────────────────────────────────────────

    /// `somVaBuf_create(vb, size)`: `vb` when it is given and `size` holds
    /// a somVaBuf, else a new one; NULL when there is no memory.
    pub(crate) fn som_va_buf_create(&self, vb: u32, size: u32) -> u32 {
        let (vb, owned) = if vb != 0 && size >= VA_BUF_SIZE { (vb, 0) } else {
            match self.som_scratch(VA_BUF_SIZE) { Some(v) => (v, 1), None => return 0 }
        };
        let Some(buf) = self.som_scratch(VA_FIRST_CAPACITY) else {
            if owned != 0 { self.som_release(vb); }
            return 0;
        };
        self.shared.som.lock_or_recover().va_buffers.insert(buf, VA_FIRST_CAPACITY);
        for (i, w) in [VA_BUF_MAGIC, buf, VA_FIRST_CAPACITY, 0, owned].into_iter().enumerate() {
            if vb.checked_add(4 * i as u32).and_then(|at| self.guest_write::<u32>(at, w)).is_none() {
                self.shared.som.lock_or_recover().va_buffers.remove(&buf);
                self.som_release(buf);
                if owned != 0 { self.som_release(vb); }
                return 0;
            }
        }
        vb
    }

    /// A somVaBuf's words (buffer, capacity, used, owned), when `vb` is one.
    /// The capacity is the kernel's own record of the buffer, not the
    /// header's, which the program can rewrite (security audit 2026-11-25:
    /// a larger one sent appends past the buffer).
    fn som_va_buf(&self, vb: u32) -> Option<[u32; 4]> {
        let w = |i: u32| self.guest_read::<u32>(vb.checked_add(4 * i)?);
        if vb == 0 || w(0)? != VA_BUF_MAGIC { return None; }
        let (buf, used, owned) = (w(1)?, w(3)?, w(4)?);
        let cap = *self.shared.som.lock_or_recover().va_buffers.get(&buf)?;
        (used <= cap).then_some([buf, cap, used, owned])
    }

    /// A word the program names, unless it lies in Warpine's protected
    /// memory (which the program cannot read itself).
    fn guest_word(&self, at: u32) -> Option<u32> {
        self.guest_read_buffer(at, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `somVaBuf_add(vb, arg, type)`: the argument at `arg` of TCKind
    /// `type`, widened, appended; 1, or 0 for an unknown buffer or kind,
    /// or no room (unverified: the reference gives no codes).
    pub(crate) fn som_va_buf_add(&self, vb: u32, arg: u32, kind: u32) -> u32 {
        let Some([mut buf, mut cap, used, owned]) = self.som_va_buf(vb) else { return 0 };
        let bytes = match kind {
            tk::DOUBLE => self.guest_read_buffer(arg, 8),
            tk::SHORT | tk::USHORT => self.guest_read_buffer(arg, 2),
            tk::CHAR | tk::BOOLEAN | tk::OCTET => self.guest_read_buffer(arg, 1),
            _ => self.guest_read_buffer(arg, 4),
        };
        let Some(bytes) = bytes else { return 0 };
        let mut raw = [0u8; 8];
        raw[..bytes.len()].copy_from_slice(&bytes);
        let Some(value) = widen(kind, &raw) else { return 0 };
        let need = used + value.len() as u32;
        if need > MAX_VA_BUF { return 0; }
        if need > cap {
            let new_cap = (cap * 2).max(need).min(MAX_VA_BUF);
            let Some(grown) = self.som_scratch(new_cap) else { return 0 };
            if let Some(old) = self.guest_read_bytes(buf, used as usize) { let _ = self.guest_write_bytes(grown, &old); }
            {
                let mut rt = self.shared.som.lock_or_recover();
                rt.va_buffers.remove(&buf);
                rt.va_buffers.insert(grown, new_cap);
            }
            self.som_release(buf);
            (buf, cap) = (grown, new_cap);
        }
        if self.guest_write_bytes(buf + used, &value).is_none() { return 0; }
        for (i, w) in [buf, cap, need, owned].into_iter().enumerate() {
            let _ = self.guest_write::<u32>(vb + 4 + 4 * i as u32, w);
        }
        1
    }

    /// `somVaBuf_get_valist(vb, ap)`: `*ap` leads to the arguments (a
    /// va_list of either form: its first word is the pointer).
    pub(crate) fn som_va_buf_get_valist(&self, vb: u32, ap: u32) -> u32 {
        if let Some([buf, ..]) = self.som_va_buf(vb) { let _ = self.guest_write::<u32>(ap, buf); }
        0
    }

    /// `somVaBuf_destroy(vb)`: its buffer, and the somVaBuf itself when
    /// `somVaBuf_create` made it, freed.
    pub(crate) fn som_va_buf_destroy(&self, vb: u32) -> u32 {
        let Some([buf, _, _, owned]) = self.som_va_buf(vb) else { return 0 };
        self.shared.som.lock_or_recover().va_buffers.remove(&buf);
        self.som_release(buf);
        let _ = self.guest_write::<u32>(vb, 0);
        if owned != 0 { self.som_release(vb); }
        0
    }

    /// `somvalistGetTarget(ap)` / `somvalistSetTarget(ap, val)`: the first
    /// word of the arguments.
    pub(crate) fn som_valist_target(&self, ap: u32, set: Option<u32>) -> u32 {
        let Some(args) = self.som_va_args(ap, None) else { return 0 };
        match set {
            Some(v) => { let _ = self.guest_write::<u32>(args, v); 0 }
            None => self.guest_word(args).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_widened_as_in_a_va_list() {
        let raw = |b: &[u8]| { let mut r = [0u8; 8]; r[..b.len()].copy_from_slice(b); r };
        assert_eq!(widen(tk::SHORT, &raw(&(-2i16).to_le_bytes())), Some((-2i32).to_le_bytes().to_vec()));
        assert_eq!(widen(tk::USHORT, &raw(&0xFFFEu16.to_le_bytes())), Some(0xFFFEu32.to_le_bytes().to_vec()));
        assert_eq!(widen(tk::CHAR, &raw(&[0xFF])), Some((-1i32).to_le_bytes().to_vec()), "char as signed");
        assert_eq!(widen(tk::OCTET, &raw(&[0xFF])), Some(255u32.to_le_bytes().to_vec()));
        assert_eq!(widen(tk::FLOAT, &raw(&1.5f32.to_le_bytes())), Some(1.5f64.to_le_bytes().to_vec()), "float as double");
        assert_eq!(widen(tk::DOUBLE, &raw(&2.25f64.to_le_bytes())).map(|v| v.len()), Some(8));
        assert_eq!(widen(tk::POINTER, &raw(&[1, 2, 3, 4])), Some(vec![1, 2, 3, 4]));
        assert_eq!(widen(15, &raw(&[])), None, "a struct is not taken");
    }

    /// somVaBuf_create / add / get_valist / destroy on the mock: widened
    /// arguments appended, the buffer grows, both va_list forms reach the
    /// target, and a destroyed buffer is no longer one.
    #[test]
    fn va_bufs_build_va_lists() {
        let l = Loader::new_mock();
        let at = 0x0020_0000u32;
        let vb = l.som_va_buf_create(0, 0);
        assert_ne!(vb, 0);
        l.guest_write::<u32>(at, 0xAB).unwrap();
        assert_eq!(l.som_va_buf_add(vb, at, tk::POINTER), 1);
        l.guest_write::<u16>(at, (-3i16) as u16).unwrap();
        assert_eq!(l.som_va_buf_add(vb, at, tk::SHORT), 1);
        l.guest_write::<f32>(at, 0.25).unwrap();
        assert_eq!(l.som_va_buf_add(vb, at, tk::FLOAT), 1);
        assert_eq!(l.som_va_buf_add(vb, at, 15), 0, "tk_struct is not taken");
        assert_eq!(l.som_va_buf_add(at, at, tk::LONG), 0, "not a somVaBuf");
        for _ in 0..20 { assert_eq!(l.som_va_buf_add(vb, at, tk::DOUBLE), 1, "it grows"); }
        let ap = at + 0x100;
        l.som_va_buf_get_valist(vb, ap);
        let args = l.guest_read::<u32>(ap).unwrap();
        assert_eq!(l.guest_read::<u32>(args), Some(0xAB));
        assert_eq!(l.guest_read::<i32>(args + 4), Some(-3), "short widened");
        assert_eq!(l.guest_read::<f64>(args + 8), Some(0.25), "float widened");
        assert_eq!(l.som_valist_target(args, None), 0xAB, "GCC's form");
        assert_eq!(l.som_valist_target(ap, None), 0xAB, "Open Watcom's form");
        l.som_valist_target(ap, Some(0xCD));
        assert_eq!(l.guest_read::<u32>(args), Some(0xCD));
        l.som_va_buf_destroy(vb);
        assert_eq!(l.som_va_buf_add(vb, at, tk::LONG), 0, "destroyed");
    }

    /// Hostile somVaBufs and arguments (security audit 2026-11-25): a
    /// header claiming more room than the buffer has, an argument in
    /// Warpine's protected memory, an mdPtr at the top of the address
    /// space, and a root method of more arguments than a call passes.
    #[test]
    fn hostile_va_bufs_and_arguments_are_refused() {
        let l = Loader::new_mock();
        let at = 0x0020_0000u32;
        let vb = l.som_va_buf_create(0, 0);
        l.guest_write::<u32>(vb + 8, 0x10000).unwrap(); // capacity
        l.guest_write::<u32>(vb + 12, 0x8000).unwrap(); // used, past the real buffer
        assert_eq!(l.som_va_buf_add(vb, at, tk::LONG), 0, "the kernel's capacity counts");
        l.guest_write::<u32>(vb + 12, 0).unwrap();
        let gdt = crate::loader::constants::SYSTEM_AREA_BASE;
        assert_eq!(l.som_va_buf_add(vb, gdt, tk::DOUBLE), 0, "protected memory is not read");
        assert!(matches!(l.som_apply(0, &[at, at, 0xFFFF_FFF8, at, 0, 0, 0, 0, 0, 0]), ApiResult::Normal(0)));
        // somInitClass (seven arguments) through somApply.
        let (md, args) = (at + 0x100, at + 0x200);
        l.guest_write::<u32>(args, at).unwrap();
        l.guest_write::<u32>(md + 12, 0).unwrap();
        l.guest_write::<u32>(md + 16, thunk(3310)).unwrap();
        assert!(matches!(l.som_apply(0, &[at, at + 0x300, md, args, 0, 0, 0, 0, 0, 0]), ApiResult::Normal(0)),
                "more arguments than a call passes: not applied");
        l.som_va_buf_destroy(vb);
    }

    /// somApply applies root-class methods only (security audit
    /// 2026-11-25): given its own thunk as the method, it would apply
    /// itself without end; that is refused, a root method is not.
    #[test]
    fn som_apply_takes_only_root_class_methods() {
        let l = Loader::new_mock();
        let obj = 0x0020_0000u32;
        let (md, args, ret) = (obj + 0x100, obj + 0x200, obj + 0x300);
        // The arguments: the target first (somApply's own four, for the loop).
        for (i, w) in [obj, ret, md, args].into_iter().enumerate() {
            l.guest_write::<u32>(args + 4 * i as u32, w).unwrap();
        }
        let apply = |method: u32| {
            l.guest_write::<u32>(md + 12, 0).unwrap(); // a token without an apply stub
            l.guest_write::<u32>(md + 16, method).unwrap();
            l.som_apply(0, &[obj, ret, md, args, 0, 0, 0, 0, 0, 0])
        };
        assert!(matches!(apply(thunk(3102)), ApiResult::Normal(0)), "somApply itself is no root method");
        assert!(matches!(apply(thunk(3103)), ApiResult::Normal(0)), "nor somVaBuf_create");
        assert!(!matches!(apply(thunk(3204)), ApiResult::Normal(_)), "somGetClassName is: a call is made");
        assert!(super::super::som::is_root_method(3224), "somDispatch is a method (nesting bounds it)");
    }

    /// A va_list is taken in whichever form leads to the target: GCC's
    /// (a pointer to the arguments) or Open Watcom's (a pointer to it).
    #[test]
    fn both_va_list_forms_are_taken() {
        let mem = std::collections::HashMap::from([(0x100u32, 0x5000u32), (0x200, 0x100), (0x300, 7)]);
        let read = |a: u32| mem.get(&a).copied();
        let is_target = |_args: u32, first: u32| first == 0x5000;
        assert_eq!(va_args(0x100, &read, &is_target), Some(0x100), "GCC: the arguments");
        assert_eq!(va_args(0x200, &read, &is_target), Some(0x100), "Open Watcom: a pointer to them");
        assert_eq!(va_args(0x300, &read, &is_target), None, "neither");
        assert_eq!(va_args(0, &read, &is_target), None);
    }
}

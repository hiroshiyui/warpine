// SPDX-License-Identifier: GPL-3.0-or-later
// typeinfo.cc - run-time type information for Warpine's C++ runtime
// (ADR-0103): std::type_info, the ABI's type_info classes (whose virtual
// tables the compilers refer to), the type_info objects of the fundamental
// types, catch matching and dynamic_cast. From the Itanium C++ ABI,
// section 2.9 (run-time type information).
#include "cxxabi_internal.h"
#include <cstring>
#include <cstdint>

namespace std {

type_info::~type_info() {}

// Two type_info objects name the same type when their names are equal;
// a name starting with '*' belongs to one program and compares by address.
bool type_info::operator==(const type_info &other) const noexcept
{
    if (__type_name == other.__type_name) return true;
    if (__type_name[0] == '*' || other.__type_name[0] == '*') return false;
    return std::strcmp(__type_name, other.__type_name) == 0;
}

bool type_info::before(const type_info &other) const noexcept
{
    return std::strcmp(name(), other.name()) < 0;
}

size_t type_info::hash_code() const noexcept
{
    size_t h = 2166136261u;
    for (const char *p = name(); *p; p++) h = (h ^ static_cast<unsigned char>(*p)) * 16777619u;
    return h;
}

bad_cast::~bad_cast() noexcept {}
const char *bad_cast::what() const noexcept { return "std::bad_cast"; }
bad_typeid::~bad_typeid() noexcept {}
const char *bad_typeid::what() const noexcept { return "std::bad_typeid"; }

} // namespace std

namespace __cxxabiv1 {

__shim_type_info::~__shim_type_info() {}
tinfo_kind __shim_type_info::kind() const { return tinfo_kind::other; }
// The key function: its definition makes the compiler emit the type_info
// objects of the fundamental types (int, const char *, ...) here.
__fundamental_type_info::~__fundamental_type_info() {}
tinfo_kind __fundamental_type_info::kind() const { return tinfo_kind::fundamental; }
__array_type_info::~__array_type_info() {}
__function_type_info::~__function_type_info() {}
tinfo_kind __function_type_info::kind() const { return tinfo_kind::function; }
__enum_type_info::~__enum_type_info() {}
__class_type_info::~__class_type_info() {}
tinfo_kind __class_type_info::kind() const { return tinfo_kind::class_type; }
__si_class_type_info::~__si_class_type_info() {}
tinfo_kind __si_class_type_info::kind() const { return tinfo_kind::si_class; }
__vmi_class_type_info::~__vmi_class_type_info() {}
tinfo_kind __vmi_class_type_info::kind() const { return tinfo_kind::vmi_class; }
__pbase_type_info::~__pbase_type_info() {}
__pointer_type_info::~__pointer_type_info() {}
tinfo_kind __pointer_type_info::kind() const { return tinfo_kind::pointer; }
__pointer_to_member_type_info::~__pointer_to_member_type_info() {}
tinfo_kind __pointer_to_member_type_info::kind() const { return tinfo_kind::member_pointer; }

static tinfo_kind kind_of(const std::type_info *t)
{
    return static_cast<const __shim_type_info *>(t)->kind();
}

// The offset of a base class subobject: fixed, or for a virtual base read
// from the derived object's virtual table.
static const char *base_address(const char *obj, long offset_flags)
{
    long offset = offset_flags >> __base_class_type_info::__offset_shift;
    if (offset_flags & __base_class_type_info::__virtual_mask) {
        const char *vtable = *reinterpret_cast<const char *const *>(obj);
        offset = *reinterpret_cast<const ptrdiff_t *>(vtable + offset);
    }
    return obj + offset;
}

// Search the class `type` at `obj` for a public base of type `target`;
// `obj` may be null (only whether there is one is asked, with no virtual
// bases to read). Returns the first one found.
static bool find_public_base(const __class_type_info *type, const char *obj, const std::type_info *target,
                             const char **found)
{
    if (*type == *target) { *found = obj; return true; }
    switch (type->kind()) {
    case tinfo_kind::si_class: {
        auto si = static_cast<const __si_class_type_info *>(type);
        return find_public_base(si->__base_type, obj, target, found);
    }
    case tinfo_kind::vmi_class: {
        auto vmi = static_cast<const __vmi_class_type_info *>(type);
        for (unsigned i = 0; i < vmi->__base_count; i++) {
            const __base_class_type_info &b = vmi->__base_info[i];
            if (!(b.__offset_flags & __base_class_type_info::__public_mask)) continue;
            if (!obj && (b.__offset_flags & __base_class_type_info::__virtual_mask)) {
                if (find_public_base(b.__base_type, nullptr, target, found)) return true;
                continue;
            }
            const char *base = obj ? base_address(obj, b.__offset_flags) : nullptr;
            if (find_public_base(b.__base_type, base, target, found)) return true;
        }
        return false;
    }
    default:
        return false;
    }
}

// Pointer conversions a handler accepts: the same type with at least the
// thrown pointer's qualifiers, void * for object pointers, a public base.
static bool can_catch_pointer(const __pbase_type_info *c, const __pbase_type_info *t, void **thrown)
{
    if (t->__flags & ~c->__flags & (__pbase_type_info::__const_mask | __pbase_type_info::__volatile_mask))
        return false;
    const std::type_info *cp = c->__pointee, *tp = t->__pointee;
    if (*cp == *tp) return true;
    // void * catches any object pointer (not a function pointer).
    if (std::strcmp(cp->name(), "v") == 0) return kind_of(tp) != tinfo_kind::function;
    tinfo_kind ck = kind_of(cp), tk = kind_of(tp);
    if (ck == tinfo_kind::pointer && tk == tinfo_kind::pointer) {
        // Deeper levels convert only when every outer level is const.
        if (!(c->__flags & __pbase_type_info::__const_mask)) return false;
        void *dummy = nullptr;
        return can_catch_pointer(static_cast<const __pbase_type_info *>(cp),
                                 static_cast<const __pbase_type_info *>(tp), &dummy);
    }
    if ((tk == tinfo_kind::class_type || tk == tinfo_kind::si_class || tk == tinfo_kind::vmi_class) &&
        (ck == tinfo_kind::class_type || ck == tinfo_kind::si_class || ck == tinfo_kind::vmi_class)) {
        const char *found = nullptr;
        if (!find_public_base(static_cast<const __class_type_info *>(tp), static_cast<const char *>(*thrown), cp, &found))
            return false;
        *thrown = const_cast<char *>(found);
        return true;
    }
    return false;
}

bool can_catch(const std::type_info *catch_type, const std::type_info *thrown_type, void **thrown)
{
    if (!catch_type) return true;            // catch (...)
    tinfo_kind ck = kind_of(catch_type), tk = kind_of(thrown_type);
    if (tk == tinfo_kind::pointer) *thrown = *static_cast<void **>(*thrown);
    if (*catch_type == *thrown_type) return true;
    // nullptr_t converts to any pointer.
    if (std::strcmp(thrown_type->name(), "Dn") == 0 && (ck == tinfo_kind::pointer || ck == tinfo_kind::member_pointer)) {
        *thrown = nullptr;
        return true;
    }
    if (ck == tinfo_kind::pointer && tk == tinfo_kind::pointer)
        return can_catch_pointer(static_cast<const __pbase_type_info *>(catch_type),
                                 static_cast<const __pbase_type_info *>(thrown_type), thrown);
    if ((tk == tinfo_kind::si_class || tk == tinfo_kind::vmi_class) &&
        (ck == tinfo_kind::class_type || ck == tinfo_kind::si_class || ck == tinfo_kind::vmi_class)) {
        const char *found = nullptr;
        if (!find_public_base(static_cast<const __class_type_info *>(thrown_type), static_cast<const char *>(*thrown),
                              catch_type, &found))
            return false;
        *thrown = const_cast<char *>(found);
        return true;
    }
    return false;
}

// Every subobject of type `target` in the class `type` at `obj`, reached
// through public bases only when `public_only`; up to `max` of them.
static int collect(const __class_type_info *type, const char *obj, const __class_type_info *target, bool public_path,
                   const char **out, int n, int max)
{
    if (*type == *target && public_path) {
        for (int i = 0; i < n; i++) if (out[i] == obj) return n;   // a virtual base met twice
        if (n < max) out[n++] = obj;
        return n;
    }
    if (type->kind() == tinfo_kind::si_class) {
        return collect(static_cast<const __si_class_type_info *>(type)->__base_type, obj, target, public_path, out, n, max);
    }
    if (type->kind() == tinfo_kind::vmi_class) {
        auto vmi = static_cast<const __vmi_class_type_info *>(type);
        for (unsigned i = 0; i < vmi->__base_count; i++) {
            const __base_class_type_info &b = vmi->__base_info[i];
            bool pub = public_path && (b.__offset_flags & __base_class_type_info::__public_mask);
            n = collect(b.__base_type, base_address(obj, b.__offset_flags), target, pub, out, n, max);
        }
    }
    return n;
}

// Whether the subobject `sub` of type `sub_type` is a public base inside
// the subobject `obj` of type `type`.
static bool contains(const __class_type_info *type, const char *obj, const __class_type_info *sub_type, const char *sub)
{
    const char *found[8];
    int n = collect(type, obj, sub_type, true, found, 0, 8);
    for (int i = 0; i < n; i++) if (found[i] == sub) return true;
    return false;
}

extern "C" void *__dynamic_cast(const void *src_ptr, const __class_type_info *src_type,
                                const __class_type_info *dst_type, ptrdiff_t src2dst)
{
    (void)src2dst;
    // The most derived object: its offset and type_info sit before the
    // address point of the virtual table.
    const char *const *vtable = *static_cast<const char *const *const *>(src_ptr);
    ptrdiff_t to_top = reinterpret_cast<const ptrdiff_t *>(vtable)[-2];
    const __class_type_info *whole_type = reinterpret_cast<const __class_type_info *const *>(vtable)[-1];
    const char *whole = static_cast<const char *>(src_ptr) + to_top;
    if (*whole_type == *dst_type) return const_cast<char *>(whole);

    const char *found[8];
    int n = collect(whole_type, whole, dst_type, true, found, 0, 8);
    if (n == 1) return const_cast<char *>(found[0]);
    // Several: a downcast to the one holding the source subobject.
    const char *pick = nullptr;
    for (int i = 0; i < n; i++) {
        if (contains(dst_type, found[i], src_type, static_cast<const char *>(src_ptr))) {
            if (pick) return nullptr;
            pick = found[i];
        }
    }
    return const_cast<char *>(pick);
}

} // namespace __cxxabiv1

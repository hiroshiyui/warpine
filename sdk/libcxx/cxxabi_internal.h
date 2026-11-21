// SPDX-License-Identifier: GPL-3.0-or-later
// cxxabi_internal.h - the Itanium C++ ABI's type information classes for
// Warpine's C++ runtime (ADR-0103). Their data members are the ABI's: the
// compilers emit type_info objects of these classes for every type, and
// their virtual tables, which the compilers refer to by name, come from
// typeinfo.cc. The virtual functions after the destructor are the
// runtime's own (only it calls them).
#ifndef WARPINE_CXXABI_INTERNAL_H
#define WARPINE_CXXABI_INTERNAL_H

#include <typeinfo>
#include <cstddef>
#include <unwind.h>

namespace __cxxabiv1 {

class __class_type_info;

// What kind of type a type_info describes, for the runtime's matching.
enum class tinfo_kind { other, fundamental, function, pointer, member_pointer, class_type, si_class, vmi_class };

struct __shim_type_info : std::type_info {
    explicit __shim_type_info(const char *n) : std::type_info(n) {}
    ~__shim_type_info() override;
    virtual tinfo_kind kind() const;
};

class __fundamental_type_info : public __shim_type_info {
public:
    explicit __fundamental_type_info(const char *n) : __shim_type_info(n) {}
    ~__fundamental_type_info() override;
    tinfo_kind kind() const override;
};

class __array_type_info : public __shim_type_info {
public:
    explicit __array_type_info(const char *n) : __shim_type_info(n) {}
    ~__array_type_info() override;
};

class __function_type_info : public __shim_type_info {
public:
    explicit __function_type_info(const char *n) : __shim_type_info(n) {}
    ~__function_type_info() override;
    tinfo_kind kind() const override;
};

class __enum_type_info : public __shim_type_info {
public:
    explicit __enum_type_info(const char *n) : __shim_type_info(n) {}
    ~__enum_type_info() override;
};

class __class_type_info : public __shim_type_info {
public:
    explicit __class_type_info(const char *n) : __shim_type_info(n) {}
    ~__class_type_info() override;
    tinfo_kind kind() const override;
};

class __si_class_type_info : public __class_type_info {
public:
    const __class_type_info *__base_type;
    explicit __si_class_type_info(const char *n, const __class_type_info *b) : __class_type_info(n), __base_type(b) {}
    ~__si_class_type_info() override;
    tinfo_kind kind() const override;
};

struct __base_class_type_info {
    const __class_type_info *__base_type;
    long __offset_flags;
    enum __offset_flags_masks { __virtual_mask = 0x1, __public_mask = 0x2, __offset_shift = 8 };
};

class __vmi_class_type_info : public __class_type_info {
public:
    unsigned int __flags;
    unsigned int __base_count;
    __base_class_type_info __base_info[1];
    enum __flags_masks { __non_diamond_repeat_mask = 0x1, __diamond_shaped_mask = 0x2 };
    explicit __vmi_class_type_info(const char *n) : __class_type_info(n), __flags(0), __base_count(0), __base_info() {}
    ~__vmi_class_type_info() override;
    tinfo_kind kind() const override;
};

class __pbase_type_info : public __shim_type_info {
public:
    unsigned int __flags;
    const std::type_info *__pointee;
    enum __masks {
        __const_mask = 0x1, __volatile_mask = 0x2, __restrict_mask = 0x4, __incomplete_mask = 0x8,
        __incomplete_class_mask = 0x10, __transaction_safe_mask = 0x20, __noexcept_mask = 0x40
    };
    explicit __pbase_type_info(const char *n) : __shim_type_info(n), __flags(0), __pointee(nullptr) {}
    ~__pbase_type_info() override;
};

class __pointer_type_info : public __pbase_type_info {
public:
    explicit __pointer_type_info(const char *n) : __pbase_type_info(n) {}
    ~__pointer_type_info() override;
    tinfo_kind kind() const override;
};

class __pointer_to_member_type_info : public __pbase_type_info {
public:
    const __class_type_info *__context;
    explicit __pointer_to_member_type_info(const char *n) : __pbase_type_info(n), __context(nullptr) {}
    ~__pointer_to_member_type_info() override;
    tinfo_kind kind() const override;
};

// Whether a handler for `catch_type` catches an exception of
// `thrown_type` at `*thrown` (the object, or for a pointer the pointer's
// value); on a match *thrown is what the handler gets (adjusted to a
// base class, a pointer's value adjusted the same way).
bool can_catch(const std::type_info *catch_type, const std::type_info *thrown_type, void **thrown);

} // namespace __cxxabiv1

namespace abi = __cxxabiv1;

#endif

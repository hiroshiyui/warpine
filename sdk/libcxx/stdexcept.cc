// SPDX-License-Identifier: GPL-3.0-or-later
// stdexcept.cc - the standard exception classes for Warpine's C++ runtime
// (ADR-0103). Each keeps its own copy of the message; a copy that cannot
// be allocated keeps no message.
#include <stdexcept>
#include <cstdlib>
#include <cstring>

namespace {

char *copy(const char *s)
{
    if (!s) return nullptr;
    std::size_t n = std::strlen(s) + 1;
    char *p = static_cast<char *>(std::malloc(n));
    if (p) std::memcpy(p, s, n);
    return p;
}

} // namespace

namespace std {

#define WARPINE_MESSAGE_ERROR(name)                                                          \
    name::name(const char *what) : msg_(copy(what)) {}                                       \
    name::name(const name &other) noexcept : exception(other), msg_(copy(other.msg_)) {}     \
    name &name::operator=(const name &other) noexcept                                        \
    {                                                                                        \
        if (this != &other) { std::free(msg_); msg_ = copy(other.msg_); }                    \
        return *this;                                                                        \
    }                                                                                        \
    name::~name() noexcept { std::free(msg_); }                                              \
    const char *name::what() const noexcept { return msg_ ? msg_ : #name; }

WARPINE_MESSAGE_ERROR(logic_error)
WARPINE_MESSAGE_ERROR(runtime_error)

domain_error::~domain_error() noexcept {}
invalid_argument::~invalid_argument() noexcept {}
length_error::~length_error() noexcept {}
out_of_range::~out_of_range() noexcept {}
range_error::~range_error() noexcept {}
overflow_error::~overflow_error() noexcept {}
underflow_error::~underflow_error() noexcept {}

} // namespace std

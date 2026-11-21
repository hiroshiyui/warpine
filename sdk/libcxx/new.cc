// SPDX-License-Identifier: GPL-3.0-or-later
// new.cc - operator new and delete for Warpine's C++ runtime (ADR-0103), on
// malloc: a failed allocation calls the new handler, if any, and tries
// again, or throws std::bad_alloc (the nothrow forms return null).
#include <new>
#include <cstdlib>
#include <cstdint>

namespace std {

const nothrow_t nothrow{};
bad_alloc::~bad_alloc() noexcept {}
const char *bad_alloc::what() const noexcept { return "std::bad_alloc"; }
bad_array_new_length::~bad_array_new_length() noexcept {}
const char *bad_array_new_length::what() const noexcept { return "std::bad_array_new_length"; }

static new_handler handler;
new_handler set_new_handler(new_handler h) noexcept { return __atomic_exchange_n(&handler, h, __ATOMIC_ACQ_REL); }
new_handler get_new_handler() noexcept { return __atomic_load_n(&handler, __ATOMIC_ACQUIRE); }

} // namespace std

void *operator new(std::size_t size)
{
    if (size == 0) size = 1;
    for (;;) {
        if (void *p = std::malloc(size)) return p;
        std::new_handler h = std::get_new_handler();
        if (!h) throw std::bad_alloc();
        h();
    }
}

void *operator new[](std::size_t size) { return ::operator new(size); }

void *operator new(std::size_t size, const std::nothrow_t &) noexcept
{
    try { return ::operator new(size); } catch (...) { return nullptr; }
}

void *operator new[](std::size_t size, const std::nothrow_t &) noexcept
{
    try { return ::operator new(size); } catch (...) { return nullptr; }
}

// Over-aligned objects: a larger block, the pointer malloc gave kept just
// before the aligned one.
void *operator new(std::size_t size, std::align_val_t align)
{
    std::size_t a = static_cast<std::size_t>(align);
    if (a < sizeof(void *)) a = sizeof(void *);
    char *raw = static_cast<char *>(::operator new(size + a + sizeof(void *)));
    uintptr_t at = (reinterpret_cast<uintptr_t>(raw) + sizeof(void *) + a - 1) & ~(uintptr_t)(a - 1);
    reinterpret_cast<void **>(at)[-1] = raw;
    return reinterpret_cast<void *>(at);
}

void *operator new[](std::size_t size, std::align_val_t align) { return ::operator new(size, align); }

void operator delete(void *p) noexcept { std::free(p); }
void operator delete[](void *p) noexcept { std::free(p); }
void operator delete(void *p, std::size_t) noexcept { std::free(p); }
void operator delete[](void *p, std::size_t) noexcept { std::free(p); }
void operator delete(void *p, const std::nothrow_t &) noexcept { std::free(p); }
void operator delete[](void *p, const std::nothrow_t &) noexcept { std::free(p); }

void operator delete(void *p, std::align_val_t) noexcept
{
    if (p) ::operator delete(static_cast<void **>(p)[-1]);
}

void operator delete[](void *p, std::align_val_t a) noexcept { ::operator delete(p, a); }
void operator delete(void *p, std::size_t, std::align_val_t a) noexcept { ::operator delete(p, a); }
void operator delete[](void *p, std::size_t, std::align_val_t a) noexcept { ::operator delete(p, a); }

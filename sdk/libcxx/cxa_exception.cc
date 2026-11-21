// SPDX-License-Identifier: GPL-3.0-or-later
// cxa_exception.cc - C++ exceptions for Warpine's C++ runtime (ADR-0103):
// the Itanium C++ ABI's level II (__cxa_allocate_exception, __cxa_throw,
// __cxa_begin_catch / __cxa_end_catch, __cxa_rethrow), the personality
// routine __gxx_personality_v0 reading GCC's LSDA format (call-site,
// action and type tables), std::terminate, and the uncaught count.
#include "cxxabi_internal.h"
#include <exception>
#include <new>
#include <cstdlib>
#include <cstring>
#include <cstdint>

#include <os2.h>

extern "C" void *__libc_cxx_globals(void);
extern "C" uint32_t __warpine_read_encoded(const unsigned char **p, unsigned char enc, uint32_t func);

namespace __cxxabiv1 {

// The header before every thrown object (the ABI's __cxa_exception, as
// far as this runtime uses it).
struct __cxa_exception {
    const std::type_info *exceptionType;
    void (*exceptionDestructor)(void *);
    std::terminate_handler terminateHandler;
    __cxa_exception *nextException;
    int handlerCount;                    // negative: rethrown
    int handlerSwitchValue;
    const unsigned char *actionRecord;
    const unsigned char *languageSpecificData;
    uintptr_t catchTemp;                 // the landing pad
    void *adjustedPtr;
    _Unwind_Exception unwindHeader;
};

struct __cxa_eh_globals {
    __cxa_exception *caughtExceptions;
    unsigned int uncaughtExceptions;
};

// "WRPN" "C++\0": an exception this runtime threw.
static const uint64_t warpine_class = 0x0043'2B2B'4E50'5257ull;

static __cxa_eh_globals *globals()
{
    return static_cast<__cxa_eh_globals *>(__libc_cxx_globals());
}

static __cxa_exception *header_of(_Unwind_Exception *ue)
{
    return reinterpret_cast<__cxa_exception *>(reinterpret_cast<char *>(ue) - offsetof(__cxa_exception, unwindHeader));
}

static __cxa_exception *header_of_object(void *obj)
{
    return static_cast<__cxa_exception *>(obj) - 1;
}

static void *object_of(__cxa_exception *h) { return h + 1; }

} // namespace __cxxabiv1

using namespace __cxxabiv1;

// ── std::terminate and friends ──────────────────────────────────────────

[[noreturn]] static void default_terminate()
{
    static const char msg[] = "terminate called: an exception was not caught\r\n";
    ULONG written;
    DosWrite(2, msg, sizeof msg - 1, &written);
    std::abort();
}

static std::terminate_handler terminate_fn = default_terminate;

namespace std {

exception::~exception() noexcept {}
const char *exception::what() const noexcept { return "std::exception"; }
bad_exception::~bad_exception() noexcept {}
const char *bad_exception::what() const noexcept { return "std::bad_exception"; }

terminate_handler set_terminate(terminate_handler f) noexcept
{
    return __atomic_exchange_n(&terminate_fn, f ? f : default_terminate, __ATOMIC_ACQ_REL);
}

terminate_handler get_terminate() noexcept { return __atomic_load_n(&terminate_fn, __ATOMIC_ACQUIRE); }

[[noreturn]] void terminate() noexcept
{
    get_terminate()();
    std::abort();     // a handler must not return
}

int uncaught_exceptions() noexcept { return static_cast<int>(globals()->uncaughtExceptions); }
bool uncaught_exception() noexcept { return globals()->uncaughtExceptions != 0; }

} // namespace std

// ── Throwing ────────────────────────────────────────────────────────────

extern "C" {

void *__cxa_allocate_exception(size_t size) noexcept
{
    void *p = std::malloc(sizeof(__cxa_exception) + size);
    if (!p) std::terminate();
    std::memset(p, 0, sizeof(__cxa_exception));
    return static_cast<__cxa_exception *>(p) + 1;
}

void __cxa_free_exception(void *obj) noexcept
{
    std::free(header_of_object(obj));
}

static void destroy(__cxa_exception *h)
{
    if (h->exceptionDestructor) h->exceptionDestructor(object_of(h));
    std::free(h);
}

static void cleanup(_Unwind_Reason_Code, _Unwind_Exception *ue)
{
    destroy(header_of(ue));
}

void *__cxa_begin_catch(void *exc) noexcept;

[[noreturn]] void __cxa_throw(void *obj, void *type, void (*dest)(void *))
{
    __cxa_exception *h = header_of_object(obj);
    h->exceptionType = static_cast<std::type_info *>(type);
    h->exceptionDestructor = dest;
    h->terminateHandler = std::get_terminate();
    h->unwindHeader.exception_class = warpine_class;
    h->unwindHeader.exception_cleanup = cleanup;
    globals()->uncaughtExceptions++;
    _Unwind_RaiseException(&h->unwindHeader);
    // No handler: terminate, with the exception counted as caught.
    __cxa_begin_catch(&h->unwindHeader);
    std::terminate();
}

void *__cxa_get_exception_ptr(void *exc) noexcept
{
    return header_of(static_cast<_Unwind_Exception *>(exc))->adjustedPtr;
}

void *__cxa_begin_catch(void *exc) noexcept
{
    _Unwind_Exception *ue = static_cast<_Unwind_Exception *>(exc);
    __cxa_eh_globals *g = globals();
    if (ue->exception_class != warpine_class) {
        // Not ours: it can only be caught by catch (...) and not rethrown.
        if (g->caughtExceptions) std::terminate();
        return nullptr;
    }
    __cxa_exception *h = header_of(ue);
    h->handlerCount = h->handlerCount < 0 ? -h->handlerCount + 1 : h->handlerCount + 1;
    if (h != g->caughtExceptions) {
        h->nextException = g->caughtExceptions;
        g->caughtExceptions = h;
    }
    if (g->uncaughtExceptions) g->uncaughtExceptions--;
    return h->adjustedPtr;
}

void __cxa_end_catch() noexcept
{
    __cxa_eh_globals *g = globals();
    __cxa_exception *h = g->caughtExceptions;
    if (!h) return;
    if (h->handlerCount < 0) {
        // Rethrown: it leaves the caught list when its last handler ends.
        if (++h->handlerCount == 0) g->caughtExceptions = h->nextException;
    } else if (--h->handlerCount == 0) {
        g->caughtExceptions = h->nextException;
        // Through its cleanup: the runtime of the module that threw it
        // allocated it, and frees it with its own heap (ADR-0107).
        _Unwind_DeleteException(&h->unwindHeader);
    }
}

[[noreturn]] void __cxa_rethrow()
{
    __cxa_eh_globals *g = globals();
    __cxa_exception *h = g->caughtExceptions;
    if (!h) std::terminate();
    h->handlerCount = -h->handlerCount;
    g->uncaughtExceptions++;
    _Unwind_Resume_or_Rethrow(&h->unwindHeader);
    __cxa_begin_catch(&h->unwindHeader);
    std::terminate();
}

std::type_info *__cxa_current_exception_type() noexcept
{
    __cxa_exception *h = globals()->caughtExceptions;
    return h ? const_cast<std::type_info *>(h->exceptionType) : nullptr;
}

[[noreturn]] void __cxa_call_unexpected(void *exc)
{
    __cxa_begin_catch(exc);
    std::terminate();
}

[[noreturn]] void __cxa_call_terminate(void *exc) noexcept
{
    if (exc) __cxa_begin_catch(exc);
    std::terminate();
}

[[noreturn]] void __cxa_bad_cast() { throw std::bad_cast(); }
[[noreturn]] void __cxa_bad_typeid() { throw std::bad_typeid(); }
[[noreturn]] void __cxa_throw_bad_array_new_length() { throw std::bad_array_new_length(); }

[[noreturn]] void __cxa_pure_virtual()
{
    static const char msg[] = "pure virtual function called\r\n";
    ULONG written;
    DosWrite(2, msg, sizeof msg - 1, &written);
    std::terminate();
}

[[noreturn]] void __cxa_deleted_virtual()
{
    static const char msg[] = "deleted virtual function called\r\n";
    ULONG written;
    DosWrite(2, msg, sizeof msg - 1, &written);
    std::terminate();
}

// ── Static locals: one initialisation, even with threads ────────────────
// The guard's first byte is 1 once initialised; its second is 1 while a
// thread initialises it.

int __cxa_guard_acquire(uint64_t *guard)
{
    unsigned char *g = reinterpret_cast<unsigned char *>(guard);
    for (;;) {
        if (__atomic_load_n(&g[0], __ATOMIC_ACQUIRE)) return 0;
        unsigned char expected = 0;
        if (__atomic_compare_exchange_n(&g[1], &expected, 1, false, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
            if (__atomic_load_n(&g[0], __ATOMIC_ACQUIRE)) {
                __atomic_store_n(&g[1], 0, __ATOMIC_RELEASE);
                return 0;
            }
            return 1;
        }
        DosSleep(1);
    }
}

void __cxa_guard_release(uint64_t *guard) noexcept
{
    unsigned char *g = reinterpret_cast<unsigned char *>(guard);
    __atomic_store_n(&g[0], 1, __ATOMIC_RELEASE);
    __atomic_store_n(&g[1], 0, __ATOMIC_RELEASE);
}

void __cxa_guard_abort(uint64_t *guard) noexcept
{
    __atomic_store_n(&reinterpret_cast<unsigned char *>(guard)[1], 0, __ATOMIC_RELEASE);
}

} // extern "C"

// ── The personality routine ─────────────────────────────────────────────

namespace {

const unsigned char DW_EH_PE_omit = 0xFF;

uint32_t read_uleb(const unsigned char **p)
{
    uint32_t v = 0;
    unsigned shift = 0;
    unsigned char b;
    do {
        b = *(*p)++;
        if (shift < 32) v |= static_cast<uint32_t>(b & 0x7F) << shift;
        shift += 7;
    } while (b & 0x80);
    return v;
}

int32_t read_sleb(const unsigned char **p)
{
    uint32_t v = 0;
    unsigned shift = 0;
    unsigned char b;
    do {
        b = *(*p)++;
        if (shift < 32) v |= static_cast<uint32_t>(b & 0x7F) << shift;
        shift += 7;
    } while (b & 0x80);
    if (shift < 32 && (b & 0x40)) v |= ~0u << shift;
    return static_cast<int32_t>(v);
}

unsigned encoded_size(unsigned char enc)
{
    switch (enc & 0x07) {
    case 0x02: return 2;
    case 0x04: return 8;
    default:   return 4;     // absptr, udata4, sdata4
    }
}

struct lsda_info {
    uint32_t lpstart;
    const unsigned char *ttype_base;   // null without a type table
    unsigned char ttype_enc;
    const unsigned char *call_sites, *actions;
    unsigned char cs_enc;
};

void parse_lsda(const unsigned char *p, uint32_t func, lsda_info *out)
{
    unsigned char lpstart_enc = *p++;
    out->lpstart = lpstart_enc == DW_EH_PE_omit ? func : __warpine_read_encoded(&p, lpstart_enc, func);
    out->ttype_enc = *p++;
    out->ttype_base = nullptr;
    if (out->ttype_enc != DW_EH_PE_omit) {
        uint32_t off = read_uleb(&p);
        out->ttype_base = p + off;
    }
    out->cs_enc = *p++;
    uint32_t cs_len = read_uleb(&p);
    out->call_sites = p;
    out->actions = p + cs_len;
}

// The catch type of filter `index` (> 0).
const std::type_info *catch_type(const lsda_info &l, int32_t index)
{
    const unsigned char *p = l.ttype_base - static_cast<uint32_t>(index) * encoded_size(l.ttype_enc);
    return reinterpret_cast<const std::type_info *>(__warpine_read_encoded(&p, l.ttype_enc, 0));
}

// An exception specification (filter < 0) accepts the exception when one
// of its types catches it.
bool spec_allows(const lsda_info &l, int32_t filter, const std::type_info *thrown, void *obj)
{
    const unsigned char *p = l.ttype_base + (-filter - 1);
    for (;;) {
        uint32_t index = read_uleb(&p);
        if (index == 0) return false;
        void *adjusted = obj;
        if (can_catch(catch_type(l, static_cast<int32_t>(index)), thrown, &adjusted)) return true;
    }
}

} // namespace

extern "C" _Unwind_Reason_Code __gxx_personality_v0(int version, _Unwind_Action actions, uint64_t exc_class,
                                                    _Unwind_Exception *ue, _Unwind_Context *ctx)
{
    if (version != 1) return _URC_FATAL_PHASE1_ERROR;
    bool native = exc_class == warpine_class;
    __cxa_exception *h = native ? header_of(ue) : nullptr;

    // Phase 2 at the frame phase 1 chose: enter its handler.
    if ((actions & _UA_HANDLER_FRAME) && native) {
        _Unwind_SetGR(ctx, __builtin_eh_return_data_regno(0), reinterpret_cast<uintptr_t>(ue));
        _Unwind_SetGR(ctx, __builtin_eh_return_data_regno(1), static_cast<uintptr_t>(h->handlerSwitchValue));
        _Unwind_SetIP(ctx, h->catchTemp);
        return _URC_INSTALL_CONTEXT;
    }

    const unsigned char *lsda = reinterpret_cast<const unsigned char *>(_Unwind_GetLanguageSpecificData(ctx));
    if (!lsda) return _URC_CONTINUE_UNWIND;
    uint32_t func = _Unwind_GetRegionStart(ctx);
    // A return address follows its call; the IP of a frame that trapped
    // (ADR-0110) is the instruction itself.
    int at_insn = 0;
    uint32_t ip = _Unwind_GetIPInfo(ctx, &at_insn);
    if (!at_insn) ip -= 1;
    lsda_info l;
    parse_lsda(lsda, func, &l);

    // The call site the IP is in.
    const unsigned char *p = l.call_sites;
    uint32_t landing = 0;
    const unsigned char *action = nullptr;
    bool found = false;
    while (p < l.actions) {
        uint32_t start = __warpine_read_encoded(&p, l.cs_enc, 0);
        uint32_t len = __warpine_read_encoded(&p, l.cs_enc, 0);
        uint32_t lp = __warpine_read_encoded(&p, l.cs_enc, 0);
        uint32_t act = read_uleb(&p);
        if (ip < func + start) break;
        if (ip < func + start + len) {
            found = true;
            if (lp) landing = l.lpstart + lp;
            if (act) action = l.actions + act - 1;
            break;
        }
    }
    // Not in the table: the call may not throw (noexcept). An instruction
    // that trapped outside every call site (code built without
    // -fnon-call-exceptions) has no cleanups to run: the unwind goes on
    // past its frame (ADR-0110).
    if (!found) return at_insn ? _URC_CONTINUE_UNWIND : _URC_FATAL_PHASE1_ERROR;
    if (!landing) return _URC_CONTINUE_UNWIND;

    // Walk the action chain: the first handler that matches, or whether
    // there is a cleanup.
    bool cleanup = action == nullptr;
    int32_t handler_filter = 0;
    void *adjusted = nullptr;
    const unsigned char *handler_action = nullptr;
    for (const unsigned char *a = action; a;) {
        const unsigned char *this_action = a;
        int32_t filter = read_sleb(&a);
        const unsigned char *disp_at = a;
        int32_t disp = read_sleb(&a);
        if (filter == 0) {
            cleanup = true;
        } else if (filter > 0) {
            const std::type_info *type = catch_type(l, filter);
            void *obj = native ? object_of(h) : nullptr;
            if (!type || (native && can_catch(type, h->exceptionType, &obj))) {
                handler_filter = filter;
                adjusted = obj;
                handler_action = this_action;
                break;
            }
        } else {
            void *obj = native ? object_of(h) : nullptr;
            if (!native || !spec_allows(l, filter, h->exceptionType, obj)) {
                handler_filter = filter;
                adjusted = obj;
                handler_action = this_action;
                break;
            }
        }
        a = disp ? disp_at + disp : nullptr;
    }

    if (actions & _UA_SEARCH_PHASE) {
        if (!handler_action) return _URC_CONTINUE_UNWIND;
        if (native) {
            h->handlerSwitchValue = handler_filter;
            h->actionRecord = handler_action;
            h->languageSpecificData = lsda;
            h->catchTemp = landing;
            h->adjustedPtr = adjusted;
        }
        return _URC_HANDLER_FOUND;
    }

    // Phase 2, a frame that does not handle it: run its cleanups.
    if (!cleanup) return _URC_CONTINUE_UNWIND;
    _Unwind_SetGR(ctx, __builtin_eh_return_data_regno(0), reinterpret_cast<uintptr_t>(ue));
    _Unwind_SetGR(ctx, __builtin_eh_return_data_regno(1), 0);
    _Unwind_SetIP(ctx, landing);
    return _URC_INSTALL_CONTEXT;
}

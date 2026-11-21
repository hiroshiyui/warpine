// SPDX-License-Identifier: GPL-3.0-or-later
/*
 * cxx_test.cc - C++ on Warpine's SDK (ADR-0103), built by g++ or clang++
 *
 * Checks the C++ runtime against the Itanium C++ ABI as both compilers
 * use it: static constructors and destructors, new and delete, virtual
 * functions, typeid and dynamic_cast, exceptions (by value, by reference
 * to a base, by pointer, catch (...), rethrow, destructors run while
 * unwinding, std::bad_alloc, std::bad_cast), thread-safe static locals,
 * exceptions on a second thread, and exceptions passing an OS/2 exception
 * handler registered with DosSetExceptionHandler, which is called with
 * EH_UNWINDING and leaves the chain with the frame it was registered in.
 * An access violation an OS/2 handler turns into a C++ throw unwinds the
 * frames that trapped, running their destructors (ADR-0110).
 *
 * With cxxdll.dll, which links its own copy of the runtime (ADR-0107):
 * exceptions thrown in the DLL and caught here, thrown here and caught in
 * the DLL, passing a DLL frame whose destructor runs, rethrown in the DLL,
 * and the uncaught counts of both runtimes afterwards.
 *
 *   cxx_test: <name>: ok
 *   cxx_test: <name>: FAIL
 *   cxx_test: done <passed>/<total>
 *
 * With the argument "uncaught" it throws with no handler: std::terminate
 * writes a message and the process ends with exit code 3.
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#include <os2.h>
#include <cstdio>
#include <cstring>
#include <new>
#include <typeinfo>
#include <stdexcept>
#include <utility>
#include "cxxdll.h"

static int n_checks, n_passed;

static void check(const char *what, bool ok)
{
    n_checks++;
    if (ok) n_passed++;
    std::printf("cxx_test: %s: %s\n", what, ok ? "ok" : "FAIL");
}

// ── Static construction and destruction ────────────────────────────────
static int constructed_before_main;

struct Global {
    int value;
    Global() : value(42) { constructed_before_main++; }
    ~Global() { std::printf("cxx_test: the global's destructor ran at exit\n"); }
};
static Global global;

// ── Classes ────────────────────────────────────────────────────────────
static int live_objects;

struct Shape {
    Shape() { live_objects++; }
    virtual ~Shape() { live_objects--; }
    virtual int sides() const = 0;
};

struct Triangle : Shape { int sides() const override { return 3; } };
struct Square : Shape { int sides() const override { return 4; } };

struct Named { virtual ~Named() {} virtual const char *name() const { return "named"; } };
struct NamedSquare : Square, Named { const char *name() const override { return "square"; } };

// ── Exceptions ─────────────────────────────────────────────────────────
struct Error : std::runtime_error {
    int code;
    Error(const char *what, int c) : std::runtime_error(what), code(c) {}
};

struct Guard {
    int *counter;
    explicit Guard(int *c) : counter(c) {}
    ~Guard() { (*counter)++; }
};

static int uncaught_in_destructor = -1;
struct Witness { ~Witness() { uncaught_in_destructor = std::uncaught_exceptions(); } };

__attribute__((noinline)) static void level3(int *cleanups) { Guard g(cleanups); Witness w; throw Error("deep", 7); }
__attribute__((noinline)) static void level2(int *cleanups) { Guard g(cleanups); level3(cleanups); }
__attribute__((noinline)) static void level1(int *cleanups) { Guard g(cleanups); level2(cleanups); }

struct ThrowsInConstructor {
    Guard member;
    explicit ThrowsInConstructor(int *c) : member(c) { throw 5; }
};

static int static_local_inits;
static int &counter_once()
{
    static int value = ++static_local_inits;
    return value;
}

// ── An OS/2 exception handler under a C++ throw ────────────────────────
static int os2_unwound;              // calls with EH_UNWINDING
static ULONG APIENTRY os2_handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD, PCONTEXTRECORD, PVOID)
{
    if (rep->fHandlerFlags & EH_UNWINDING) os2_unwound++;
    return XCPT_CONTINUE_SEARCH;
}

static ULONG chain_head()
{
    ULONG head;
    __asm__ volatile("movl %%fs:0, %0" : "=r"(head));
    return head;
}

// Not inlined: the registration must be in a frame the exception leaves.
__attribute__((noinline)) static void throw_past_os2_handler()
{
    EXCEPTIONREGISTRATIONRECORD rec;
    std::memset(&rec, 0, sizeof rec);
    rec.ExceptionHandler = os2_handler;
    DosSetExceptionHandler(&rec);
    throw std::logic_error("past an OS/2 handler");
}

// ── A trap made a C++ exception (ADR-0110) ─────────────────────────────
// An OS/2 handler throws on an access violation: the exception unwinds
// the frames of the code that trapped, running their destructors, to a
// catch above them, and the handler's own registration is unwound.
struct Trap {
    ULONG address;
};

static int trap_dtors;                // destructors run while unwinding
static int trap_unwound;              // the trap handler's EH_UNWINDING calls
struct TrapGuard {
    ~TrapGuard() { trap_dtors++; }
};

static ULONG APIENTRY trap_to_throw(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD, PCONTEXTRECORD, PVOID)
{
    if (rep->fHandlerFlags & (EH_UNWINDING | EH_EXIT_UNWIND)) {
        trap_unwound++;
        return XCPT_CONTINUE_SEARCH;
    }
    if (rep->ExceptionNum == XCPT_ACCESS_VIOLATION) throw Trap{rep->ExceptionInfo[1]};
    return XCPT_CONTINUE_SEARCH;
}

static int *volatile nowhere = nullptr;

__attribute__((noinline)) static void touch_nowhere()
{
    *nowhere = 1;                     // access violation at address 0
}

// Called through a pointer the compiler cannot see through: otherwise it
// proves the call cannot throw (a store throws nothing in C++) and leaves
// the caller without the cleanup that runs its destructor. Code compiled
// with GCC's -fnon-call-exceptions needs no such care.
static void (*volatile trap_here)() = touch_nowhere;

__attribute__((noinline)) static void guarded_inner()
{
    TrapGuard g;
    trap_here();
}

__attribute__((noinline)) static void guarded_outer()
{
    TrapGuard g;
    guarded_inner();
}

// Registers the handler, then traps two frames down.
__attribute__((noinline)) static void trap_under_handler()
{
    EXCEPTIONREGISTRATIONRECORD rec;
    std::memset(&rec, 0, sizeof rec);
    rec.ExceptionHandler = trap_to_throw;
    DosSetExceptionHandler(&rec);
    guarded_outer();
    DosUnsetExceptionHandler(&rec);   // never reached
}

// ── A second thread ────────────────────────────────────────────────────
static void throw_exe_error(void)
{
    throw ExeError{17};
}

static volatile int thread_caught;
static void APIENTRY thread_main(ULONG)
{
    try {
        throw Error("on thread 2", 2);
    } catch (const Error &e) {
        thread_caught = e.code == 2 && std::strcmp(e.what(), "on thread 2") == 0 ? 1 : -1;
    }
}

int main(int argc, char **argv)
{
    if (argc > 1 && std::strcmp(argv[1], "uncaught") == 0) {
        std::printf("cxx_test: throwing with no handler\n");
        throw 1;
    }

    check("the global was constructed before main", constructed_before_main == 1 && global.value == 42);

    Shape *shapes[2] = { new Triangle, new Square };
    check("virtual calls", shapes[0]->sides() == 3 && shapes[1]->sides() == 4);
    check("typeid of a polymorphic object", typeid(*shapes[1]) == typeid(Square) && typeid(*shapes[0]) != typeid(Square));
    check("dynamic_cast down", dynamic_cast<Square *>(shapes[1]) != nullptr && dynamic_cast<Square *>(shapes[0]) == nullptr);
    for (Shape *s : shapes) delete s;
    check("virtual destructors ran", live_objects == 0);

    NamedSquare ns;
    Shape *as_shape = &ns;
    Named *cross = dynamic_cast<Named *>(as_shape);
    check("dynamic_cast across bases", cross != nullptr && std::strcmp(cross->name(), "square") == 0);
    check("dynamic_cast<void *> finds the whole object", dynamic_cast<void *>(cross) == static_cast<void *>(&ns));
    try {
        Triangle &t = dynamic_cast<Triangle &>(*as_shape);
        (void)t;
        check("a failed reference cast throws std::bad_cast", false);
    } catch (const std::bad_cast &) {
        check("a failed reference cast throws std::bad_cast", true);
    }

    int *array = new int[100];
    for (int i = 0; i < 100; i++) array[i] = i;
    check("new[]", array[99] == 99);
    delete[] array;
    alignas(16) unsigned char buf[sizeof(Square)];
    Square *placed = new (buf) Square;
    check("placement new", placed->sides() == 4 && static_cast<void *>(placed) == buf);
    placed->~Square();

    try {
        throw 42;
    } catch (int v) {
        check("catch an int by value", v == 42);
    }

    int cleanups = 0;
    try {
        level1(&cleanups);
        check("an exception through three frames", false);
    } catch (const std::exception &e) {
        const Error *err = dynamic_cast<const Error *>(&e);
        check("an exception through three frames, caught as its base", err && err->code == 7);
        check("its message", std::strcmp(e.what(), "deep") == 0);
        check("every frame's destructor ran", cleanups == 3);
        check("uncaught_exceptions() during unwinding", uncaught_in_destructor == 1);
        check("uncaught_exceptions() in the handler", std::uncaught_exceptions() == 0);
    }

    try {
        try {
            throw Error("again", 9);
        } catch (Error &e) {
            e.code = 10;
            throw;
        }
    } catch (const Error &e) {
        check("rethrow keeps the same object", e.code == 10);
    }

    try {
        throw std::make_pair(1, 2).first;
    } catch (...) {
        check("catch (...)", true);
    }

    Square sq;
    try {
        throw &sq;
    } catch (Shape *s) {
        check("catch a pointer to a base", s == &sq && s->sides() == 4);
    }

    int member_cleanups = 0;
    try {
        ThrowsInConstructor t(&member_cleanups);
        (void)t;
    } catch (int v) {
        check("a constructor's members are destroyed when it throws", v == 5 && member_cleanups == 1);
    }

    try {
        void *huge = ::operator new(0x7FFFFFF0u);
        ::operator delete(huge);
        check("operator new throws std::bad_alloc", false);
    } catch (const std::bad_alloc &e) {
        check("operator new throws std::bad_alloc", std::strcmp(e.what(), "std::bad_alloc") == 0);
    }
    // Kept in a volatile pointer: an unused allocation may be left out.
    char *volatile nothrow_block = new (std::nothrow) char[0x7FFFFFF0u];
    check("new (std::nothrow) returns null", nothrow_block == nullptr);
    delete[] nothrow_block;

    check("a static local is initialised once", counter_once() == 1 && counter_once() == 1 && static_local_inits == 1);

    ULONG head_before = chain_head();
    try {
        throw_past_os2_handler();
    } catch (const std::logic_error &e) {
        check("an exception passes an OS/2 exception handler", std::strcmp(e.what(), "past an OS/2 handler") == 0);
    }
    check("... whose registration left the chain", chain_head() == head_before);
    check("... after its handler was called with EH_UNWINDING", os2_unwound == 1);

    for (int round = 1; round <= 3; round++) {
        ULONG trap_head = chain_head();
        trap_dtors = trap_unwound = 0;
        ULONG where = 1;
        try {
            trap_under_handler();
        } catch (const Trap &t) {
            where = t.address;
        }
        if (round == 1) {
            check("an access violation thrown as a C++ exception is caught", where == 0);
            check("... the trapping frames' destructors ran", trap_dtors == 2);
            check("... the handler was unwound and left the chain", trap_unwound == 1 && chain_head() == trap_head);
        } else if (round == 3) {
            check("... three times over", where == 0 && trap_dtors == 2 && chain_head() == trap_head);
        }
    }

    TID tid;
    check("DosCreateThread", DosCreateThread(&tid, thread_main, 0, 0, 0x10000) == NO_ERROR);
    DosWaitThread(&tid, DCWW_WAIT);
    check("an exception caught on a second thread", thread_caught == 1);

    // ── Exceptions across a DLL (ADR-0107) ──────────────────────────────
    try {
        dll_throw(5);
        check("an exception from a DLL, caught by its type", false);
    } catch (const DllError &e) {
        check("an exception from a DLL, caught by its type", e.code == 5 && std::strcmp(e.what(), "from the DLL") == 0);
    }
    try {
        dll_throw(6);
    } catch (const std::runtime_error &e) {
        check("... and by a standard base class", std::strcmp(e.what(), "from the DLL") == 0);
    }
    check("the DLL catches the program's exception", dll_catch(throw_exe_error) == 17);
    int dll_cleanups = 0;
    try {
        dll_pass(throw_exe_error, &dll_cleanups);
        check("an exception passes a DLL frame", false);
    } catch (const ExeError &e) {
        check("an exception passes a DLL frame", e.value == 17);
        check("... whose destructor ran", dll_cleanups == 1);
    }
    try {
        dll_rethrow(throw_exe_error);
    } catch (const ExeError &e) {
        check("rethrown in the DLL", e.value == 17);
    }
    int many = 0;
    for (int i = 0; i < 200; i++) {
        try { dll_throw(i); } catch (const DllError &e) { many += e.code == i; }
    }
    check("200 exceptions from the DLL, each freed by its runtime", many == 200);
    check("both runtimes count no uncaught exception", std::uncaught_exceptions() == 0 && dll_uncaught() == 0);

    std::printf("cxx_test: built by %s\n",
#ifdef __clang__
                "clang++"
#else
                "g++"
#endif
    );
    std::printf("cxx_test: done %d/%d\n", n_passed, n_checks);
    return n_passed == n_checks ? 0 : 1;
}

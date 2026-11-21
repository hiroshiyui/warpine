// SPDX-License-Identifier: GPL-3.0-or-later
// cxxdll.h - what cxx_test and cxxdll.dll share (ADR-0107): the exception
// types each throws and the DLL's calls.
#ifndef CXXDLL_H
#define CXXDLL_H
#include <stdexcept>

// Thrown by the DLL, caught by the program.
struct DllError : std::runtime_error {
    int code;
    DllError(const char *what, int c) : std::runtime_error(what), code(c) {}
};

// Thrown by the program, caught (or passed on) by the DLL.
struct ExeError {
    int value;
};

extern "C" {
// Throws DllError("from the DLL", code).
int dll_throw(int code);
// Calls fn; returns the ExeError's value it caught, -1 for another
// std::exception, 0 when nothing was thrown.
int dll_catch(void (*fn)(void));
// Calls fn with a local object whose destructor adds 1 to *cleanups; an
// exception passes through.
int dll_pass(void (*fn)(void), int *cleanups);
// Calls fn, catches what it throws and rethrows it with `throw;`.
int dll_rethrow(void (*fn)(void));
// std::uncaught_exceptions() as the DLL's runtime counts it.
int dll_uncaught(void);
}
#endif

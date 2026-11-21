// SPDX-License-Identifier: GPL-3.0-or-later
/*
 * cxxdll.cc - a C++ DLL for cxx_test (ADR-0107)
 *
 * It links its own copy of the SDK's C++ runtime. Exceptions pass between
 * it and the program both ways: thrown here and caught there, thrown
 * there and caught here, passing through a frame here whose destructor
 * runs, and rethrown here.
 */
#include <exception>
#include "cxxdll.h"

namespace {
struct Cleanup {
    int *count;
    ~Cleanup() { ++*count; }
};
}

extern "C" int dll_throw(int code)
{
    throw DllError("from the DLL", code);
}

extern "C" int dll_catch(void (*fn)(void))
{
    try {
        fn();
    } catch (const ExeError &e) {
        return e.value;
    } catch (const std::exception &) {
        return -1;
    }
    return 0;
}

extern "C" int dll_pass(void (*fn)(void), int *cleanups)
{
    Cleanup c{cleanups};
    fn();
    return 0;
}

extern "C" int dll_rethrow(void (*fn)(void))
{
    try {
        fn();
    } catch (...) {
        throw;
    }
    return 0;
}

extern "C" int dll_uncaught(void)
{
    return std::uncaught_exceptions();
}

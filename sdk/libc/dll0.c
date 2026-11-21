/* SPDX-License-Identifier: GPL-3.0-or-later */
/* dll0.c - start-up for DLLs built with Warpine's SDK (ADR-0101); link it
 * first, in place of crt0.o.
 *
 * OS/2 calls a DLL's initialization and termination routine as a _System
 * (cdecl) call with the module handle and a flag: 0 when a process attaches
 * (initialization), 1 when it detaches (termination). It returns 1 on
 * success and 0 on failure, which makes the load fail.
 *
 * On initialization this one sets up the DLL's own copy of the runtime
 * (errno), runs its preinit and init arrays (constructors), then
 * __dll_initialize if the DLL defines it, the hook other OS/2 compilers'
 * runtimes call; a zero result fails the load. On termination it calls
 * __dll_terminate if defined, the destructors of the DLL's C++ statics
 * (__cxa_atexit with the DLL's __dso_handle, ADR-0103), then the fini
 * arrays in reverse. A DLL can define _DLL_InitTerm itself instead. */
#include <os2.h>

#include "libc_internal.h"

typedef void (*init_fn)(void);
extern init_fn __preinit_array_start[], __preinit_array_end[];
extern init_fn __init_array_start[], __init_array_end[];
extern init_fn __fini_array_start[], __fini_array_end[];

/* The DLL's handle for __cxa_atexit. */
void *__dso_handle = &__dso_handle;

int __dll_initialize(void) __attribute__((weak));
int __dll_terminate(void) __attribute__((weak));

#define DLL_INIT 0
#define DLL_TERM 1

__attribute__((weak, force_align_arg_pointer, used))
ULONG APIENTRY _DLL_InitTerm(ULONG hmod, ULONG flag)
{
    (void)hmod;
    if (flag == DLL_INIT) {
        __libc_init_errno();
        for (init_fn *f = __preinit_array_start; f < __preinit_array_end; f++) (*f)();
        for (init_fn *f = __init_array_start; f < __init_array_end; f++) (*f)();
        if (__dll_initialize && !__dll_initialize()) return 0;
        return 1;
    }
    if (flag == DLL_TERM) {
        if (__dll_terminate) __dll_terminate();
        __cxa_finalize(&__dso_handle);
        for (init_fn *f = __fini_array_end; f > __fini_array_start; ) (*--f)();
        return 1;
    }
    return 0;
}

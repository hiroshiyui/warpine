/* SPDX-License-Identifier: GPL-3.0-or-later */
/* unwind.h - the Itanium C++ ABI's base unwinding interface (level I) for
 * Warpine's SDK (ADR-0103), as GCC and clang call it on i386: two-phase
 * unwinding over the DWARF call-frame information in .eh_frame. */
#ifndef WARPINE_UNWIND_H
#define WARPINE_UNWIND_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    _URC_NO_REASON = 0,
    _URC_FOREIGN_EXCEPTION_CAUGHT = 1,
    _URC_FATAL_PHASE2_ERROR = 2,
    _URC_FATAL_PHASE1_ERROR = 3,
    _URC_NORMAL_STOP = 4,
    _URC_END_OF_STACK = 5,
    _URC_HANDLER_FOUND = 6,
    _URC_INSTALL_CONTEXT = 7,
    _URC_CONTINUE_UNWIND = 8
} _Unwind_Reason_Code;

typedef int _Unwind_Action;
#define _UA_SEARCH_PHASE  1
#define _UA_CLEANUP_PHASE 2
#define _UA_HANDLER_FRAME 4
#define _UA_FORCE_UNWIND  8
#define _UA_END_OF_STACK  16

typedef uint64_t _Unwind_Exception_Class;
typedef uintptr_t _Unwind_Word;
typedef intptr_t _Unwind_Sword;
typedef uintptr_t _Unwind_Ptr;

struct _Unwind_Exception;
typedef void (*_Unwind_Exception_Cleanup_Fn)(_Unwind_Reason_Code, struct _Unwind_Exception *);

struct _Unwind_Exception {
    _Unwind_Exception_Class exception_class;
    _Unwind_Exception_Cleanup_Fn exception_cleanup;
    _Unwind_Word private_1;        /* 0 (no forced unwinding) */
    _Unwind_Word private_2;        /* the handler frame's CFA, phase 1 -> 2 */
};

struct _Unwind_Context;

typedef _Unwind_Reason_Code (*_Unwind_Personality_Fn)(int version, _Unwind_Action actions,
                                                      _Unwind_Exception_Class exception_class,
                                                      struct _Unwind_Exception *exception,
                                                      struct _Unwind_Context *context);
typedef _Unwind_Reason_Code (*_Unwind_Trace_Fn)(struct _Unwind_Context *context, void *arg);

_Unwind_Reason_Code _Unwind_RaiseException(struct _Unwind_Exception *exception);
_Unwind_Reason_Code _Unwind_Resume_or_Rethrow(struct _Unwind_Exception *exception);
void _Unwind_Resume(struct _Unwind_Exception *exception) __attribute__((__noreturn__));
void _Unwind_DeleteException(struct _Unwind_Exception *exception);
_Unwind_Reason_Code _Unwind_Backtrace(_Unwind_Trace_Fn trace, void *arg);

_Unwind_Word _Unwind_GetGR(struct _Unwind_Context *context, int index);
void _Unwind_SetGR(struct _Unwind_Context *context, int index, _Unwind_Word value);
_Unwind_Ptr _Unwind_GetIP(struct _Unwind_Context *context);
_Unwind_Ptr _Unwind_GetIPInfo(struct _Unwind_Context *context, int *ip_before_insn);
void _Unwind_SetIP(struct _Unwind_Context *context, _Unwind_Ptr value);
_Unwind_Word _Unwind_GetCFA(struct _Unwind_Context *context);
_Unwind_Ptr _Unwind_GetRegionStart(struct _Unwind_Context *context);
_Unwind_Ptr _Unwind_GetLanguageSpecificData(struct _Unwind_Context *context);

#ifdef __cplusplus
}
#endif

#endif

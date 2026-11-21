/* SPDX-License-Identifier: GPL-3.0-or-later */
/* libc_internal.h - what the runtime's own files share. */
#ifndef WARPINE_LIBC_INTERNAL_H
#define WARPINE_LIBC_INTERNAL_H

#include <stddef.h>

/* Give errno and the C++ runtime their thread-local words (crt0 and
 * dll0, before anything can fail). */
void __libc_init_errno(void);

/* The thread's two words for the C++ runtime (ADR-0103). */
void *__libc_cxx_globals(void);

/* Run the exit functions registered for a module (NULL: all). */
void __cxa_finalize(void *dso);

/* The environment block crt0 was given. */
extern char *__libc_env;

/* Flush and close every open stream (exit). */
void __libc_close_streams(void);

/* An OS/2 error code as an errno value. */
int __libc_errno_of(unsigned long rc);

/* A process-wide lock for the runtime's own state. */
typedef struct { volatile int held; } libc_lock;
void __libc_lock(libc_lock *l);
void __libc_unlock(libc_lock *l);

/* The exact decimal digits of a finite, non-negative double (dtoa.c):
 * the integer part's digits, then fraction digits until there are at
 * least `need_significant` significant ones and `need_fraction` fraction
 * ones, or the fraction ends. Returns the digit count; *nint is how many
 * belong to the integer part, *sticky whether a non-zero fraction is
 * left over. */
int __libc_dtoa(double v, char *out, int out_max, int need_significant, int need_fraction,
                int *nint, int *sticky);

/* The core of printf: write the formatted text through `put`. */
typedef void (*libc_sink)(void *ctx, const char *s, size_t n);
int __libc_format(libc_sink put, void *ctx, const char *fmt, __builtin_va_list ap);

#endif

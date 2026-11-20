/* SPDX-License-Identifier: GPL-3.0-or-later */
/* libc_internal.h - what the runtime's own files share. */
#ifndef WARPINE_LIBC_INTERNAL_H
#define WARPINE_LIBC_INTERNAL_H

#include <stddef.h>

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

/* The core of printf: write the formatted text through `put`. */
typedef void (*libc_sink)(void *ctx, const char *s, size_t n);
int __libc_format(libc_sink put, void *ctx, const char *fmt, __builtin_va_list ap);

#endif

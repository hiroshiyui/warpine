/* SPDX-License-Identifier: GPL-3.0-or-later */
/* assert.h - Warpine's small C runtime (ADR-0096). */
#undef assert
#ifdef NDEBUG
#  define assert(e) ((void)0)
#else
void __assert_fail(const char *expr, const char *file, int line) __attribute__((__noreturn__));
#  define assert(e) ((e) ? (void)0 : __assert_fail(#e, __FILE__, __LINE__))
#endif

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* stdlib.c - Warpine's small C runtime (ADR-0096): process exit, the
 * environment, number conversions, sorting, rand and the runtime's lock. */
#define INCL_DOSPROCESS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>

#include "libc_internal.h"

char *__libc_env;

/* ── Thread-local memory: errno ─────────────────────────────────────── */
#define TLS_ERRNO      0
#define TLS_WORDS      1
static ULONG *tls_words;
static int errno_before_init;

void __libc_init_errno(void)
{
    PULONG words;
    if (DosAllocThreadLocalMemory(TLS_WORDS, &words) == NO_ERROR) tls_words = words;
}

int *__errno_location(void)
{
    return tls_words ? (int *)&tls_words[TLS_ERRNO] : &errno_before_init;
}

/* The C++ runtime's two words per thread (ADR-0107): in Warpine's runtime
 * block, which every module of the process shares, so an exception thrown
 * in one module and caught in another is counted once. Its address is the
 * same on every thread. */
static ULONG *runtime_block;
static ULONG cxx_fallback[2];

void *__libc_cxx_globals(void)
{
    if (!runtime_block) {
        PULONG block;
        if (WrpQueryRuntimeBlock(&block) != NO_ERROR) return cxx_fallback;
        runtime_block = block;
    }
    return &runtime_block[WRP_RUNTIME_CXX_WORD];
}

/* ── The runtime's lock ─────────────────────────────────────────────── */
void __libc_lock(libc_lock *l)
{
    while (__atomic_exchange_n(&l->held, 1, __ATOMIC_ACQUIRE))
        DosSleep(0);
}

void __libc_unlock(libc_lock *l)
{
    __atomic_store_n(&l->held, 0, __ATOMIC_RELEASE);
}

int __libc_errno_of(unsigned long rc)
{
    switch (rc) {
    case NO_ERROR:                  return 0;
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
    case ERROR_INVALID_DRIVE:       return ENOENT;
    case ERROR_TOO_MANY_OPEN_FILES: return EMFILE;
    case ERROR_ACCESS_DENIED:
    case ERROR_SHARING_VIOLATION:
    case ERROR_LOCK_VIOLATION:      return EACCES;
    case ERROR_INVALID_HANDLE:      return EBADF;
    case ERROR_NOT_ENOUGH_MEMORY:   return ENOMEM;
    case ERROR_FILE_EXISTS:
    case ERROR_ALREADY_EXISTS:      return EEXIST;
    case ERROR_DISK_FULL:           return ENOSPC;
    case ERROR_BROKEN_PIPE:         return EPIPE;
    case ERROR_SEEK_ON_DEVICE:      return ESPIPE;
    case ERROR_INVALID_PARAMETER:
    case ERROR_INVALID_NAME:        return EINVAL;
    default:                        return EIO;
    }
}

/* ── Exit ───────────────────────────────────────────────────────────── */
/* atexit functions and C++ destructors registered by __cxa_atexit, run in
 * reverse order of registration: by exit() all of them, by a DLL's
 * termination (__cxa_finalize with its __dso_handle) the DLL's own. */
#define ATEXIT_MAX 128
struct exit_fn {
    void (*fn)(void *);
    void *arg;
    void *dso;
    int   plain;          /* an atexit function: called with no argument */
};
static struct exit_fn exit_fns[ATEXIT_MAX];
static int exit_count;
static libc_lock exit_lock;

static int add_exit_fn(void (*fn)(void *), void *arg, void *dso, int plain)
{
    int rc = -1;
    __libc_lock(&exit_lock);
    if (exit_count < ATEXIT_MAX) {
        exit_fns[exit_count++] = (struct exit_fn){ fn, arg, dso, plain };
        rc = 0;
    }
    __libc_unlock(&exit_lock);
    return rc;
}

int atexit(void (*fn)(void))
{
    return add_exit_fn((void (*)(void *))fn, NULL, NULL, 1);
}

int __cxa_atexit(void (*fn)(void *), void *arg, void *dso)
{
    return add_exit_fn(fn, arg, dso, 0);
}

/* Run (newest first) the functions registered for `dso`, or all of them
 * for NULL; each runs once, and may register more. */
void __cxa_finalize(void *dso)
{
    for (;;) {
        struct exit_fn e = { 0, 0, 0, 0 };
        __libc_lock(&exit_lock);
        for (int i = exit_count - 1; i >= 0; i--) {
            if (exit_fns[i].fn && (!dso || exit_fns[i].dso == dso)) {
                e = exit_fns[i];
                exit_fns[i].fn = NULL;
                break;
            }
        }
        while (exit_count > 0 && !exit_fns[exit_count - 1].fn) exit_count--;
        __libc_unlock(&exit_lock);
        if (!e.fn) return;
        if (e.plain) ((void (*)(void))e.fn)(); else e.fn(e.arg);
    }
}

typedef void (*fini_fn)(void);
extern fini_fn __fini_array_start[], __fini_array_end[];

void exit(int status)
{
    __cxa_finalize(NULL);
    for (fini_fn *f = __fini_array_end; f > __fini_array_start; ) (*--f)();
    __libc_close_streams();
    DosExit(EXIT_PROCESS, (ULONG)status);
}

void _Exit(int status)
{
    DosExit(EXIT_PROCESS, (ULONG)status);
}

/* The exit code OS/2 C runtimes use for abort(). */
#define ABORT_EXIT_CODE 3

void abort(void)
{
    static const char msg[] = "abnormal program termination\r\n";
    ULONG written;
    DosWrite(HF_STDERR, msg, sizeof msg - 1, &written);
    DosExit(EXIT_PROCESS, ABORT_EXIT_CODE);
}

void __assert_fail(const char *expr, const char *file, int line)
{
    fprintf(stderr, "Assertion failed: %s, file %s, line %d\n", expr, file, line);
    abort();
}

/* ── The environment ────────────────────────────────────────────────── */
char *getenv(const char *name)
{
    size_t n = strlen(name);
    for (char *p = __libc_env; p && *p; p += strlen(p) + 1)
        if (!strnicmp(p, name, n) && p[n] == '=') return p + n + 1;
    return NULL;
}

/* ── Numbers ────────────────────────────────────────────────────────── */
int abs(int n)               { return n < 0 ? -n : n; }
long labs(long n)            { return n < 0 ? -n : n; }
long long llabs(long long n) { return n < 0 ? -n : n; }

div_t div(int num, int den)
{
    div_t r = { num / den, num % den };
    return r;
}

ldiv_t ldiv(long num, long den)
{
    ldiv_t r = { num / den, num % den };
    return r;
}

static int digit_value(int c)
{
    if (isdigit(c)) return c - '0';
    if (isalpha(c)) return tolower(c) - 'a' + 10;
    return 99;
}

/* The common part of the strto* functions: the magnitude, clamped to
 * `limit`, the sign, and where the number ended. */
static unsigned long long parse_number(const char *s, char **end, int base,
                                       unsigned long long limit, int *negative)
{
    const char *p = s;
    while (isspace((unsigned char)*p)) p++;
    *negative = 0;
    if (*p == '+' || *p == '-') *negative = (*p++ == '-');
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X')
        && digit_value((unsigned char)p[2]) < 16) {
        p += 2;
        base = 16;
    } else if (base == 0) {
        base = (*p == '0') ? 8 : 10;
    }
    unsigned long long value = 0;
    int any = 0, overflow = 0;
    for (;; p++) {
        int d = digit_value((unsigned char)*p);
        if (d >= base) break;
        any = 1;
        if (value > (limit - (unsigned)d) / (unsigned)base) overflow = 1;
        else value = value * (unsigned)base + (unsigned)d;
    }
    if (end) *end = (char *)(any ? p : s);
    if (overflow) { errno = ERANGE; return limit; }
    return value;
}

long strtol(const char *s, char **end, int base)
{
    int neg;
    unsigned long long v = parse_number(s, end, base, (unsigned long long)LONG_MAX + 1, &neg);
    if (!neg && v > LONG_MAX) { errno = ERANGE; return LONG_MAX; }
    return neg ? (long)(0 - v) : (long)v;
}

unsigned long strtoul(const char *s, char **end, int base)
{
    int neg;
    unsigned long long v = parse_number(s, end, base, ULONG_MAX, &neg);
    return neg ? (unsigned long)(0 - v) : (unsigned long)v;
}

long long strtoll(const char *s, char **end, int base)
{
    int neg;
    unsigned long long v = parse_number(s, end, base, (unsigned long long)LLONG_MAX + 1, &neg);
    if (!neg && v > LLONG_MAX) { errno = ERANGE; return LLONG_MAX; }
    return neg ? (long long)(0 - v) : (long long)v;
}

unsigned long long strtoull(const char *s, char **end, int base)
{
    int neg;
    unsigned long long v = parse_number(s, end, base, ULLONG_MAX, &neg);
    return neg ? 0 - v : v;
}

int atoi(const char *s)  { return (int)strtol(s, NULL, 10); }
long atol(const char *s) { return strtol(s, NULL, 10); }

/* ── Sorting and searching ──────────────────────────────────────────── */
static void swap_bytes(unsigned char *a, unsigned char *b, size_t n)
{
    while (n--) { unsigned char t = *a; *a++ = *b; *b++ = t; }
}

/* Shell sort: no recursion and no extra memory. */
void qsort(void *base, size_t count, size_t size, int (*cmp)(const void *, const void *))
{
    unsigned char *b = base;
    size_t gap = 1;
    while (gap < count / 3) gap = gap * 3 + 1;
    for (; gap > 0; gap /= 3)
        for (size_t i = gap; i < count; i++)
            for (size_t j = i; j >= gap && cmp(b + (j - gap) * size, b + j * size) > 0; j -= gap)
                swap_bytes(b + (j - gap) * size, b + j * size, size);
}

void *bsearch(const void *key, const void *base, size_t count, size_t size,
              int (*cmp)(const void *, const void *))
{
    const unsigned char *b = base;
    while (count > 0) {
        const unsigned char *mid = b + (count / 2) * size;
        int c = cmp(key, mid);
        if (c == 0) return (void *)mid;
        if (c > 0) { b = mid + size; count -= count / 2 + 1; }
        else count /= 2;
    }
    return NULL;
}

/* ── rand (the C standard's example generator) ──────────────────────── */
static unsigned long rand_next = 1;

int rand(void)
{
    rand_next = rand_next * 1103515245UL + 12345UL;
    return (int)((rand_next / 65536UL) % (RAND_MAX + 1UL));
}

void srand(unsigned seed)
{
    rand_next = seed;
}

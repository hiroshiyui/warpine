/* SPDX-License-Identifier: GPL-3.0-or-later */
/* stdlib.h - Warpine's small C runtime (ADR-0096). */
#ifndef WARPINE_STDLIB_H
#define WARPINE_STDLIB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
#define RAND_MAX     0x7fff

typedef struct { int quot; int rem; } div_t;
typedef struct { long quot; long rem; } ldiv_t;

void  *malloc(size_t size);
void  *calloc(size_t count, size_t size);
void  *realloc(void *ptr, size_t size);
void   free(void *ptr);

void   exit(int status) __attribute__((__noreturn__));
void   _Exit(int status) __attribute__((__noreturn__));
void   abort(void) __attribute__((__noreturn__));
int    atexit(void (*fn)(void));
/* For C++ compilers: destructors of statics, run at exit (ADR-0103). */
int    __cxa_atexit(void (*fn)(void *), void *arg, void *dso);
char  *getenv(const char *name);

int    abs(int n);
long   labs(long n);
long long llabs(long long n);
div_t  div(int num, int den);
ldiv_t ldiv(long num, long den);
double atof(const char *s);
double strtod(const char *s, char **end);
int    atoi(const char *s);
long   atol(const char *s);
long   strtol(const char *s, char **end, int base);
unsigned long strtoul(const char *s, char **end, int base);
long long strtoll(const char *s, char **end, int base);
unsigned long long strtoull(const char *s, char **end, int base);

int    rand(void);
void   srand(unsigned seed);
void   qsort(void *base, size_t count, size_t size, int (*cmp)(const void *, const void *));
void  *bsearch(const void *key, const void *base, size_t count, size_t size,
               int (*cmp)(const void *, const void *));

#ifdef __cplusplus
}
#endif

#endif

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* printf.c - Warpine's small C runtime (ADR-0096): the formatter behind
 * every printf. Conversions: d i u o x X c s p %, with the flags
 * - + space # 0, a width and a precision (or *), and the lengths hh h l
 * ll z t j. Floating point is not formatted: %f and the like print as
 * written. */
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include <stddef.h>

#include "libc_internal.h"

#define FL_LEFT   0x01
#define FL_PLUS   0x02
#define FL_SPACE  0x04
#define FL_ALT    0x08
#define FL_ZERO   0x10

typedef struct {
    libc_sink put;
    void     *ctx;
    int       count;
} out;

static void emit(out *o, const char *s, size_t n)
{
    if (n) o->put(o->ctx, s, n);
    o->count += (int)n;
}

static void pad(out *o, char c, int n)
{
    char block[16];
    memset(block, c, sizeof block);
    while (n > 0) {
        int take = n < (int)sizeof block ? n : (int)sizeof block;
        emit(o, block, (size_t)take);
        n -= take;
    }
}

/* One integer: digits from `value`, then sign / prefix, padding and
 * precision as printf defines them. */
static void format_int(out *o, unsigned long long value, int negative, unsigned base, int upper,
                       int flags, int width, int prec)
{
    char digits[24];
    int n = 0;
    const char *set = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    while (value) { digits[n++] = set[value % base]; value /= base; }
    if (prec < 0) prec = 1;
    else flags &= ~FL_ZERO;

    char prefix[3];
    int plen = 0;
    if (negative) prefix[plen++] = '-';
    else if (flags & FL_PLUS) prefix[plen++] = '+';
    else if (flags & FL_SPACE) prefix[plen++] = ' ';
    if ((flags & FL_ALT) && base == 16 && n) { prefix[plen++] = '0'; prefix[plen++] = upper ? 'X' : 'x'; }
    if ((flags & FL_ALT) && base == 8 && prec <= n) prec = n + 1;

    int zeros = prec > n ? prec - n : 0;
    int body = plen + zeros + n;
    if (!(flags & FL_LEFT) && !(flags & FL_ZERO)) pad(o, ' ', width - body);
    emit(o, prefix, (size_t)plen);
    if (!(flags & FL_LEFT) && (flags & FL_ZERO)) pad(o, '0', width - body);
    pad(o, '0', zeros);
    while (n) emit(o, &digits[--n], 1);
    if (flags & FL_LEFT) pad(o, ' ', width - body);
}

int __libc_format(libc_sink put, void *ctx, const char *fmt, __builtin_va_list ap)
{
    out o = { put, ctx, 0 };
    while (*fmt) {
        const char *start = fmt;
        while (*fmt && *fmt != '%') fmt++;
        emit(&o, start, (size_t)(fmt - start));
        if (!*fmt) break;
        const char *spec = fmt++;

        int flags = 0;
        for (;; fmt++) {
            if (*fmt == '-') flags |= FL_LEFT;
            else if (*fmt == '+') flags |= FL_PLUS;
            else if (*fmt == ' ') flags |= FL_SPACE;
            else if (*fmt == '#') flags |= FL_ALT;
            else if (*fmt == '0') flags |= FL_ZERO;
            else break;
        }
        int width = 0;
        if (*fmt == '*') {
            width = __builtin_va_arg(ap, int);
            if (width < 0) { flags |= FL_LEFT; width = -width; }
            fmt++;
        } else {
            while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        }
        int prec = -1;
        if (*fmt == '.') {
            fmt++;
            prec = 0;
            if (*fmt == '*') {
                prec = __builtin_va_arg(ap, int);
                fmt++;
            } else {
                while (*fmt >= '0' && *fmt <= '9') prec = prec * 10 + (*fmt++ - '0');
            }
        }
        int size = 0;   /* -2 hh, -1 h, 0 int, 1 long, 2 long long */
        for (;; fmt++) {
            if (*fmt == 'h') size = size == -1 ? -2 : -1;
            else if (*fmt == 'l') size = size == 1 ? 2 : 1;
            else if (*fmt == 'z' || *fmt == 't' || *fmt == 'j') size = (*fmt == 'j') ? 2 : 1;
            else break;
        }

        char conv = *fmt;
        if (!conv) { emit(&o, spec, (size_t)(fmt - spec)); break; }
        fmt++;
        switch (conv) {
        case 'd': case 'i': {
            long long v = size == 2 ? __builtin_va_arg(ap, long long)
                        : size == 1 ? __builtin_va_arg(ap, long)
                        : __builtin_va_arg(ap, int);
            if (size == -1) v = (short)v;
            if (size == -2) v = (signed char)v;
            unsigned long long mag = v < 0 ? 0 - (unsigned long long)v : (unsigned long long)v;
            format_int(&o, mag, v < 0, 10, 0, flags, width, prec);
            break;
        }
        case 'u': case 'o': case 'x': case 'X': {
            unsigned long long v = size == 2 ? __builtin_va_arg(ap, unsigned long long)
                                 : size == 1 ? __builtin_va_arg(ap, unsigned long)
                                 : __builtin_va_arg(ap, unsigned);
            if (size == -1) v = (unsigned short)v;
            if (size == -2) v = (unsigned char)v;
            unsigned base = conv == 'u' ? 10 : conv == 'o' ? 8 : 16;
            format_int(&o, v, 0, base, conv == 'X', flags & ~(FL_PLUS | FL_SPACE), width, prec);
            break;
        }
        case 'p': {
            uintptr_t v = (uintptr_t)__builtin_va_arg(ap, void *);
            format_int(&o, v, 0, 16, 0, FL_ALT | (flags & FL_LEFT), width, 8);
            break;
        }
        case 'c': {
            char c = (char)__builtin_va_arg(ap, int);
            if (!(flags & FL_LEFT)) pad(&o, ' ', width - 1);
            emit(&o, &c, 1);
            if (flags & FL_LEFT) pad(&o, ' ', width - 1);
            break;
        }
        case 's': {
            const char *s = __builtin_va_arg(ap, const char *);
            if (!s) s = "(null)";
            int n = (int)(prec >= 0 ? strnlen(s, (size_t)prec) : strlen(s));
            if (!(flags & FL_LEFT)) pad(&o, ' ', width - n);
            emit(&o, s, (size_t)n);
            if (flags & FL_LEFT) pad(&o, ' ', width - n);
            break;
        }
        case '%':
            emit(&o, "%", 1);
            break;
        default:
            emit(&o, spec, (size_t)(fmt - spec));
            break;
        }
    }
    return o.count;
}

/* ── printf into memory ─────────────────────────────────────────────── */
typedef struct {
    char  *buf;
    size_t size, len;
} mem_sink;

static void mem_put(void *ctx, const char *s, size_t n)
{
    mem_sink *m = ctx;
    for (size_t i = 0; i < n; i++, m->len++)
        if (m->size && m->len < m->size - 1) m->buf[m->len] = s[i];
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap)
{
    mem_sink m = { buf, size, 0 };
    int n = __libc_format(mem_put, &m, fmt, ap);
    if (size) buf[m.len < size ? m.len : size - 1] = '\0';
    return n;
}

int vsprintf(char *buf, const char *fmt, va_list ap)
{
    return vsnprintf(buf, SIZE_MAX, fmt, ap);
}

int snprintf(char *buf, size_t size, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}

int sprintf(char *buf, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, SIZE_MAX, fmt, ap);
    va_end(ap);
    return n;
}

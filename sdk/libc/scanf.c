/* SPDX-License-Identifier: GPL-3.0-or-later */
/* scanf.c - Warpine's small C runtime (ADR-0101): sscanf / vsscanf.
 * Conversions d i u o x X s c n % and e f g (by strtod), with widths,
 * `*` and the length modifiers hh h l ll; whitespace in the format skips
 * any whitespace, other characters must match. No %[ ] sets. */
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <ctype.h>
#include <string.h>

enum { LEN_INT, LEN_CHAR, LEN_SHORT, LEN_LONG, LEN_LLONG };

static void store_int(va_list *ap, int len, unsigned long long v)
{
    switch (len) {
    case LEN_CHAR:  *va_arg(*ap, signed char *) = (signed char)v; break;
    case LEN_SHORT: *va_arg(*ap, short *) = (short)v; break;
    case LEN_LONG:  *va_arg(*ap, long *) = (long)v; break;
    case LEN_LLONG: *va_arg(*ap, long long *) = (long long)v; break;
    default:        *va_arg(*ap, int *) = (int)v; break;
    }
}

int vsscanf(const char *str, const char *fmt, va_list args)
{
    va_list ap;
    va_copy(ap, args);
    const char *s = str;
    int assigned = 0, converted_any = 0;
    char tmp[128];

    for (; *fmt; fmt++) {
        if (isspace((unsigned char)*fmt)) {
            while (isspace((unsigned char)*s)) s++;
            continue;
        }
        if (*fmt != '%' || fmt[1] == '%') {
            if (*fmt == '%') fmt++;
            if (*s != *fmt) break;
            s++;
            continue;
        }
        fmt++;
        int suppress = 0, width = 0, len = LEN_INT;
        if (*fmt == '*') { suppress = 1; fmt++; }
        while (isdigit((unsigned char)*fmt)) width = width * 10 + (*fmt++ - '0');
        if (*fmt == 'h') { len = LEN_SHORT; if (*++fmt == 'h') { len = LEN_CHAR; fmt++; } }
        else if (*fmt == 'l') { len = LEN_LONG; if (*++fmt == 'l') { len = LEN_LLONG; fmt++; } }
        char conv = *fmt;
        if (!conv) break;

        if (conv == 'n') {
            if (!suppress) store_int(&ap, len, (unsigned long long)(s - str));
            continue;
        }
        if (conv != 'c')
            while (isspace((unsigned char)*s)) s++;
        if (!*s) { if (!converted_any) assigned = EOF; break; }

        if (conv == 'c') {
            int n = width ? width : 1;
            if ((int)strlen(s) < n) break;
            if (!suppress) { memcpy(va_arg(ap, char *), s, (size_t)n); assigned++; }
            s += n;
            converted_any = 1;
            continue;
        }
        if (conv == 's') {
            char *out = suppress ? NULL : va_arg(ap, char *);
            int n = 0;
            while (*s && !isspace((unsigned char)*s) && (!width || n < width)) {
                if (out) out[n] = *s;
                n++, s++;
            }
            if (out) { out[n] = '\0'; assigned++; }
            converted_any = 1;
            continue;
        }

        /* Numbers: copy at most `width` characters, then convert. */
        size_t limit = width ? (size_t)width : sizeof tmp - 1;
        if (limit > sizeof tmp - 1) limit = sizeof tmp - 1;
        memcpy(tmp, s, limit);
        tmp[limit] = '\0';
        size_t avail = strlen(s);
        if (avail < limit) tmp[avail] = '\0';
        char *end;
        if (conv == 'e' || conv == 'f' || conv == 'g' || conv == 'E' || conv == 'G') {
            double d = strtod(tmp, &end);
            if (end == tmp) break;
            if (!suppress) {
                if (len == LEN_LONG) *va_arg(ap, double *) = d;
                else *va_arg(ap, float *) = (float)d;
                assigned++;
            }
        } else {
            int base;
            switch (conv) {
            case 'd': case 'u': base = 10; break;
            case 'i': base = 0; break;
            case 'o': base = 8; break;
            case 'x': case 'X': base = 16; break;
            default: va_end(ap); return assigned;
            }
            unsigned long long v;
            if (conv == 'u' || conv == 'o' || conv == 'x' || conv == 'X')
                v = strtoull(tmp, &end, base);
            else
                v = (unsigned long long)strtoll(tmp, &end, base);
            if (end == tmp) break;
            if (!suppress) { store_int(&ap, len, v); assigned++; }
        }
        s += end - tmp;
        converted_any = 1;
    }
    va_end(ap);
    return assigned;
}

int sscanf(const char *str, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vsscanf(str, fmt, ap);
    va_end(ap);
    return n;
}

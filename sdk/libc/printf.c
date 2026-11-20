/* SPDX-License-Identifier: GPL-3.0-or-later */
/* printf.c - Warpine's small C runtime (ADR-0096): the formatter behind
 * every printf. Conversions: d i u o x X c s p %, with the flags
 * - + space # 0, a width and a precision (or *), and the lengths hh h l
 * ll z t j; and e E f F g G for double (L: long double, rounded to a
 * double first), from exact digits (dtoa.c) rounded once, ties to even. */
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

/* Round the digit string d[0..n) to its first `keep` digits (ties to
 * even, `sticky`: non-zero digits beyond d[n]). Returns 1 when the
 * rounding carried out of the first digit (d now starts with a new '1'
 * and has keep + 1 digits). */
static int round_digits(char *d, int n, int keep, int sticky)
{
    if (keep < 0) return 0;
    int next = keep < n ? d[keep] - '0' : 0;
    for (int i = keep + 1; i < n; i++) sticky |= d[i] != '0';
    int odd = keep > 0 && ((d[keep - 1] - '0') & 1);
    int up = next > 5 || (next == 5 && (sticky || odd));
    for (int i = n; i < keep; i++) d[i] = '0';
    if (!up) return 0;
    for (int i = keep - 1; i >= 0; i--) {
        if (d[i] != '9') { d[i]++; return 0; }
        d[i] = '0';
    }
    memmove(d + 1, d, (size_t)keep);
    d[0] = '1';
    return 1;
}

#define DIGITS_MAX 1200
/* The largest precision honoured: with 309 integer digits, or 323 leading
 * zeros, the digits still fit DIGITS_MAX. */
#define PREC_MAX   800

/* One floating-point conversion into `buf` (without sign or padding);
 * the length. */
static int format_double(char *buf, double v, char conv, int prec, int alt)
{
    int upper = conv == 'E' || conv == 'F' || conv == 'G';
    char kind = (char)(conv | 0x20);
    if (prec < 0) prec = 6;
    if (prec > PREC_MAX) prec = PREC_MAX;
    char d[DIGITS_MAX + 2];
    int nint, sticky, len = 0;

    if (kind == 'g' && prec == 0) prec = 1;
    if (kind == 'f') {
        int n = __libc_dtoa(v, d, DIGITS_MAX, 0, prec + 1, &nint, &sticky);
        nint += round_digits(d, n, nint + prec, sticky);
        if (nint == 0) buf[len++] = '0';
        memcpy(buf + len, d, (size_t)nint);
        len += nint;
        if (prec || alt) buf[len++] = '.';
        memcpy(buf + len, d + nint, (size_t)prec);
        return len + prec;
    }

    /* e and g: the significant digits and the decimal exponent. */
    int want = kind == 'e' ? prec + 1 : prec;
    int n = __libc_dtoa(v, d, DIGITS_MAX, want + 1, 0, &nint, &sticky);
    int first = 0;
    while (first < n && d[first] == '0') first++;
    int exp10;
    char *sig = d + first;
    int nsig = n - first;
    if (nsig == 0) {
        exp10 = 0;
        memset(d, '0', (size_t)want);
        sig = d;
        nsig = want;
    } else {
        exp10 = nint - first - 1;
        exp10 += round_digits(sig, nsig, want, sticky);
    }
    int use_e = kind == 'e' || exp10 < -4 || exp10 >= prec;
    int frac = kind == 'e' ? prec : (use_e ? prec - 1 : prec - 1 - exp10);
    if (use_e) {
        buf[len++] = sig[0];
        int nf = want - 1;
        if (kind == 'g' && !alt) while (nf > 0 && sig[nf] == '0') nf--;
        if (nf || alt) buf[len++] = '.';
        memcpy(buf + len, sig + 1, (size_t)nf);
        len += nf;
        buf[len++] = upper ? 'E' : 'e';
        buf[len++] = exp10 < 0 ? '-' : '+';
        int x = exp10 < 0 ? -exp10 : exp10;
        char t[4];
        int nt = 0;
        do { t[nt++] = (char)('0' + x % 10); x /= 10; } while (x);
        if (nt < 2) t[nt++] = '0';
        while (nt) buf[len++] = t[--nt];
        return len;
    }
    /* %g in fixed notation: `want` significant digits around the point. */
    if (exp10 < 0) {
        buf[len++] = '0';
        int nf = frac;
        int zeros = -exp10 - 1;
        char *f = buf + len + 1;
        memset(f, '0', (size_t)zeros);
        memcpy(f + zeros, sig, (size_t)want);
        if (!alt) while (nf > 0 && f[nf - 1] == '0') nf--;
        if (nf || alt) buf[len++] = '.';
        return len + nf;
    }
    memcpy(buf + len, sig, (size_t)exp10 + 1);
    len += exp10 + 1;
    int nf = frac;
    const char *f = sig + exp10 + 1;
    if (!alt) while (nf > 0 && f[nf - 1] == '0') nf--;
    if (nf || alt) buf[len++] = '.';
    memcpy(buf + len, f, (size_t)nf);
    return len + nf;
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
        int long_double = 0;
        for (;; fmt++) {
            if (*fmt == 'L') long_double = 1;
            else if (*fmt == 'h') size = size == -1 ? -2 : -1;
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
        case 'e': case 'E': case 'f': case 'F': case 'g': case 'G': {
            double v = long_double ? (double)__builtin_va_arg(ap, long double) : __builtin_va_arg(ap, double);
            uint64_t bits;
            memcpy(&bits, &v, sizeof bits);
            int negative = (int)(bits >> 63);
            bits &= ~(1ULL << 63);
            memcpy(&v, &bits, sizeof v);
            char body[DIGITS_MAX + 16];
            int blen;
            int upper = conv == 'E' || conv == 'F' || conv == 'G';
            if ((bits >> 52) == 0x7ff) {
                memcpy(body, (bits << 12) ? (upper ? "NAN" : "nan") : (upper ? "INF" : "inf"), 3);
                blen = 3;
                flags &= ~FL_ZERO;
            } else {
                blen = format_double(body, v, conv, prec, flags & FL_ALT);
            }
            char sign = negative ? '-' : (flags & FL_PLUS) ? '+' : (flags & FL_SPACE) ? ' ' : 0;
            int total_len = blen + (sign != 0);
            if (!(flags & FL_LEFT) && !(flags & FL_ZERO)) pad(&o, ' ', width - total_len);
            if (sign) emit(&o, &sign, 1);
            if (!(flags & FL_LEFT) && (flags & FL_ZERO)) pad(&o, '0', width - total_len);
            emit(&o, body, (size_t)blen);
            if (flags & FL_LEFT) pad(&o, ' ', width - total_len);
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

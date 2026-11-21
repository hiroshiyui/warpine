/* SPDX-License-Identifier: GPL-3.0-or-later */
/* strtod.c - Warpine's small C runtime (ADR-0100): strtod, atof.
 *
 * Correctly rounded, like dtoa.c in the other direction: the decimal
 * digits form a big integer D and the value is D * 10^E exactly. For
 * E >= 0 the product is computed; for E < 0 the quotient D / 10^-E is
 * taken with enough bits, and its remainder decides the rounding. The
 * result is rounded once, ties to even, and assembled from its bits.
 * Hexadecimal input (0x1.8p3), inf, infinity and nan are read too. */
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <ctype.h>
#include <errno.h>

/* Room for 800 digits (2658 bits) times 2^60, or 10^400. */
#define WORDS 96
#define MAX_DIGITS 800
#define DBL_MANT_BITS 53
#define DBL_MIN_EXP2 (-1022)
#define DBL_MAX_EXP2 1023

typedef struct { uint32_t w[WORDS]; int n; } big;

static void big_set(big *b, uint32_t v) { memset(b, 0, sizeof *b); b->w[0] = v; b->n = v ? 1 : 0; }

static void big_mul_add(big *b, uint32_t m, uint32_t a)
{
    uint64_t carry = a;
    for (int i = 0; i < b->n; i++) {
        uint64_t t = (uint64_t)b->w[i] * m + carry;
        b->w[i] = (uint32_t)t;
        carry = t >> 32;
    }
    if (carry && b->n < WORDS) b->w[b->n++] = (uint32_t)carry;
}

static int big_bits(const big *b)
{
    if (b->n == 0) return 0;
    return (b->n - 1) * 32 + (32 - __builtin_clz(b->w[b->n - 1]));
}

static int big_bit(const big *b, int i) { return i >= 0 && i / 32 < b->n && (b->w[i / 32] >> (i % 32)) & 1; }

static int big_any_below(const big *b, int i)
{
    for (int k = 0; k < i && k / 32 < b->n; k++)
        if (big_bit(b, k)) return 1;
    return 0;
}

static void big_shl(big *b, int s)
{
    if (s <= 0 || b->n == 0) return;
    int words = s / 32, bits = s % 32;
    int n = b->n + words + 1;
    if (n > WORDS) n = WORDS;
    for (int i = n - 1; i >= 0; i--) {
        uint32_t hi = (i - words >= 0 && i - words < b->n) ? b->w[i - words] : 0;
        uint32_t lo = (i - words - 1 >= 0 && i - words - 1 < b->n) ? b->w[i - words - 1] : 0;
        b->w[i] = bits ? (hi << bits) | (lo >> (32 - bits)) : hi;
    }
    b->n = n;
    while (b->n && !b->w[b->n - 1]) b->n--;
}

static int big_cmp(const big *a, const big *b)
{
    if (a->n != b->n) return a->n < b->n ? -1 : 1;
    for (int i = a->n - 1; i >= 0; i--)
        if (a->w[i] != b->w[i]) return a->w[i] < b->w[i] ? -1 : 1;
    return 0;
}

static void big_sub(big *a, const big *b)
{
    int64_t borrow = 0;
    for (int i = 0; i < a->n; i++) {
        int64_t t = (int64_t)a->w[i] - (i < b->n ? b->w[i] : 0) - borrow;
        borrow = t < 0;
        a->w[i] = (uint32_t)(t + (borrow ? (1LL << 32) : 0));
    }
    while (a->n && !a->w[a->n - 1]) a->n--;
}

/* q = a / d (q fits 64 bits here), a becomes the remainder. */
static uint64_t big_div(big *a, const big *d)
{
    int shift = big_bits(a) - big_bits(d);
    uint64_t q = 0;
    if (shift < 0) return 0;
    big t = *d;
    big_shl(&t, shift);
    for (; shift >= 0; shift--) {
        q <<= 1;
        if (big_cmp(a, &t) >= 0) { big_sub(a, &t); q |= 1; }
        /* t >>= 1 */
        for (int i = 0; i < t.n; i++) t.w[i] = (t.w[i] >> 1) | (i + 1 < t.n ? t.w[i + 1] << 31 : 0);
        while (t.n && !t.w[t.n - 1]) t.n--;
    }
    return q;
}

static double from_bits(uint64_t b) { double d; memcpy(&d, &b, sizeof d); return d; }

/* The double nearest to m * 2^k (m given as a big integer), ties to
 * even; `sticky`: the exact value is a little more than that. */
static double make_double(const big *m, int k, int sticky, int negative)
{
    uint64_t sign = negative ? 1ULL << 63 : 0;
    int len = big_bits(m);
    if (len == 0) return from_bits(sign);
    int e2 = len - 1 + k;                         /* exponent of the leading bit */
    if (e2 > DBL_MAX_EXP2) { errno = ERANGE; return from_bits(sign | 0x7FF0000000000000ULL); }
    int p = DBL_MANT_BITS;
    if (e2 < DBL_MIN_EXP2) p -= DBL_MIN_EXP2 - e2;
    if (p < 0) { errno = ERANGE; return from_bits(sign); }
    uint64_t mant = 0;
    for (int i = 0; i < p; i++) mant = (mant << 1) | (uint64_t)big_bit(m, len - 1 - i);
    int half = big_bit(m, len - 1 - p);
    sticky |= big_any_below(m, len - 1 - p);
    if (half && (sticky || (mant & 1))) mant++;
    uint64_t bits;
    if (e2 >= DBL_MIN_EXP2) {
        if (mant >> DBL_MANT_BITS) { mant >>= 1; e2++; }
        if (e2 > DBL_MAX_EXP2) { errno = ERANGE; return from_bits(sign | 0x7FF0000000000000ULL); }
        bits = ((uint64_t)(e2 + 1023) << 52) | (mant & ((1ULL << 52) - 1));
    } else {
        bits = mant;                              /* a carry into 2^52 is the smallest normal */
        if (mant == 0) errno = ERANGE;
    }
    return from_bits(sign | bits);
}

static int match_word(const char *s, const char *word)
{
    size_t n = strlen(word);
    for (size_t i = 0; i < n; i++)
        if (tolower((unsigned char)s[i]) != word[i]) return 0;
    return 1;
}

static int hex_digit(int c)
{
    if (isdigit(c)) return c - '0';
    if (isxdigit(c)) return tolower(c) - 'a' + 10;
    return -1;
}

double strtod(const char *s, char **end)
{
    const char *p = s;
    while (isspace((unsigned char)*p)) p++;
    int negative = 0;
    if (*p == '+' || *p == '-') negative = (*p++ == '-');

    if (match_word(p, "infinity") || match_word(p, "inf")) {
        if (end) *end = (char *)p + (match_word(p, "infinity") ? 8 : 3);
        return negative ? -__builtin_inf() : __builtin_inf();
    }
    if (match_word(p, "nan")) {
        p += 3;
        if (*p == '(') { const char *q = p + 1; while (isalnum((unsigned char)*q) || *q == '_') q++; if (*q == ')') p = q + 1; }
        if (end) *end = (char *)p;
        return negative ? -__builtin_nan("") : __builtin_nan("");
    }

    big m;
    big_set(&m, 0);
    int sticky = 0, any = 0;
    if (p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && (hex_digit((unsigned char)p[2]) >= 0
        || (p[2] == '.' && hex_digit((unsigned char)p[3]) >= 0))) {
        p += 2;
        int k = 0, seen_point = 0, digits = 0;
        for (;; p++) {
            int d = hex_digit((unsigned char)*p);
            if (*p == '.' && !seen_point) { seen_point = 1; continue; }
            if (d < 0) break;
            any = 1;
            if (digits < (WORDS - 4) * 8) {
                big_mul_add(&m, 16, (uint32_t)d);
                if (m.n) digits++;
                if (seen_point) k -= 4;
            } else {
                sticky |= d != 0;
                if (!seen_point) k += 4;
            }
        }
        if (*p == 'p' || *p == 'P') {
            const char *q = p + 1;
            int eneg = 0;
            if (*q == '+' || *q == '-') eneg = (*q++ == '-');
            if (isdigit((unsigned char)*q)) {
                long e = 0;
                while (isdigit((unsigned char)*q)) { if (e < 100000) e = e * 10 + (*q - '0'); q++; }
                k += (int)(eneg ? -e : e);
                p = q;
            }
        }
        if (end) *end = (char *)p;
        return make_double(&m, k, sticky, negative);
    }

    int digits = 0, exp10 = 0, seen_point = 0;
    for (;; p++) {
        if (*p == '.' && !seen_point) { seen_point = 1; continue; }
        if (!isdigit((unsigned char)*p)) break;
        any = 1;
        int d = *p - '0';
        if (digits < MAX_DIGITS) {
            if (d || m.n) { big_mul_add(&m, 10, (uint32_t)d); if (m.n) digits++; }
            if (seen_point) exp10--;
        } else {
            sticky |= d != 0;
            if (!seen_point) exp10++;
        }
    }
    if (!any) {
        if (end) *end = (char *)s;
        return 0.0;
    }
    if (*p == 'e' || *p == 'E') {
        const char *q = p + 1;
        int eneg = 0;
        if (*q == '+' || *q == '-') eneg = (*q++ == '-');
        if (isdigit((unsigned char)*q)) {
            long e = 0;
            while (isdigit((unsigned char)*q)) { if (e < 100000) e = e * 10 + (*q - '0'); q++; }
            exp10 += (int)(eneg ? -e : e);
            p = q;
        }
    }
    if (end) *end = (char *)p;
    if (m.n == 0) return negative ? -0.0 : 0.0;

    /* Far outside the double range: no arithmetic needed. */
    if (digits + exp10 > 310) { errno = ERANGE; return negative ? -__builtin_inf() : __builtin_inf(); }
    if (digits + exp10 < -330) { errno = ERANGE; return negative ? -0.0 : 0.0; }

    if (exp10 >= 0) {
        for (int i = 0; i < exp10; i++) big_mul_add(&m, 10, 0);
        return make_double(&m, 0, sticky, negative);
    }
    big d;
    big_set(&d, 1);
    for (int i = 0; i < -exp10; i++) big_mul_add(&d, 10, 0);
    /* Shift so the quotient has 56 or 57 significant bits: the dividend
     * up, or (when it is far longer) the divisor; value = q * 2^-s2. */
    int s2 = DBL_MANT_BITS + 3 + big_bits(&d) - big_bits(&m);
    if (s2 >= 0) big_shl(&m, s2);
    else big_shl(&d, -s2);
    uint64_t q = big_div(&m, &d);
    sticky |= m.n != 0;
    big qb;
    big_set(&qb, (uint32_t)q);
    qb.w[1] = (uint32_t)(q >> 32);
    qb.n = qb.w[1] ? 2 : (qb.w[0] ? 1 : 0);
    return make_double(&qb, -s2, sticky, negative);
}

double atof(const char *s) { return strtod(s, NULL); }

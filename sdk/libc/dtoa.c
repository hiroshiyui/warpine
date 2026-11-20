/* SPDX-License-Identifier: GPL-3.0-or-later */
/* dtoa.c - exact decimal digits of a double for printf (ADR-0098).
 *
 * A finite double is m * 2^e with m < 2^53. Its decimal expansion is
 * finite, so it is produced exactly with integer arithmetic: the integer
 * part is m << e (or m >> -e) written out in base 10, and each fraction
 * digit is the top of the remaining fraction times 10. Rounding then
 * happens once, on exact digits, with ties to even. No floating-point
 * instruction is used. */
#include <stdint.h>
#include <string.h>

#include "libc_internal.h"

/* 1074 fraction bits + headroom for the digit: 35 words. */
#define FRAC_WORDS 36
/* m << 971 is below 2^1024: 32 words. */
#define INT_WORDS  33

/* Divide the big number w[0..n) (least significant word first) by `d`
 * in place; the remainder. */
static uint32_t divide_small(uint32_t *w, int n, uint32_t d)
{
    uint64_t rem = 0;
    for (int i = n - 1; i >= 0; i--) {
        uint64_t cur = (rem << 32) | w[i];
        w[i] = (uint32_t)(cur / d);
        rem = cur % d;
    }
    return (uint32_t)rem;
}

static int is_zero(const uint32_t *w, int n)
{
    for (int i = 0; i < n; i++)
        if (w[i]) return 0;
    return 1;
}

int __libc_dtoa(double v, char *out, int out_max, int need_significant, int need_fraction,
                int *nint, int *sticky)
{
    uint64_t bits;
    memcpy(&bits, &v, sizeof bits);
    uint64_t m = bits & ((1ULL << 52) - 1);
    int exp_field = (int)((bits >> 52) & 0x7ff);
    int e;
    if (exp_field == 0) {
        e = -1074;
    } else {
        m |= 1ULL << 52;
        e = exp_field - 1075;
    }

    int n = 0;
    /* The integer part, most significant digit first. */
    uint32_t iw[INT_WORDS];
    memset(iw, 0, sizeof iw);
    if (e >= 0) {
        int word = e / 32, shift = e % 32;
        uint64_t lo = m << shift;                       /* bits 0..63 of the shifted value */
        uint64_t hi = shift ? m >> (64 - shift) : 0;    /* bits 64.. */
        iw[word] = (uint32_t)lo;
        iw[word + 1] = (uint32_t)(lo >> 32);
        if (word + 2 < INT_WORDS) iw[word + 2] = (uint32_t)hi;
    } else if (e > -64) {
        uint64_t ip = m >> -e;
        iw[0] = (uint32_t)ip;
        iw[1] = (uint32_t)(ip >> 32);
    }
    char rev[320];
    int nr = 0;
    while (!is_zero(iw, INT_WORDS)) {
        uint32_t chunk = divide_small(iw, INT_WORDS, 1000000000u);
        for (int k = 0; k < 9; k++) {
            rev[nr++] = (char)('0' + chunk % 10);
            chunk /= 10;
        }
    }
    while (nr > 0 && rev[nr - 1] == '0') nr--;          /* leading zeros of the top chunk */
    for (int k = nr - 1; k >= 0 && n < out_max; k--) out[n++] = rev[k];
    *nint = n;

    /* The fraction: f / 2^k with k = -e fraction bits. */
    uint32_t fw[FRAC_WORDS];
    memset(fw, 0, sizeof fw);
    int k = e < 0 ? -e : 0;
    if (k > 0) {
        uint64_t f = k >= 64 ? m : (m & ((1ULL << k) - 1));
        fw[0] = (uint32_t)f;
        fw[1] = (uint32_t)(f >> 32);
    }
    int significant = n;
    int fraction = 0;
    int words = k / 32 + 2;
    while (k > 0 && !is_zero(fw, words) && n < out_max
           && (significant < need_significant || fraction < need_fraction)) {
        uint32_t carry = 0;
        for (int i = 0; i < words; i++) {
            uint64_t t = (uint64_t)fw[i] * 10 + carry;
            fw[i] = (uint32_t)t;
            carry = (uint32_t)(t >> 32);
        }
        /* The digit is the value above bit k; then drop it. */
        int digit;
        if (k % 32) {
            digit = (int)(fw[k / 32] >> (k % 32));
            if (k / 32 + 1 < words) digit |= (int)(fw[k / 32 + 1] << (32 - k % 32));
            fw[k / 32] &= (1u << (k % 32)) - 1;
        } else {
            digit = (int)fw[k / 32];
            fw[k / 32] = 0;
        }
        for (int i = k / 32 + 1; i < words; i++) fw[i] = 0;
        out[n++] = (char)('0' + digit);
        fraction++;
        if (significant || digit) significant++;
    }
    *sticky = k > 0 && !is_zero(fw, words);
    return n;
}

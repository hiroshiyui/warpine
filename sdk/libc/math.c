/* SPDX-License-Identifier: GPL-3.0-or-later */
/* math.c - Warpine's small C runtime (ADR-0100): <math.h> on the x87.
 *
 * The x87 computes in 80-bit extended precision, so these results are
 * within an ulp or so of the exact value rather than always correctly
 * rounded. Rounding to integers (floor, ceil, trunc, round) and the
 * exponent functions (frexp, ldexp) work on the bits and are exact. */
#include <math.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>

static uint64_t bits_of(double x) { uint64_t b; memcpy(&b, &x, sizeof b); return b; }
static double double_of(uint64_t b) { double x; memcpy(&x, &b, sizeof x); return x; }

#define EXP_SHIFT 52
#define EXP_MASK  0x7FFULL
#define EXP_BIAS  1023
#define MANT_MASK ((1ULL << EXP_SHIFT) - 1)
#define SIGN_BIT  (1ULL << 63)

double fabs(double x) { return double_of(bits_of(x) & ~SIGN_BIT); }

double sqrt(double x)
{
    if (x < 0) { errno = EDOM; return NAN; }
    long double r;
    __asm__("fsqrt" : "=t"(r) : "0"((long double)x));
    return (double)r;
}

/* x without its fraction bits: towards zero. */
double trunc(double x)
{
    uint64_t b = bits_of(x);
    int e = (int)((b >> EXP_SHIFT) & EXP_MASK) - EXP_BIAS;
    if (e >= EXP_SHIFT) return x;                       /* integral, inf or nan */
    if (e < 0) return double_of(b & SIGN_BIT);          /* |x| < 1 */
    return double_of(b & ~(MANT_MASK >> e));
}

double floor(double x)
{
    double t = trunc(x);
    return (t != x && x < 0) ? t - 1.0 : t;
}

double ceil(double x)
{
    double t = trunc(x);
    return (t != x && x > 0) ? t + 1.0 : t;
}

/* Halfway cases away from zero. */
double round(double x)
{
    double t = trunc(x);
    double frac = fabs(x - t);
    if (frac >= 0.5) t += (x < 0) ? -1.0 : 1.0;
    return t;
}

double modf(double x, double *iptr)
{
    double t = trunc(x);
    *iptr = t;
    if (isinf(x)) return double_of(bits_of(x) & SIGN_BIT);
    return x - t;
}

double frexp(double x, int *exp)
{
    uint64_t b = bits_of(x);
    int e = (int)((b >> EXP_SHIFT) & EXP_MASK);
    *exp = 0;
    if (x == 0 || e == EXP_MASK) return x;
    if (e == 0) {                                       /* subnormal: scale up first */
        x *= 0x1p54;
        b = bits_of(x);
        e = (int)((b >> EXP_SHIFT) & EXP_MASK) - 54;
    }
    *exp = e - (EXP_BIAS - 1);
    return double_of((b & ~(EXP_MASK << EXP_SHIFT)) | ((uint64_t)(EXP_BIAS - 1) << EXP_SHIFT));
}

double ldexp(double x, int exp)
{
    long double r;
    __asm__("fscale" : "=t"(r) : "0"((long double)x), "u"((long double)exp));
    double d = (double)r;
    if (isinf(d) && !isinf(x)) errno = ERANGE;
    return d;
}

double fmod(double x, double y)
{
    if (y == 0 || isinf(x)) { errno = EDOM; return NAN; }
    long double r = x;
    unsigned short sw;
    do {
        __asm__("fprem\n\tfnstsw %1" : "+t"(r), "=a"(sw) : "u"((long double)y));
    } while (sw & 0x0400);                              /* C2: partial remainder */
    return (double)r;
}

/* 2^x for any finite x: f2xm1 on the fraction, fscale by the integer. */
static long double exp2l_x87(long double x)
{
    long double i, f, r;
    __asm__("frndint" : "=t"(i) : "0"(x));
    f = x - i;
    __asm__("f2xm1" : "=t"(r) : "0"(f));
    r += 1.0L;
    __asm__("fscale" : "=t"(r) : "0"(r), "u"(i));
    return r;
}

static long double log2l_x87(long double x)
{
    long double r;
    __asm__("fyl2x" : "=t"(r) : "0"(x), "u"(1.0L) : "st(1)");
    return r;
}

double exp(double x)
{
    if (isnan(x)) return x;
    if (isinf(x)) return x > 0 ? x : 0.0;
    if (x > 710.0) { errno = ERANGE; return HUGE_VAL; }
    if (x < -746.0) { errno = ERANGE; return 0.0; }
    return (double)exp2l_x87((long double)x * 1.44269504088896340735992468100189214L);
}

double log2(double x)
{
    if (x < 0) { errno = EDOM; return NAN; }
    if (x == 0) { errno = ERANGE; return -HUGE_VAL; }
    if (isinf(x) || isnan(x)) return x;
    return (double)log2l_x87(x);
}

double log(double x)
{
    if (x <= 0 || isinf(x) || isnan(x)) return log2(x);
    return (double)(log2l_x87(x) * 0.693147180559945309417232121458176568L);
}

double log10(double x)
{
    if (x <= 0 || isinf(x) || isnan(x)) return log2(x);
    return (double)(log2l_x87(x) * 0.301029995663981195213738894724493027L);
}

/* An infinity with the sign of x. */
static double copysign_inf(double x) { return double_of((bits_of(x) & SIGN_BIT) | (EXP_MASK << EXP_SHIFT)); }

/* Whether y is an integer, and an odd one. */
static int is_integer(double y) { return trunc(y) == y; }
static int is_odd_integer(double y) { return is_integer(y) && fmod(y, 2.0) != 0; }

double pow(double x, double y)
{
    if (y == 0 || x == 1) return 1.0;
    if (isnan(x) || isnan(y)) return NAN;
    if (x == 0) {
        if (y < 0) { errno = ERANGE; return is_odd_integer(y) ? copysign_inf(x) : HUGE_VAL; }
        return is_odd_integer(y) ? x : 0.0;
    }
    if (isinf(y)) {
        double ax = fabs(x);
        if (ax == 1) return 1.0;
        return (ax > 1) == (y > 0) ? HUGE_VAL : 0.0;
    }
    int negate = 0;
    if (x < 0) {
        if (!is_integer(y)) { errno = EDOM; return NAN; }
        negate = is_odd_integer(y);
        x = -x;
    }
    long double l = log2l_x87(x) * y;
    double r;
    if (l > 1100) { errno = ERANGE; r = HUGE_VAL; }
    else if (l < -1100) { errno = ERANGE; r = 0.0; }
    else r = (double)exp2l_x87(l);
    return negate ? -r : r;
}

double sin(double x)
{
    long double r;
    __asm__("fsin" : "=t"(r) : "0"((long double)x));
    return (double)r;
}

double cos(double x)
{
    long double r;
    __asm__("fcos" : "=t"(r) : "0"((long double)x));
    return (double)r;
}

double tan(double x)
{
    long double r, one;
    __asm__("fptan" : "=t"(one), "=u"(r) : "0"((long double)x));
    (void)one;
    return (double)r;
}

double atan2(double y, double x)
{
    long double r;
    __asm__("fpatan" : "=t"(r) : "0"((long double)x), "u"((long double)y) : "st(1)");
    return (double)r;
}

double atan(double x) { return atan2(x, 1.0); }

double asin(double x)
{
    if (x < -1 || x > 1) { errno = EDOM; return NAN; }
    long double lx = x;
    return atan2(x, (double)sqrt((double)((1.0L - lx) * (1.0L + lx))));
}

double acos(double x)
{
    if (x < -1 || x > 1) { errno = EDOM; return NAN; }
    long double lx = x;
    return atan2((double)sqrt((double)((1.0L - lx) * (1.0L + lx))), x);
}

double sinh(double x) { long double e = exp2l_x87(x * 1.44269504088896340736L); return (double)((e - 1.0L / e) / 2); }
double cosh(double x) { long double e = exp2l_x87(x * 1.44269504088896340736L); return (double)((e + 1.0L / e) / 2); }

double tanh(double x)
{
    if (x > 22) return 1.0;
    if (x < -22) return -1.0;
    long double e = exp2l_x87(2 * x * 1.44269504088896340736L);
    return (double)((e - 1) / (e + 1));
}

/* The extended exponent range keeps x*x + y*y from overflowing. */
double hypot(double x, double y)
{
    if (isinf(x) || isinf(y)) return HUGE_VAL;
    long double s = (long double)x * x + (long double)y * y, r;
    __asm__("fsqrt" : "=t"(r) : "0"(s));
    return (double)r;
}

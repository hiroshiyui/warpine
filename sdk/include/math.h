/* SPDX-License-Identifier: GPL-3.0-or-later */
/* math.h - Warpine's small C runtime (ADR-0100): the double functions,
 * on the x87 instructions every i386 has; float and long double
 * variants are not provided yet. */
#ifndef WARPINE_MATH_H
#define WARPINE_MATH_H

#ifdef __cplusplus
extern "C" {
#endif

#define HUGE_VAL  __builtin_huge_val()
#define INFINITY  __builtin_inff()
#define NAN       __builtin_nanf("")

#define M_E        2.71828182845904523536
#define M_LOG2E    1.44269504088896340736
#define M_LOG10E   0.43429448190325182765
#define M_LN2      0.69314718055994530942
#define M_LN10     2.30258509299404568402
#define M_PI       3.14159265358979323846
#define M_PI_2     1.57079632679489661923
#define M_PI_4     0.78539816339744830962
#define M_SQRT2    1.41421356237309504880

#define isnan(x)    __builtin_isnan(x)
#define isinf(x)    __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x)  __builtin_signbit(x)

double fabs(double x);
double sqrt(double x);
double floor(double x);
double ceil(double x);
double trunc(double x);
double round(double x);
double fmod(double x, double y);
double modf(double x, double *iptr);
double frexp(double x, int *exp);
double ldexp(double x, int exp);
double exp(double x);
double log(double x);
double log10(double x);
double log2(double x);
double pow(double x, double y);
double sin(double x);
double cos(double x);
double tan(double x);
double asin(double x);
double acos(double x);
double atan(double x);
double atan2(double y, double x);
double sinh(double x);
double cosh(double x);
double tanh(double x);
double hypot(double x, double y);

#ifdef __cplusplus
}
#endif

#endif

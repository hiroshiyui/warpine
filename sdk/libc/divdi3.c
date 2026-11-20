/* SPDX-License-Identifier: GPL-3.0-or-later */
/* divdi3.c - 64-bit division for i386 (ADR-0096). GCC and clang call these
 * for long long `/` and `%`; a freestanding link has no libgcc or
 * compiler-rt, so the runtime carries them: shift-and-subtract long
 * division, one quotient bit a step. */
typedef unsigned long long u64;
typedef long long s64;

u64 __udivmoddi4(u64 num, u64 den, u64 *rem);
u64 __udivdi3(u64 num, u64 den);
u64 __umoddi3(u64 num, u64 den);
s64 __divdi3(s64 num, s64 den);
s64 __moddi3(s64 num, s64 den);

u64 __udivmoddi4(u64 num, u64 den, u64 *rem)
{
    if (den == 0) {
        /* As the hardware would: a division by zero traps. */
        volatile unsigned zero = 0;
        return (u64)(1u / zero);
    }
    u64 quot = 0, r = 0;
    for (int bit = 63; bit >= 0; bit--) {
        r = (r << 1) | ((num >> bit) & 1);
        if (r >= den) {
            r -= den;
            quot |= 1ULL << bit;
        }
    }
    if (rem) *rem = r;
    return quot;
}

u64 __udivdi3(u64 num, u64 den) { return __udivmoddi4(num, den, 0); }

u64 __umoddi3(u64 num, u64 den)
{
    u64 r = 0;
    __udivmoddi4(num, den, &r);
    return r;
}

static u64 magnitude(s64 v) { return v < 0 ? 0 - (u64)v : (u64)v; }

s64 __divdi3(s64 num, s64 den)
{
    u64 q = __udivmoddi4(magnitude(num), magnitude(den), 0);
    return ((num < 0) != (den < 0)) ? (s64)(0 - q) : (s64)q;
}

s64 __moddi3(s64 num, s64 den)
{
    u64 r = 0;
    __udivmoddi4(magnitude(num), magnitude(den), &r);
    return num < 0 ? (s64)(0 - r) : (s64)r;
}

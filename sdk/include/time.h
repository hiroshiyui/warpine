/* SPDX-License-Identifier: GPL-3.0-or-later */
/* time.h - Warpine's small C runtime (ADR-0101): calendar time from
 * DosGetDateTime. time_t counts seconds since 1970-01-01 UTC in 64 bits,
 * so dates past 2038 work; local time applies DATETIME.timezone (minutes
 * west of UTC). */
#ifndef WARPINE_TIME_H
#define WARPINE_TIME_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef long long time_t;
typedef unsigned long clock_t;
#define CLOCKS_PER_SEC 1000

struct tm {
    int tm_sec;
    int tm_min;
    int tm_hour;
    int tm_mday;
    int tm_mon;
    int tm_year;
    int tm_wday;
    int tm_yday;
    int tm_isdst;
};

time_t     time(time_t *t);
double     difftime(time_t end, time_t start);
time_t     mktime(struct tm *tm);
struct tm *gmtime(const time_t *t);
struct tm *localtime(const time_t *t);
struct tm *gmtime_r(const time_t *t, struct tm *out);
struct tm *localtime_r(const time_t *t, struct tm *out);
clock_t    clock(void);
size_t     strftime(char *buf, size_t size, const char *fmt, const struct tm *tm);

#ifdef __cplusplus
}
#endif

#endif

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* time.c - Warpine's small C runtime (ADR-0101): <time.h> on
 * DosGetDateTime and DosQuerySysInfo(QSV_MS_COUNT). The calendar is the
 * proleptic Gregorian one, computed from day counts (no tables). */
#define INCL_DOSDATETIME
#include <os2.h>
#include <time.h>
#include <string.h>

#define SECS_PER_DAY 86400LL

/* Days from 1970-01-01 to y-m-d (m 1..12). */
static long long days_from_civil(long long y, unsigned m, unsigned d)
{
    y -= m <= 2;
    long long era = (y >= 0 ? y : y - 399) / 400;
    unsigned yoe = (unsigned)(y - era * 400);
    unsigned doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    unsigned doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + (long long)doe - 719468;
}

static int is_leap(long long y) { return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }

/* The local time's offset east of UTC, in seconds, from DATETIME. */
static long long local_offset(void)
{
    DATETIME dt;
    if (DosGetDateTime(&dt) != NO_ERROR || dt.timezone == -1) return 0;
    return -(long long)dt.timezone * 60;
}

time_t time(time_t *t)
{
    DATETIME dt;
    time_t now = 0;
    if (DosGetDateTime(&dt) == NO_ERROR) {
        now = days_from_civil(dt.year, dt.month, dt.day) * SECS_PER_DAY
            + dt.hours * 3600LL + dt.minutes * 60LL + dt.seconds;
        if (dt.timezone != -1) now += dt.timezone * 60LL;      /* local -> UTC */
    }
    if (t) *t = now;
    return now;
}

double difftime(time_t end, time_t start) { return (double)(end - start); }

struct tm *gmtime_r(const time_t *t, struct tm *out)
{
    long long s = *t;
    long long days = s / SECS_PER_DAY, rem = s % SECS_PER_DAY;
    if (rem < 0) { rem += SECS_PER_DAY; days--; }
    out->tm_hour = (int)(rem / 3600);
    out->tm_min = (int)(rem % 3600 / 60);
    out->tm_sec = (int)(rem % 60);
    out->tm_wday = (int)(((days % 7) + 11) % 7);               /* 1970-01-01 was a Thursday */
    /* civil_from_days */
    long long z = days + 719468;
    long long era = (z >= 0 ? z : z - 146096) / 146097;
    unsigned doe = (unsigned)(z - era * 146097);
    unsigned yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    long long y = (long long)yoe + era * 400;
    unsigned doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    unsigned mp = (5 * doy + 2) / 153;
    unsigned d = doy - (153 * mp + 2) / 5 + 1;
    unsigned m = mp < 10 ? mp + 3 : mp - 9;
    y += m <= 2;
    out->tm_year = (int)(y - 1900);
    out->tm_mon = (int)m - 1;
    out->tm_mday = (int)d;
    out->tm_yday = (int)(days - days_from_civil(y, 1, 1));
    out->tm_isdst = 0;
    return out;
}

struct tm *localtime_r(const time_t *t, struct tm *out)
{
    time_t local = *t + local_offset();
    return gmtime_r(&local, out);
}

struct tm *gmtime(const time_t *t)
{
    static struct tm result;
    return gmtime_r(t, &result);
}

struct tm *localtime(const time_t *t)
{
    static struct tm result;
    return localtime_r(t, &result);
}

/* Normalises the fields and gives local time's time_t. */
time_t mktime(struct tm *tm)
{
    long long year = tm->tm_year + 1900LL + tm->tm_mon / 12;
    int mon = tm->tm_mon % 12;
    if (mon < 0) { mon += 12; year--; }
    long long s = days_from_civil(year, (unsigned)mon + 1, 1) * SECS_PER_DAY
                + (tm->tm_mday - 1) * SECS_PER_DAY + tm->tm_hour * 3600LL + tm->tm_min * 60LL + tm->tm_sec;
    time_t t = s - local_offset();
    localtime_r(&t, tm);
    return t;
}

clock_t clock(void)
{
    ULONG ms = 0;
    DosQuerySysInfo(QSV_MS_COUNT, QSV_MS_COUNT, &ms, sizeof ms);
    return ms;
}

static const char *const day_names[] = { "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday",
                                         "Saturday" };
static const char *const month_names[] = { "January", "February", "March", "April", "May", "June", "July",
                                           "August", "September", "October", "November", "December" };

/* The C locale's conversions: a A b B c d H I j m M p S U w y Y Z %. */
size_t strftime(char *buf, size_t size, const char *fmt, const struct tm *tm)
{
    size_t n = 0;
    char tmp[32];
    for (; *fmt; fmt++) {
        const char *piece = tmp;
        size_t len;
        if (*fmt != '%' || !fmt[1]) {
            tmp[0] = *fmt;
            len = 1;
        } else {
            fmt++;
            int v = -1, width = 2;
            switch (*fmt) {
            case 'a': piece = day_names[tm->tm_wday % 7]; len = 3; goto copy;
            case 'A': piece = day_names[tm->tm_wday % 7]; len = strlen(piece); goto copy;
            case 'b': piece = month_names[tm->tm_mon % 12]; len = 3; goto copy;
            case 'B': piece = month_names[tm->tm_mon % 12]; len = strlen(piece); goto copy;
            case 'c': {
                struct tm c = *tm;
                len = strftime(tmp, sizeof tmp, "%a %b %d %H:%M:%S %Y", &c);
                goto copy;
            }
            case 'd': v = tm->tm_mday; break;
            case 'H': v = tm->tm_hour; break;
            case 'I': v = tm->tm_hour % 12 ? tm->tm_hour % 12 : 12; break;
            case 'j': v = tm->tm_yday + 1; width = 3; break;
            case 'm': v = tm->tm_mon + 1; break;
            case 'M': v = tm->tm_min; break;
            case 'p': piece = tm->tm_hour < 12 ? "AM" : "PM"; len = 2; goto copy;
            case 'S': v = tm->tm_sec; break;
            case 'U': v = (tm->tm_yday + 7 - tm->tm_wday) / 7; break;
            case 'w': v = tm->tm_wday; width = 1; break;
            case 'y': v = (tm->tm_year + 1900) % 100; break;
            case 'Y': v = tm->tm_year + 1900; width = 4; break;
            case 'Z': len = 0; goto copy;
            case '%': tmp[0] = '%'; len = 1; goto copy;
            default: tmp[0] = '%'; tmp[1] = *fmt; len = 2; goto copy;
            }
            /* v >= 0 here */
            len = 0;
            char digits[12];
            int k = 0;
            do { digits[k++] = (char)('0' + v % 10); v /= 10; } while (v);
            while (k < width) digits[k++] = '0';
            while (k) tmp[len++] = digits[--k];
        }
    copy:
        if (n + len >= size) return 0;
        memcpy(buf + n, piece, len);
        n += len;
    }
    if (n >= size) return 0;
    buf[n] = '\0';
    return n;
}

int __libc_is_leap_year(long long y) { return is_leap(y); }

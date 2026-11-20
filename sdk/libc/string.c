/* SPDX-License-Identifier: GPL-3.0-or-later */
/* string.c - Warpine's small C runtime (ADR-0096). The compilers also call
 * memcpy, memmove, memset and memcmp on their own, so these must not be
 * turned back into calls to themselves (-ffreestanding implies
 * -fno-builtin, and the loops below are plain). */
#include <string.h>
#include <stdlib.h>
#include <ctype.h>
#include <errno.h>

void *memcpy(void *dst, const void *src, size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dst;
}

void *memmove(void *dst, const void *src, size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else if (d > s) {
        d += n; s += n;
        while (n--) *--d = *--s;
    }
    return dst;
}

void *memset(void *dst, int c, size_t n)
{
    unsigned char *d = dst;
    while (n--) *d++ = (unsigned char)c;
    return dst;
}

int memcmp(const void *a, const void *b, size_t n)
{
    const unsigned char *x = a, *y = b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x - *y;
    return 0;
}

void *memchr(const void *s, int c, size_t n)
{
    const unsigned char *p = s;
    for (; n; n--, p++)
        if (*p == (unsigned char)c) return (void *)p;
    return NULL;
}

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

size_t strnlen(const char *s, size_t max)
{
    size_t n = 0;
    while (n < max && s[n]) n++;
    return n;
}

char *strcpy(char *dst, const char *src)
{
    char *d = dst;
    while ((*d++ = *src++)) { }
    return dst;
}

char *strncpy(char *dst, const char *src, size_t n)
{
    size_t i = 0;
    for (; i < n && src[i]; i++) dst[i] = src[i];
    for (; i < n; i++) dst[i] = '\0';
    return dst;
}

char *strcat(char *dst, const char *src)
{
    strcpy(dst + strlen(dst), src);
    return dst;
}

char *strncat(char *dst, const char *src, size_t n)
{
    char *d = dst + strlen(dst);
    while (n-- && *src) *d++ = *src++;
    *d = '\0';
    return dst;
}

int strcmp(const char *a, const char *b)
{
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n)
{
    for (; n; n--, a++, b++) {
        if (*a != *b) return (unsigned char)*a - (unsigned char)*b;
        if (!*a) break;
    }
    return 0;
}

int stricmp(const char *a, const char *b)
{
    int x, y;
    do {
        x = tolower((unsigned char)*a++);
        y = tolower((unsigned char)*b++);
    } while (x && x == y);
    return x - y;
}

int strnicmp(const char *a, const char *b, size_t n)
{
    for (; n; n--) {
        int x = tolower((unsigned char)*a++), y = tolower((unsigned char)*b++);
        if (x != y) return x - y;
        if (!x) break;
    }
    return 0;
}

char *strchr(const char *s, int c)
{
    for (;; s++) {
        if (*s == (char)c) return (char *)s;
        if (!*s) return NULL;
    }
}

char *strrchr(const char *s, int c)
{
    const char *last = NULL;
    for (;; s++) {
        if (*s == (char)c) last = s;
        if (!*s) return (char *)last;
    }
}

char *strstr(const char *haystack, const char *needle)
{
    size_t n = strlen(needle);
    if (!n) return (char *)haystack;
    for (; *haystack; haystack++)
        if (*haystack == *needle && !strncmp(haystack, needle, n)) return (char *)haystack;
    return NULL;
}

size_t strspn(const char *s, const char *accept)
{
    size_t n = 0;
    while (s[n] && strchr(accept, s[n])) n++;
    return n;
}

size_t strcspn(const char *s, const char *reject)
{
    size_t n = 0;
    while (s[n] && !strchr(reject, s[n])) n++;
    return n;
}

char *strpbrk(const char *s, const char *accept)
{
    s += strcspn(s, accept);
    return *s ? (char *)s : NULL;
}

char *strtok_r(char *s, const char *delim, char **save)
{
    if (!s) s = *save;
    s += strspn(s, delim);
    if (!*s) { *save = s; return NULL; }
    char *end = s + strcspn(s, delim);
    if (*end) *end++ = '\0';
    *save = end;
    return s;
}

char *strtok(char *s, const char *delim)
{
    static char *save;
    return strtok_r(s, delim, &save);
}

char *strdup(const char *s)
{
    size_t n = strlen(s) + 1;
    char *d = malloc(n);
    return d ? memcpy(d, s, n) : NULL;
}

char *strerror(int errnum)
{
    switch (errnum) {
    case 0:       return "No error";
    case EPERM:   return "Operation not permitted";
    case ENOENT:  return "No such file or directory";
    case EIO:     return "Input/output error";
    case EBADF:   return "Bad file handle";
    case EAGAIN:  return "Resource temporarily unavailable";
    case ENOMEM:  return "Not enough memory";
    case EACCES:  return "Access denied";
    case EEXIST:  return "File exists";
    case ENOTDIR: return "Not a directory";
    case EISDIR:  return "Is a directory";
    case EINVAL:  return "Invalid argument";
    case EMFILE:  return "Too many open files";
    case ENOSPC:  return "No space left on device";
    case ESPIPE:  return "Invalid seek";
    case EPIPE:   return "Broken pipe";
    case EDOM:    return "Argument out of domain";
    case ERANGE:  return "Result out of range";
    default:      return "Unknown error";
    }
}

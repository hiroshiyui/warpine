/* SPDX-License-Identifier: GPL-3.0-or-later */
/* stdio.c - Warpine's small C runtime (ADR-0096): streams on OS/2 file
 * handles. Writes go straight to DosWrite (nothing waits in a buffer);
 * reads fill a BUFSIZ read-ahead buffer, which a seek or a write drops. */
#define INCL_DOSFILEMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

#include "libc_internal.h"

#define F_READ    0x01
#define F_WRITE   0x02
#define F_APPEND  0x04
#define F_EOF     0x08
#define F_ERR     0x10
#define F_OPEN    0x20
#define F_STATIC  0x40   /* stdin / stdout / stderr: never freed */

struct _WARPINE_FILE {
    HFILE  handle;
    int    flags;
    int    unget;         /* a pushed-back byte, or EOF */
    size_t pos, len;      /* the unread part of `buf` */
    unsigned char buf[BUFSIZ];
};

static FILE std_streams[3] = {
    { HF_STDIN,  F_OPEN | F_STATIC | F_READ,  EOF, 0, 0, {0} },
    { HF_STDOUT, F_OPEN | F_STATIC | F_WRITE, EOF, 0, 0, {0} },
    { HF_STDERR, F_OPEN | F_STATIC | F_WRITE, EOF, 0, 0, {0} },
};
FILE *stdin  = &std_streams[0];
FILE *stdout = &std_streams[1];
FILE *stderr = &std_streams[2];

static FILE *open_files[FOPEN_MAX];
static libc_lock files_lock;

/* Drop read-ahead: move the file position back to what the caller has
 * consumed. */
static void drop_readahead(FILE *f)
{
    long back = (long)(f->len - f->pos) + (f->unget != EOF ? 1 : 0);
    if (back) {
        ULONG where;
        DosSetFilePtr(f->handle, -back, FILE_CURRENT, &where);
    }
    f->pos = f->len = 0;
    f->unget = EOF;
}

FILE *fopen(const char *path, const char *mode)
{
    ULONG access, action_flags;
    int flags = F_OPEN;
    int plus = strchr(mode, '+') != NULL;
    switch (mode[0]) {
    case 'r':
        access = plus ? OPEN_ACCESS_READWRITE : OPEN_ACCESS_READONLY;
        action_flags = OPEN_ACTION_OPEN_IF_EXISTS | OPEN_ACTION_FAIL_IF_NEW;
        flags |= F_READ | (plus ? F_WRITE : 0);
        break;
    case 'w':
        access = plus ? OPEN_ACCESS_READWRITE : OPEN_ACCESS_WRITEONLY;
        action_flags = OPEN_ACTION_REPLACE_IF_EXISTS | OPEN_ACTION_CREATE_IF_NEW;
        flags |= F_WRITE | (plus ? F_READ : 0);
        break;
    case 'a':
        access = plus ? OPEN_ACCESS_READWRITE : OPEN_ACCESS_WRITEONLY;
        action_flags = OPEN_ACTION_OPEN_IF_EXISTS | OPEN_ACTION_CREATE_IF_NEW;
        flags |= F_WRITE | F_APPEND | (plus ? F_READ : 0);
        break;
    default:
        errno = EINVAL;
        return NULL;
    }
    FILE *f = malloc(sizeof *f);
    if (!f) return NULL;
    ULONG action;
    APIRET rc = DosOpen(path, &f->handle, &action, 0, FILE_NORMAL, action_flags,
                        access | OPEN_SHARE_DENYNONE | OPEN_FLAGS_NOINHERIT, NULL);
    if (rc != NO_ERROR) {
        free(f);
        errno = __libc_errno_of(rc);
        return NULL;
    }
    f->flags = flags;
    f->unget = EOF;
    f->pos = f->len = 0;
    __libc_lock(&files_lock);
    for (int i = 0; i < FOPEN_MAX; i++)
        if (!open_files[i]) { open_files[i] = f; break; }
    __libc_unlock(&files_lock);
    return f;
}

int fclose(FILE *f)
{
    if (!f || !(f->flags & F_OPEN)) { errno = EBADF; return EOF; }
    APIRET rc = DosClose(f->handle);
    f->flags &= ~F_OPEN;
    if (!(f->flags & F_STATIC)) {
        __libc_lock(&files_lock);
        for (int i = 0; i < FOPEN_MAX; i++)
            if (open_files[i] == f) open_files[i] = NULL;
        __libc_unlock(&files_lock);
        free(f);
    }
    if (rc != NO_ERROR) { errno = __libc_errno_of(rc); return EOF; }
    return 0;
}

void __libc_close_streams(void)
{
    for (int i = 0; i < FOPEN_MAX; i++)
        if (open_files[i]) fclose(open_files[i]);
}

size_t fwrite(const void *buf, size_t size, size_t count, FILE *f)
{
    size_t total = size * count;
    if (!total) return 0;
    if (!(f->flags & F_WRITE)) { f->flags |= F_ERR; errno = EBADF; return 0; }
    if (f->len || f->unget != EOF) drop_readahead(f);
    if (f->flags & F_APPEND) {
        ULONG where;
        DosSetFilePtr(f->handle, 0, FILE_END, &where);
    }
    const unsigned char *p = buf;
    size_t done = 0;
    while (done < total) {
        ULONG wrote = 0;
        APIRET rc = DosWrite(f->handle, p + done, total - done, &wrote);
        if (rc != NO_ERROR || wrote == 0) {
            f->flags |= F_ERR;
            errno = rc ? __libc_errno_of(rc) : ENOSPC;
            break;
        }
        done += wrote;
    }
    return done / size;
}

/* Refill the read-ahead buffer; 0 at the end of the file or on an error. */
static int fill(FILE *f)
{
    ULONG got = 0;
    APIRET rc = DosRead(f->handle, f->buf, sizeof f->buf, &got);
    f->pos = 0;
    f->len = got;
    if (rc != NO_ERROR) { f->flags |= F_ERR; errno = __libc_errno_of(rc); return 0; }
    if (got == 0) { f->flags |= F_EOF; return 0; }
    return 1;
}

int fgetc(FILE *f)
{
    if (!(f->flags & F_READ)) { f->flags |= F_ERR; errno = EBADF; return EOF; }
    if (f->unget != EOF) {
        int c = f->unget;
        f->unget = EOF;
        return c;
    }
    if (f->pos == f->len && !fill(f)) return EOF;
    return f->buf[f->pos++];
}

int getc(FILE *f)    { return fgetc(f); }
int getchar(void)    { return fgetc(stdin); }

int ungetc(int c, FILE *f)
{
    if (c == EOF || f->unget != EOF) return EOF;
    f->unget = (unsigned char)c;
    f->flags &= ~F_EOF;
    return (unsigned char)c;
}

size_t fread(void *buf, size_t size, size_t count, FILE *f)
{
    size_t total = size * count, done = 0;
    unsigned char *p = buf;
    while (done < total) {
        int c = fgetc(f);
        if (c == EOF) break;
        p[done++] = (unsigned char)c;
        size_t avail = f->len - f->pos;
        if (avail > total - done) avail = total - done;
        memcpy(p + done, f->buf + f->pos, avail);
        f->pos += avail;
        done += avail;
    }
    return size ? done / size : 0;
}

char *fgets(char *buf, int size, FILE *f)
{
    int n = 0;
    while (n < size - 1) {
        int c = fgetc(f);
        if (c == EOF) break;
        buf[n++] = (char)c;
        if (c == '\n') break;
    }
    if (n == 0 || size <= 0) return NULL;
    buf[n] = '\0';
    return buf;
}

int fputc(int c, FILE *f)
{
    unsigned char ch = (unsigned char)c;
    return fwrite(&ch, 1, 1, f) == 1 ? ch : EOF;
}

int putc(int c, FILE *f) { return fputc(c, f); }
int putchar(int c)       { return fputc(c, stdout); }

int fputs(const char *s, FILE *f)
{
    size_t n = strlen(s);
    return fwrite(s, 1, n, f) == n ? 0 : EOF;
}

int puts(const char *s)
{
    return (fputs(s, stdout) == EOF || fputc('\n', stdout) == EOF) ? EOF : 0;
}

int fflush(FILE *f)
{
    (void)f;   /* nothing is ever buffered for output */
    return 0;
}

int fseek(FILE *f, long offset, int whence)
{
    if (whence == SEEK_CUR) offset -= (long)(f->len - f->pos) + (f->unget != EOF ? 1 : 0);
    f->pos = f->len = 0;
    f->unget = EOF;
    ULONG where;
    ULONG method = whence == SEEK_SET ? FILE_BEGIN : whence == SEEK_CUR ? FILE_CURRENT : FILE_END;
    APIRET rc = DosSetFilePtr(f->handle, offset, method, &where);
    if (rc != NO_ERROR) { errno = __libc_errno_of(rc); return -1; }
    f->flags &= ~F_EOF;
    return 0;
}

long ftell(FILE *f)
{
    ULONG where;
    APIRET rc = DosSetFilePtr(f->handle, 0, FILE_CURRENT, &where);
    if (rc != NO_ERROR) { errno = __libc_errno_of(rc); return -1; }
    return (long)where - (long)(f->len - f->pos) - (f->unget != EOF ? 1 : 0);
}

void rewind(FILE *f)     { fseek(f, 0, SEEK_SET); f->flags &= ~F_ERR; }
int feof(FILE *f)        { return (f->flags & F_EOF) != 0; }
int ferror(FILE *f)      { return (f->flags & F_ERR) != 0; }
void clearerr(FILE *f)   { f->flags &= ~(F_EOF | F_ERR); }

int remove(const char *path)
{
    APIRET rc = DosDelete(path);
    if (rc != NO_ERROR) { errno = __libc_errno_of(rc); return -1; }
    return 0;
}

int rename(const char *from, const char *to)
{
    APIRET rc = DosMove(from, to);
    if (rc != NO_ERROR) { errno = __libc_errno_of(rc); return -1; }
    return 0;
}

/* ── printf to streams ──────────────────────────────────────────────── */

/* Formatted text gathers here and goes out in one write per call. */
typedef struct {
    FILE  *f;
    size_t len;
    char   buf[256];
} stream_sink;

static void stream_put(void *ctx, const char *s, size_t n)
{
    stream_sink *k = ctx;
    while (n) {
        if (k->len == sizeof k->buf) {
            fwrite(k->buf, 1, k->len, k->f);
            k->len = 0;
        }
        size_t room = sizeof k->buf - k->len;
        size_t take = n < room ? n : room;
        memcpy(k->buf + k->len, s, take);
        k->len += take;
        s += take;
        n -= take;
    }
}

int vfprintf(FILE *f, const char *fmt, va_list ap)
{
    stream_sink k;
    k.f = f;
    k.len = 0;
    int n = __libc_format(stream_put, &k, fmt, ap);
    if (k.len) fwrite(k.buf, 1, k.len, f);
    return ferror(f) ? -1 : n;
}

int vprintf(const char *fmt, va_list ap) { return vfprintf(stdout, fmt, ap); }

int fprintf(FILE *f, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf(f, fmt, ap);
    va_end(ap);
    return n;
}

int printf(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = vfprintf(stdout, fmt, ap);
    va_end(ap);
    return n;
}

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* stdio.h - Warpine's small C runtime (ADR-0096).
 *
 * Streams sit directly on OS/2 file handles. Output is written through
 * at the end of every call (no buffering to flush); input is read ahead
 * a block at a time. printf knows integers, characters, strings and
 * pointers, not floating point. */
#ifndef WARPINE_STDIO_H
#define WARPINE_STDIO_H

#include <stddef.h>
#include <stdarg.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EOF      (-1)
#define BUFSIZ   512
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define FOPEN_MAX 32
#define _IOFBF   0
#define _IOLBF   1
#define _IONBF   2

typedef struct _WARPINE_FILE FILE;

extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

FILE  *fopen(const char *path, const char *mode);
int    fclose(FILE *f);
size_t fread(void *buf, size_t size, size_t count, FILE *f);
size_t fwrite(const void *buf, size_t size, size_t count, FILE *f);
int    fgetc(FILE *f);
int    getc(FILE *f);
int    getchar(void);
int    ungetc(int c, FILE *f);
char  *fgets(char *buf, int size, FILE *f);
int    fputc(int c, FILE *f);
int    putc(int c, FILE *f);
int    putchar(int c);
int    fputs(const char *s, FILE *f);
int    puts(const char *s);
int    fflush(FILE *f);
int    setvbuf(FILE *f, char *buf, int mode, size_t size);
int    fseek(FILE *f, long offset, int whence);
long   ftell(FILE *f);
void   rewind(FILE *f);
int    feof(FILE *f);
int    ferror(FILE *f);
void   clearerr(FILE *f);
int    remove(const char *path);
int    rename(const char *from, const char *to);

int    printf(const char *fmt, ...) __attribute__((__format__(__printf__, 1, 2)));
int    fprintf(FILE *f, const char *fmt, ...) __attribute__((__format__(__printf__, 2, 3)));
int    sprintf(char *buf, const char *fmt, ...) __attribute__((__format__(__printf__, 2, 3)));
int    snprintf(char *buf, size_t size, const char *fmt, ...) __attribute__((__format__(__printf__, 3, 4)));
int    vprintf(const char *fmt, va_list ap);
int    vfprintf(FILE *f, const char *fmt, va_list ap);
int    vsprintf(char *buf, const char *fmt, va_list ap);
int    vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int    sscanf(const char *str, const char *fmt, ...) __attribute__((__format__(__scanf__, 2, 3)));
int    vsscanf(const char *str, const char *fmt, va_list ap);

#ifdef __cplusplus
}
#endif

#endif

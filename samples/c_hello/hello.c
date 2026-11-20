/* SPDX-License-Identifier: GPL-3.0-or-later */
/* hello.c - a C program built by a modern compiler (GCC or clang) as i386
 * ELF objects and linked into an OS/2 LX executable by lx-link (M1).
 *
 * It checks what such objects bring: string literals at offsets in merged
 * string sections, tables of pointers in data, zero-initialised data, a
 * common symbol shared by two objects, a constructor and a destructor,
 * sections per function, two objects with sections of the same names.
 * Writes "c_hello: <name>: ok" or "... FAIL", then "c_hello: done p/t". */
#include "os2min.h"

void put(const char *s);
unsigned str_len(const char *s);
const char *util_word(int i);
const char *util_name(void);
int util_call_count(void);

int shared_counter;                    /* common, merged with util.c's */
static const char hello_tag[] = "hello";
static const char *const greetings[] = { "first", "second", "third" };
static int zeroes[256];                /* .bss */
static int constructed;
static int passed, total;

__attribute__((constructor)) static void construct(void) { constructed = 42; }
__attribute__((destructor)) static void destruct(void) { put("c_hello: the destructor ran\r\n"); }

static int same(const char *a, const char *b)
{
    while (*a && *a == *b) { a++; b++; }
    return *a == *b;
}

static void check(const char *what, int ok)
{
    total++;
    if (ok) passed++;
    put("c_hello: ");
    put(what);
    put(ok ? ": ok\r\n" : ": FAIL\r\n");
}

static void put_num(int n)
{
    char buf[12];
    int i = sizeof buf - 1;
    buf[i] = 0;
    do { buf[--i] = (char)('0' + n % 10); n /= 10; } while (n);
    put(&buf[i]);
}

int main(void)
{
    int i, sum = 0;
    put("Hello from C on Warpine!\r\n");
    check("a string literal at an offset", same(greetings[1], "second") && same(greetings[2], "third"));
    check("each object's own .rodata", same(hello_tag, "hello") && same(util_name(), "util"));
    check("a table of pointers in another object", same(util_word(2), "two") && same(util_word(0), "zero"));
    for (i = 0; i < 256; i++) sum += zeroes[i];
    check("zero-initialised data", sum == 0);
    check("the constructor ran", constructed == 42);
    check("one common symbol for both objects", shared_counter == util_call_count());
    check("a computed length", str_len("12345") == 5);
    put("c_hello: done ");
    put_num(passed);
    put("/");
    put_num(total);
    put("\r\n");
    return passed == total ? 0 : 1;
}

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* c_sdk_test.c - Warpine's SDK for GCC and clang (ADR-0096): os2.h, the
 * Pascal-order VIO calls and the small C runtime, from a program built by
 * an unmodified compiler. Run as `c_sdk_test alpha "beta gamma"`; prints
 * one ok / FAIL line per check and "c_sdk_test: done N/M". */
#define INCL_BASE
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <math.h>

static int passed, total;

static void check(int ok, const char *what)
{
    total++;
    if (ok) passed++;
    printf("%s %s\n", ok ? "ok" : "FAIL", what);
}

/* ── The headers ────────────────────────────────────────────────────── */
_Static_assert(sizeof(FILESTATUS3) == 24, "FILESTATUS3");
_Static_assert(offsetof(FILEFINDBUF3, cchName) == 28, "FILEFINDBUF3.cchName");
_Static_assert(offsetof(FILEFINDBUF3, achName) == 29, "FILEFINDBUF3.achName");
_Static_assert(sizeof(DATETIME) == 12, "DATETIME");
_Static_assert(sizeof(KBDKEYINFO) == 10, "KBDKEYINFO");
_Static_assert(sizeof(MOUEVENTINFO) == 10, "MOUEVENTINFO");
_Static_assert(offsetof(VIOMODEINFO, buf_addr) == 14, "VIOMODEINFO.buf_addr");
_Static_assert(offsetof(CONTEXTRECORD, ctx_SegGs) == 0x8C, "CONTEXTRECORD.ctx_SegGs");
_Static_assert(offsetof(CONTEXTRECORD, ctx_RegEip) == 0xB8, "CONTEXTRECORD.ctx_RegEip");
_Static_assert(sizeof(CONTEXTRECORD) == 0xCC, "CONTEXTRECORD");
_Static_assert(offsetof(PIB, pib_pchcmd) == 0x0C, "PIB.pib_pchcmd");
_Static_assert(offsetof(FEA2, szName) == 8 && offsetof(FEA2, cbValue) == 6, "FEA2");
_Static_assert(offsetof(GEA2, szName) == 5, "GEA2");
_Static_assert(sizeof(EAOP2) == 12 && offsetof(EAOP2, oError) == 8, "EAOP2");

/* ── Start-up: arguments and environment ────────────────────────────── */
static void test_startup(int argc, char **argv, char **envp)
{
    check(argc == 3, "argc counts the program and two arguments");
    check(argc == 3 && !strcmp(argv[1], "alpha"), "argv[1] is alpha");
    check(argc == 3 && !strcmp(argv[2], "beta gamma"), "a quoted argument stays one");
    check(argv[argc] == NULL, "argv ends with NULL");
    const char *os = getenv("OS");
    check(os && !strcmp(os, "OS2"), "getenv finds OS=OS2");
    check(getenv("WARPINE_NO_SUCH_VARIABLE") == NULL, "getenv of an unset name is NULL");
    int found = 0;
    for (char **e = envp; e && *e; e++) found |= !strcmp(*e, "OS=OS2");
    check(found, "envp holds OS=OS2");
    PTIB tib;
    PPIB pib;
    check(DosGetInfoBlocks(&tib, &pib) == NO_ERROR && pib->pib_pchcmd == argv[0],
          "argv[0] is the PIB's command line");
}

/* ── Strings and formatting ─────────────────────────────────────────── */
static void test_strings(void)
{
    char buf[64];
    check(strlen("warpine") == 7 && !strcmp("abc", "abc") && strcmp("abc", "abd") < 0,
          "strlen and strcmp");
    check(!stricmp("OS/2 Warp", "os/2 WARP") && strncmp("abcdef", "abcxyz", 3) == 0,
          "stricmp and strncmp");
    strcpy(buf, "Hello");
    strcat(buf, ", world");
    check(!strcmp(buf, "Hello, world") && strchr(buf, ',') == buf + 5 && strrchr(buf, 'o') == buf + 8,
          "strcpy, strcat, strchr, strrchr");
    check(strstr(buf, "world") == buf + 7 && strstr(buf, "warp") == NULL, "strstr");
    memmove(buf + 2, buf, 5);
    check(!memcmp(buf, "HeHello", 7), "memmove with overlap");
    char tok[] = "a,b;;c";
    char *save, *t1 = strtok_r(tok, ",;", &save), *t2 = strtok_r(NULL, ",;", &save),
         *t3 = strtok_r(NULL, ",;", &save), *t4 = strtok_r(NULL, ",;", &save);
    check(t1 && t2 && t3 && !t4 && !strcmp(t1, "a") && !strcmp(t2, "b") && !strcmp(t3, "c"),
          "strtok_r");

    snprintf(buf, sizeof buf, "[%5d|%-5d|%05d|%+d|%x|%#X|%o]", 42, 42, 42, 42, 255, 255, 8);
    check(!strcmp(buf, "[   42|42   |00042|+42|ff|0XFF|10]"), "printf integers, flags, widths");
    snprintf(buf, sizeof buf, "[%s|%.3s|%8s|%-4c|%%|%u]", "os2", "warpine", "right", 'x', 4000000000u);
    check(!strcmp(buf, "[os2|war|   right|x   |%|4000000000]"), "printf strings, chars, unsigned");
    snprintf(buf, sizeof buf, "%lld %llu %lld", -9000000000LL, 18446744073709551615ULL, LLONG_MIN);
    check(!strcmp(buf, "-9000000000 18446744073709551615 -9223372036854775808"), "printf long long");
    int n = snprintf(buf, 6, "%s", "truncated");
    check(n == 9 && !strcmp(buf, "trunc"), "snprintf truncates and counts");
    snprintf(buf, sizeof buf, "%*d|%.*d", 4, 7, 3, 7);
    check(!strcmp(buf, "   7|007"), "printf * width and precision");
}

/* ── Floating point in printf ───────────────────────────────────────── */
/* The expected text is what a correctly rounding printf prints (ties to
 * even on the exact binary value). */
static void test_floats(void)
{
    static const struct { const char *fmt; double value; const char *want; } cases[] = {
        { "%f", 3.14159, "3.141590" },
        { "%.2f", 2.675, "2.67" },
        { "%.0f", 0.5, "0" },
        { "%.0f", 1.5, "2" },
        { "%.0f", 2.5, "2" },
        { "%e", 1234.5, "1.234500e+03" },
        { "%.3e", 5e-324, "4.941e-324" },
        { "%g", 0.0001234, "0.0001234" },
        { "%g", 1e+20, "1e+20" },
        { "%g", 100000.0, "100000" },
        { "%g", 1000000.0, "1e+06" },
        { "%.17g", 0.1, "0.10000000000000001" },
        { "%.20f", 0.1, "0.10000000000000000555" },
        { "%10.3f", -1.5, "    -1.500" },
        { "%+.1e", 0.0, "+0.0e+00" },
        { "%#.3g", 1.0, "1.00" },
        { "%.3g", 100.0, "100" },
        { "%G", 1e-10, "1E-10" },
        { "%05.1f", 2.25, "002.2" },
        { "%-8.2e|", -0.000123456, "-1.23e-04|" },
        { "%.15g", 1152921504606846976.0, "1.15292150460685e+18" },
        { "%f", 1e+300, "1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160.000000" },
        { "%.10g", 1.0 / 3.0, "0.3333333333" },
        { "%g", 123456789.0, "1.23457e+08" },
        { "%.0e", 15.0, "2e+01" },
        { "%.1f", 0.05, "0.1" },
        { "%#.0f", 3.0, "3." },
        { "%g", 1e-05, "1e-05" },
        { "% .3f", 1.0005, " 1.000" },
    };
    int good = 0, n = (int)(sizeof cases / sizeof cases[0]);
    char buf[400];
    for (int i = 0; i < n; i++) {
        snprintf(buf, sizeof buf, cases[i].fmt, cases[i].value);
        if (!strcmp(buf, cases[i].want)) good++;
        else printf("  %s of %.17g: got \"%s\" want \"%s\"\n", cases[i].fmt, cases[i].value, buf, cases[i].want);
    }
    check(good == n, "printf %f / %e / %g round the exact binary value, ties to even");
    double inf = __builtin_inf(), nan = __builtin_nan("");
    snprintf(buf, sizeof buf, "%f %E %5.1f %g", inf, -inf, nan, -0.0);
    check(!strcmp(buf, "inf -INF   nan -0"), "infinities, NaN and negative zero");
    snprintf(buf, sizeof buf, "%Lf|%.2Le", (long double)1.5, (long double)12345.0);
    check(!strcmp(buf, "1.500000|1.23e+04"), "long double arguments");
}

/* ── strtod and <math.h> ────────────────────────────────────────────── */
static uint64_t bits(double d) { uint64_t b; memcpy(&b, &d, sizeof b); return b; }

/* Every expected value is what a correctly rounding strtod gives (taken
 * from Python's float()). */
static void test_strtod(void)
{
    static const struct { const char *text; uint64_t want; } cases[] = {
        { "0.1", 0x3FB999999999999AULL },
        { "1e23", 0x44B52D02C7E14AF6ULL },
        { "2.2250738585072011e-308", 0x000FFFFFFFFFFFFFULL },
        { "2.2250738585072012e-308", 0x0010000000000000ULL },
        { "4.9e-324", 0x0000000000000001ULL },
        { "2.4703282292062327e-324", 0x0000000000000000ULL },
        { "2.4703282292062328e-324", 0x0000000000000001ULL },
        { "1.7976931348623157e308", 0x7FEFFFFFFFFFFFFFULL },
        { "1.7976931348623158e308", 0x7FEFFFFFFFFFFFFFULL },
        { "1.7976931348623159e308", 0x7FF0000000000000ULL },
        { "0x1.8p1", 0x4008000000000000ULL },
        { "0x.8p-1074", 0x0000000000000000ULL },
        { "123456789012345678901234567890", 0x45F8EE90FF6C373EULL },
        { "3.14159265358979323846264338327950288", 0x400921FB54442D18ULL },
        { "9007199254740993", 0x4340000000000000ULL },
        { "9007199254740995", 0x4340000000000002ULL },
        { "1e-400", 0x0000000000000000ULL },
        { "0.000000000000000000000000000000000000000000001e45", 0x3FF0000000000000ULL },
        { "1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e-100", 0x7E031CFD3999F7B0ULL },
        { "-0.0", 0x8000000000000000ULL },
        { "10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e-400", 0x3FF0000000000000ULL },
    };
    int good = 0, n = (int)(sizeof cases / sizeof cases[0]);
    for (int i = 0; i < n; i++) {
        double v = strtod(cases[i].text, NULL);
        if (bits(v) == cases[i].want) good++;
        else printf("  strtod(%.40s): got %016llx want %016llx\n", cases[i].text,
                    (unsigned long long)bits(v), (unsigned long long)cases[i].want);
    }
    check(good == n, "strtod rounds correctly: halfway cases, subnormals, the largest double, hex");
    char *end;
    double v = strtod("  -12.5e-1xyz", &end);
    check(v == -1.25 && !strcmp(end, "xyz"), "strtod stops where the number ends");
    errno = 0;
    check(isinf(strtod("1e400", NULL)) && errno == ERANGE, "overflow is infinity with ERANGE");
    check(isinf(strtod("-Infinity", &end)) && *end == 0 && isnan(strtod("nan(x)", NULL)) && atof("2.5") == 2.5,
          "infinity, nan and atof");
    strtod("e5", &end);
    check(!strcmp(end, "e5"), "no digits: nothing read");
}

/* Within one unit in the last place of the correctly rounded value. */
static int near(double got, uint64_t want)
{
    uint64_t g = bits(got);
    return (g > want ? g - want : want - g) <= 1;
}

static void test_math(void)
{
    const struct { const char *what; double got; uint64_t want; } cases[] = {
        { "sqrt(2.0)", sqrt(2.0), 0x3FF6A09E667F3BCDULL },
        { "exp(1.0)", exp(1.0), 0x4005BF0A8B145769ULL },
        { "log(10.0)", log(10.0), 0x40026BB1BBB55516ULL },
        { "log10(2.0)", log10(2.0), 0x3FD34413509F79FFULL },
        { "log2(10.0)", log2(10.0), 0x400A934F0979A371ULL },
        { "pow(2.0, 0.5)", pow(2.0, 0.5), 0x3FF6A09E667F3BCDULL },
        { "pow(10.0, -3.0)", pow(10.0, -3.0), 0x3F50624DD2F1A9FCULL },
        { "pow(-2.0, 3.0)", pow(-2.0, 3.0), 0xC020000000000000ULL },
        { "sin(1.0)", sin(1.0), 0x3FEAED548F090CEEULL },
        { "cos(1.0)", cos(1.0), 0x3FE14A280FB5068CULL },
        { "tan(0.5)", tan(0.5), 0x3FE17B4F5BF3474AULL },
        { "atan2(1.0, 1.0) * 4", atan2(1.0, 1.0) * 4, 0x400921FB54442D18ULL },
        { "asin(0.5)", asin(0.5), 0x3FE0C152382D7366ULL },
        { "acos(0.5)", acos(0.5), 0x3FF0C152382D7366ULL },
        { "atan(1e10)", atan(1e10), 0x3FF921FB543D4DE0ULL },
        { "sinh(1.0)", sinh(1.0), 0x3FF2CD9FC44EB982ULL },
        { "cosh(1.0)", cosh(1.0), 0x3FF8B07551D9F550ULL },
        { "tanh(0.5)", tanh(0.5), 0x3FDD9353D7568AF3ULL },
        { "hypot(3.0, 4.0)", hypot(3.0, 4.0), 0x4014000000000000ULL },
        { "hypot(1e300, 1e300)", hypot(1e300, 1e300), 0x7E40E4D50F99B211ULL },
        { "fmod(10.0, 3.0)", fmod(10.0, 3.0), 0x3FF0000000000000ULL },
        { "fmod(-7.5, 2.0)", fmod(-7.5, 2.0), 0xBFF8000000000000ULL },
        { "exp(-745.0)", exp(-745.0), 0x0000000000000001ULL },
    };
    int good = 0, n = (int)(sizeof cases / sizeof cases[0]);
    for (int i = 0; i < n; i++) {
        if (near(cases[i].got, cases[i].want)) good++;
        else printf("  %s: got %.17g\n", cases[i].what, cases[i].got);
    }
    check(good == n, "math functions within an ulp of the correctly rounded value");
    int e;
    double f = frexp(48.0, &e);
    check(floor(-1.5) == -2 && ceil(-1.5) == -1 && trunc(-1.5) == -1 && round(2.5) == 3 && round(-2.5) == -3
          && f == 0.75 && e == 6 && ldexp(0.75, 6) == 48, "floor, ceil, trunc, round, frexp, ldexp");
    double ip, frac = modf(-3.25, &ip);
    errno = 0;
    check(frac == -0.25 && ip == -3 && isnan(sqrt(-1)) && errno == EDOM && fabs(-0.0) == 0 && !signbit(fabs(-0.0)),
          "modf, sqrt of a negative, fabs");
}

/* ── Numbers ────────────────────────────────────────────────────────── */
static int compare_ints(const void *a, const void *b)
{
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

static void test_numbers(void)
{
    char *end;
    check(strtol("  -1234xyz", &end, 10) == -1234 && *end == 'x', "strtol stops at the first non-digit");
    check(strtol("0x1F", NULL, 0) == 31 && strtol("017", NULL, 0) == 15 && strtoul("z", NULL, 36) == 35,
          "strtol bases 0 and 36");
    errno = 0;
    check(strtol("99999999999", NULL, 10) == LONG_MAX && errno == ERANGE, "strtol clamps with ERANGE");
    check(atoi("  77") == 77 && abs(-5) == 5 && strtoull("18446744073709551615", NULL, 10) == ULLONG_MAX,
          "atoi, abs, strtoull");
    volatile unsigned long long big = 1000000000000000007ULL;
    volatile long long neg = -1000000000000LL;
    check(big / 1000000007ULL == 999999993ULL && big % 1000000007ULL == 56ULL,
          "64-bit unsigned division and remainder");
    check(neg / 7 == -142857142857LL && neg % 7 == -1LL, "64-bit signed division and remainder");
    int values[] = { 5, -3, 99, 0, 42, -3, 7, 18, 1, 2 };
    qsort(values, 10, sizeof values[0], compare_ints);
    int sorted = 1;
    for (int i = 1; i < 10; i++) sorted &= values[i - 1] <= values[i];
    int key = 42;
    check(sorted && bsearch(&key, values, 10, sizeof key, compare_ints) == &values[8], "qsort and bsearch");
    check(isalpha('q') && !isalpha('1') && isspace('\t') && toupper('a') == 'A', "ctype");
}

/* ── The heap ───────────────────────────────────────────────────────── */
static void test_heap(void)
{
    enum { COUNT = 400 };
    unsigned char *blocks[COUNT];
    int ok = 1;
    for (int i = 0; i < COUNT; i++) {
        size_t size = 1 + (size_t)(i * 37 % 3000);
        blocks[i] = malloc(size);
        if (!blocks[i] || ((uintptr_t)blocks[i] & 15)) { ok = 0; break; }
        memset(blocks[i], i & 0xff, size);
    }
    check(ok, "400 allocations, each 16-byte aligned");
    for (int i = 0; ok && i < COUNT; i += 2) { free(blocks[i]); blocks[i] = NULL; }
    for (int i = 1; ok && i < COUNT; i += 2) {
        size_t size = 1 + (size_t)(i * 37 % 3000);
        for (size_t j = 0; j < size; j++) ok &= blocks[i][j] == (unsigned char)(i & 0xff);
    }
    check(ok, "freeing every other block leaves the rest intact");
    unsigned char *grown = realloc(blocks[1], 20000);
    ok = grown != NULL;
    for (size_t j = 0; ok && j < 1 + 37 % 3000; j++) ok &= grown[j] == 1;
    blocks[1] = grown;
    check(ok, "realloc keeps the contents");
    for (int i = 1; i < COUNT; i += 2) free(blocks[i]);
    unsigned char *large = malloc(3u << 20);
    check(large && (large[0] = 1, large[(3u << 20) - 1] = 2, large[(3u << 20) - 1] == 2),
          "a 3 MB allocation is usable");
    free(large);
    int *zeros = calloc(1000, sizeof *zeros);
    ok = zeros != NULL;
    for (int i = 0; ok && i < 1000; i++) ok &= zeros[i] == 0;
    check(ok, "calloc zeroes");
    free(zeros);
}

/* ── Streams on OS/2 files ──────────────────────────────────────────── */
static void test_files(void)
{
    const char *path = "C:\\SDKTEST.TXT";
    FILE *f = fopen(path, "w");
    check(f != NULL, "fopen for writing");
    if (!f) return;
    for (int i = 1; i <= 3; i++) fprintf(f, "line %d\n", i);
    fputs("last", f);
    check(fclose(f) == 0, "fclose");

    f = fopen(path, "r");
    char line[32];
    int ok = f != NULL;
    ok = ok && fgets(line, sizeof line, f) && !strcmp(line, "line 1\n");
    ok = ok && fgets(line, sizeof line, f) && !strcmp(line, "line 2\n");
    long at = ok ? ftell(f) : -1;
    check(ok && at == 14, "fgets and ftell");
    ok = ok && fgets(line, sizeof line, f) && fgets(line, sizeof line, f) && !strcmp(line, "last");
    check(ok && fgets(line, sizeof line, f) == NULL && feof(f), "end of file");
    ok = ok && fseek(f, at, SEEK_SET) == 0 && fgetc(f) == 'l' && ungetc('L', f) == 'L'
         && fread(line, 1, 6, f) == 6 && !memcmp(line, "Line 3", 6);
    check(ok, "fseek, fgetc, ungetc, fread");
    if (f) fclose(f);

    f = fopen(path, "a");
    if (f) { fputs("!", f); fclose(f); }
    FILESTATUS3 fs;
    check(DosQueryPathInfo(path, FIL_STANDARD, &fs, sizeof fs) == NO_ERROR && fs.cbFile == 26,
          "append mode, the size through DosQueryPathInfo");
    HFILE hf;
    ULONG action, done = 0;
    FHLOCK lock = 0;
    char back[8] = "";
    APIRET rc = DosProtectOpen(path, &hf, &action, 0, FILE_NORMAL, OPEN_ACTION_OPEN_IF_EXISTS,
                               OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL, &lock);
    rc = rc ? rc : DosProtectRead(hf, back, 4, &done, lock);
    check(rc == NO_ERROR && done == 4 && !memcmp(back, "line", 4)
          && DosProtectRead(hf, back, 4, &done, lock + 1) != NO_ERROR
          && DosProtectClose(hf, lock) == NO_ERROR,
          "DosProtectOpen / Read / Close, and a wrong lock ID is refused");
    check(remove(path) == 0 && fopen(path, "r") == NULL && errno == ENOENT,
          "remove; a missing file sets ENOENT");
}

/* ── Threads, semaphores and callbacks ──────────────────────────────── */
static HEV ready;
static HMTX lock;
static volatile int counter;
static volatile int thread_aligned;
static volatile int thread_errno_at_start = -1, thread_errno_kept = 1;

static void APIENTRY worker(ULONG arg)
{
    _Alignas(16) unsigned char probe[16];
    __asm__ volatile("" : : "r"(probe) : "memory");
    thread_aligned = ((uintptr_t)probe & 15) == 0;
    thread_errno_at_start = errno;
    errno = EDOM;
    for (ULONG i = 0; i < arg; i++) {
        DosRequestMutexSem(lock, SEM_INDEFINITE_WAIT);
        counter = counter + 1;
        DosReleaseMutexSem(lock);
    }
    if (errno != EDOM) thread_errno_kept = 0;
    DosPostEventSem(ready);
}

static void test_threads(void)
{
    check(DosCreateEventSem(NULL, &ready, 0, FALSE) == NO_ERROR
          && DosCreateMutexSem(NULL, &lock, 0, FALSE) == NO_ERROR, "semaphores created");
    TID tids[2];
    int ok = 1;
    errno = ERANGE;
    for (int i = 0; i < 2; i++)
        ok &= DosCreateThread(&tids[i], worker, 500, CREATE_READY | STACK_COMMITTED, 65536) == NO_ERROR;
    for (int i = 0; ok && i < 2; i++) ok &= DosWaitThread(&tids[i], DCWW_WAIT) == NO_ERROR;
    check(ok && counter == 1000, "two threads count to 1000 under a mutex");
    check(thread_aligned, "a thread entered on a 4-byte stack realigns for 16-byte locals");
    check(thread_errno_at_start == 0 && thread_errno_kept && errno == ERANGE,
          "errno is per thread: a new thread starts at 0, and its EDOM leaves this thread's ERANGE");
    ULONG posts = 0;
    check(DosQueryEventSem(ready, &posts) == NO_ERROR && posts == 2, "the event semaphore was posted twice");
    DosCloseEventSem(ready);
    DosCloseMutexSem(lock);
}

#define XCPT_SDK_TEST 0xE0005D4BUL
static volatile ULONG seen_exception, seen_eip;

static ULONG APIENTRY handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                              PCONTEXTRECORD ctx, PVOID dispatch)
{
    (void)reg; (void)dispatch;
    if (rep->ExceptionNum != XCPT_SDK_TEST) return XCPT_CONTINUE_SEARCH;
    seen_exception = rep->ExceptionInfo[0];
    seen_eip = ctx->ctx_RegEip;
    return XCPT_CONTINUE_EXECUTION;
}

static void test_exceptions(void)
{
    EXCEPTIONREGISTRATIONRECORD rec = { 0, handler };
    check(DosSetExceptionHandler(&rec) == NO_ERROR, "DosSetExceptionHandler");
    EXCEPTIONREPORTRECORD rep;
    memset(&rep, 0, sizeof rep);
    rep.ExceptionNum = XCPT_SDK_TEST;
    rep.cParameters = 1;
    rep.ExceptionInfo[0] = 0x5D4B;
    APIRET rc = DosRaiseException(&rep);
    check(rc == NO_ERROR && seen_exception == 0x5D4B && seen_eip != 0,
          "a raised exception reaches the handler and continues");
    check(DosUnsetExceptionHandler(&rec) == NO_ERROR, "DosUnsetExceptionHandler");
}

/* ── The Pascal-order subsystem calls ───────────────────────────────── */
/* Call VioGetCurPos by hand, as a Pascal caller does: the arguments pushed
 * left to right and nothing removed afterwards. Returns how far ESP moved
 * across the call (0 when the callee removed its 12 bytes). */
static long pascal_call_esp_delta(USHORT *row, USHORT *col)
{
    long delta;
    __asm__ volatile(
        "movl %%esp, %%esi\n\t"
        "pushl %%ebx\n\t"       /* pusRow */
        "pushl %%edi\n\t"       /* pusColumn */
        "pushl $0\n\t"          /* hvio */
        "call VioGetCurPos\n\t"
        "movl %%esp, %%ecx\n\t"
        "subl %%esi, %%ecx\n\t"
        "movl %%esi, %%esp"
        : "=c"(delta)
        : "b"(row), "D"(col)
        : "eax", "edx", "esi", "memory");
    return delta;
}

static void test_vio(void)
{
    VIOMODEINFO mode;
    mode.cb = sizeof mode;
    check(VioGetMode(&mode, 0) == NO_ERROR && mode.col >= 40 && mode.row >= 25, "VioGetMode");
    APIRET16 rc = VioSetCurPos(3, 7, 0);
    USHORT row = 0, col = 0;
    rc |= VioGetCurPos(&row, &col, 0);
    check(rc == NO_ERROR && row == 3 && col == 7, "VioSetCurPos / VioGetCurPos keep row and column apart");
    /* Printing the line above moved the cursor: put it back first. */
    row = col = 0;
    rc = VioSetCurPos(3, 7, 0);
    long delta = pascal_call_esp_delta(&row, &col);
    check(rc == NO_ERROR && delta == 0 && row == 3 && col == 7, "a hand-made Pascal call: the callee removes the arguments");
    static const char cells[] = { 'S', 0x1E, 'D', 0x2F, 'K', 0x4E };
    char back[sizeof cells];
    USHORT cb = sizeof back;
    rc = VioWrtCellStr(cells, sizeof cells, 5, 10, 0);
    rc |= VioReadCellStr(back, &cb, 5, 10, 0);
    check(rc == NO_ERROR && cb == sizeof cells && !memcmp(back, cells, sizeof cells),
          "VioWrtCellStr / VioReadCellStr round trip");
    BYTE attr = 0x17;
    rc = VioWrtCharStrAtt("ab", 2, 6, 0, &attr, 0);
    cb = 4;
    rc |= VioReadCellStr(back, &cb, 6, 0, 0);
    check(rc == NO_ERROR && back[0] == 'a' && (BYTE)back[1] == 0x17 && back[2] == 'b',
          "VioWrtCharStrAtt writes characters with the attribute");
}

int main(int argc, char **argv, char **envp)
{
#ifdef __clang__
    puts("c_sdk_test: built by clang");
#else
    puts("c_sdk_test: built by gcc");
#endif
    test_startup(argc, argv, envp);
    test_strings();
    test_floats();
    test_strtod();
    test_math();
    test_numbers();
    test_heap();
    test_files();
    test_threads();
    test_exceptions();
    test_vio();
    printf("c_sdk_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}

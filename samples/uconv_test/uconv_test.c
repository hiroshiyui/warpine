/*
 * uconv_test.c — Verify UCONV.DLL Unicode conversion APIs.
 *
 * Loads UCONV.DLL dynamically via DosLoadModule so that the test binary
 * does not require a UCONV import library at link time.
 *
 * Tests:
 *  1. DosLoadModule("UCONV") — must succeed
 *  2. DosQueryProcAddr — resolve ordinals 1–4 and 6
 *  3. UniCreateUconvObject("IBM-437") — create conversion object
 *  4. UniUconvToUcs — convert ASCII bytes to UCS-2
 *  5. UniUconvFromUcs — convert UCS-2 back to bytes (round-trip)
 *  6. UniCreateUconvObject("IBM-850") — second codepage
 *  7. UniMapCpToUcsCp(437) — returns UCS-2 name string
 *  8. UniFreeUconvObject — release both objects
 *  9. DosFreeModule("UCONV") — release DLL
 */

#define INCL_DOS
#include <os2.h>
#include <string.h>

typedef unsigned short UniChar;
typedef void *UCONV_OBJECT;

/* Function-pointer typedefs for UCONV.DLL ordinals */
typedef ULONG (APIENTRY *PFN_UniCreate)(UniChar *ucsName, UCONV_OBJECT *uobj);
typedef ULONG (APIENTRY *PFN_UniFree)(UCONV_OBJECT uobj);
typedef ULONG (APIENTRY *PFN_UniToUcs)(UCONV_OBJECT uobj,
                                        void **inbuf,    ULONG *inbytesleft,
                                        UniChar **outbuf, ULONG *outcharsleft,
                                        ULONG *nonident);
typedef ULONG (APIENTRY *PFN_UniFromUcs)(UCONV_OBJECT uobj,
                                          UniChar **inbuf,  ULONG *incharsleft,
                                          void **outbuf,    ULONG *outbytesleft,
                                          ULONG *nonident);
typedef ULONG (APIENTRY *PFN_UniMapCp)(ULONG codepage, UniChar *ucsName, ULONG n);

static ULONG dummy;

static void print(const char *msg) {
    ULONG len = 0;
    const char *p = msg;
    while (*p++) len++;
    DosWrite(1, (PVOID)msg, len, &dummy);
}

static void print_num(ULONG val) {
    char buf[12];
    int i = 11;
    buf[i--] = 0;
    if (val == 0) { buf[i--] = '0'; }
    else { while (val > 0) { buf[i--] = '0' + (val % 10); val /= 10; } }
    print(&buf[i + 1]);
}

static void check(const char *label, int pass, int *passed, int *failed) {
    print("  ");
    print(label);
    if (pass) { print(" OK\r\n"); (*passed)++; }
    else       { print(" FAILED\r\n"); (*failed)++; }
}

/* Build a null-terminated UCS-2 string from an ASCII literal */
static void ascii_to_ucs2(const char *ascii, UniChar *ucs, int maxlen) {
    int i;
    for (i = 0; ascii[i] && i < maxlen - 1; i++)
        ucs[i] = (UniChar)(unsigned char)ascii[i];
    ucs[i] = 0;
}

int main(void) {
    int passed = 0, failed = 0;
    APIRET rc;
    HMODULE hmodUconv = NULLHANDLE;
    char errBuf[80];
    PFN pfn;
    PFN_UniCreate   pUniCreate  = NULL;
    PFN_UniFree     pUniFree    = NULL;
    PFN_UniToUcs    pUniToUcs   = NULL;
    PFN_UniFromUcs  pUniFromUcs = NULL;
    PFN_UniMapCp    pUniMapCp   = NULL;
    UCONV_OBJECT obj437 = NULL, obj850 = NULL;
    UniChar ucsName[32];

    print("=== UCONV.DLL Unicode Conversion Test ===\r\n\r\n");

    /* ── Test 1: DosLoadModule ── */
    print("Test 1: DosLoadModule(\"UCONV\")\r\n");
    memset(errBuf, 0, sizeof(errBuf));
    rc = DosLoadModule(errBuf, sizeof(errBuf), "UCONV", &hmodUconv);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosLoadModule(UCONV) returns 0", rc == 0, &passed, &failed);
    if (rc != 0) {
        print("  FATAL: cannot load UCONV.DLL — aborting\r\n");
        DosExit(EXIT_PROCESS, 1);
    }

    /* ── Test 2: DosQueryProcAddr for ordinals 1,2,3,4,6 ── */
    print("\r\nTest 2: DosQueryProcAddr — resolve UCONV ordinals\r\n");
    rc = DosQueryProcAddr(hmodUconv, 1, NULL, &pfn);
    pUniCreate = (PFN_UniCreate)pfn;
    check("Ordinal 1 (UniCreateUconvObject) resolved", rc == 0 && pfn != NULL, &passed, &failed);

    rc = DosQueryProcAddr(hmodUconv, 2, NULL, &pfn);
    pUniFree = (PFN_UniFree)pfn;
    check("Ordinal 2 (UniFreeUconvObject) resolved", rc == 0 && pfn != NULL, &passed, &failed);

    rc = DosQueryProcAddr(hmodUconv, 3, NULL, &pfn);
    pUniToUcs = (PFN_UniToUcs)pfn;
    check("Ordinal 3 (UniUconvToUcs) resolved", rc == 0 && pfn != NULL, &passed, &failed);

    rc = DosQueryProcAddr(hmodUconv, 4, NULL, &pfn);
    pUniFromUcs = (PFN_UniFromUcs)pfn;
    check("Ordinal 4 (UniUconvFromUcs) resolved", rc == 0 && pfn != NULL, &passed, &failed);

    rc = DosQueryProcAddr(hmodUconv, 6, NULL, &pfn);
    pUniMapCp = (PFN_UniMapCp)pfn;
    check("Ordinal 6 (UniMapCpToUcsCp) resolved", rc == 0 && pfn != NULL, &passed, &failed);

    if (!pUniCreate || !pUniFree || !pUniToUcs || !pUniFromUcs) {
        print("  FATAL: required UCONV ordinals missing — aborting\r\n");
        DosFreeModule(hmodUconv);
        DosExit(EXIT_PROCESS, 1);
    }

    /* ── Test 3: UniCreateUconvObject("IBM-437") ── */
    print("\r\nTest 3: UniCreateUconvObject(\"IBM-437\")\r\n");
    ascii_to_ucs2("IBM-437", ucsName, 32);
    rc = pUniCreate(ucsName, &obj437);
    print("  rc="); print_num(rc); print("\r\n");
    check("UniCreateUconvObject(IBM-437) returns 0", rc == 0, &passed, &failed);
    check("obj437 handle is non-NULL", obj437 != NULL, &passed, &failed);

    /* ── Test 4: UniUconvToUcs — ASCII "Hello" → UCS-2 ── */
    print("\r\nTest 4: UniUconvToUcs — \"Hello\" (CP437) → UCS-2\r\n");
    if (obj437) {
        char inBuf[] = "Hello";
        UniChar outBuf[10];
        void *inPtr = inBuf;
        UniChar *outPtr = outBuf;
        ULONG inLeft = 5;
        ULONG outLeft = 10;
        ULONG nonIdent = 0;
        int i;

        memset(outBuf, 0, sizeof(outBuf));
        rc = pUniToUcs(obj437, &inPtr, &inLeft, &outPtr, &outLeft, &nonIdent);
        print("  rc="); print_num(rc);
        print("  inLeft="); print_num(inLeft);
        print("  outLeft="); print_num(outLeft); print("\r\n");
        check("UniUconvToUcs returns 0", rc == 0, &passed, &failed);
        check("All 5 input bytes consumed (inLeft==0)", inLeft == 0, &passed, &failed);
        check("5 UCS-2 chars produced (outLeft==5)", outLeft == 5, &passed, &failed);
        check("UCS-2 'H' == 0x0048", outBuf[0] == 0x0048, &passed, &failed);
        check("UCS-2 'e' == 0x0065", outBuf[1] == 0x0065, &passed, &failed);
        check("UCS-2 'o' == 0x006F", outBuf[4] == 0x006F, &passed, &failed);

        print("  UCS-2 values: ");
        for (i = 0; i < 5; i++) {
            print_num(outBuf[i]);
            print(" ");
        }
        print("\r\n");
    }

    /* ── Test 5: UniUconvFromUcs — UCS-2 'H','i' → CP437 bytes (round-trip) ── */
    print("\r\nTest 5: UniUconvFromUcs — UCS-2 'H','i' → CP437 bytes\r\n");
    if (obj437) {
        UniChar inBuf[3] = { 0x0048, 0x0069, 0 }; /* "Hi" */
        char outBuf[10];
        UniChar *inPtr = inBuf;
        void *outPtr = outBuf;
        ULONG inLeft = 2;
        ULONG outLeft = 10;
        ULONG nonIdent = 0;

        memset(outBuf, 0, sizeof(outBuf));
        rc = pUniFromUcs(obj437, &inPtr, &inLeft, &outPtr, &outLeft, &nonIdent);
        print("  rc="); print_num(rc);
        print("  inLeft="); print_num(inLeft);
        print("  outLeft="); print_num(outLeft); print("\r\n");
        check("UniUconvFromUcs returns 0", rc == 0, &passed, &failed);
        check("Both UCS-2 chars consumed (inLeft==0)", inLeft == 0, &passed, &failed);
        check("2 bytes produced (outLeft==8)", outLeft == 8, &passed, &failed);
        check("Output byte 0 is 'H' (0x48)", (unsigned char)outBuf[0] == 0x48, &passed, &failed);
        check("Output byte 1 is 'i' (0x69)", (unsigned char)outBuf[1] == 0x69, &passed, &failed);
    }

    /* ── Test 6: UniCreateUconvObject("IBM-850") ── */
    print("\r\nTest 6: UniCreateUconvObject(\"IBM-850\")\r\n");
    ascii_to_ucs2("IBM-850", ucsName, 32);
    rc = pUniCreate(ucsName, &obj850);
    print("  rc="); print_num(rc); print("\r\n");
    check("UniCreateUconvObject(IBM-850) returns 0", rc == 0, &passed, &failed);
    check("obj850 handle is non-NULL", obj850 != NULL, &passed, &failed);

    /* ── Test 7: UniMapCpToUcsCp ── */
    print("\r\nTest 7: UniMapCpToUcsCp(437) — get UCS-2 codepage name\r\n");
    if (pUniMapCp) {
        UniChar nameBuf[32];
        memset(nameBuf, 0, sizeof(nameBuf));
        rc = pUniMapCp(437, nameBuf, 32);
        print("  rc="); print_num(rc); print("\r\n");
        check("UniMapCpToUcsCp(437) returns 0", rc == 0, &passed, &failed);
        /* The returned UCS-2 name must be non-empty */
        check("Returned name is non-empty", nameBuf[0] != 0, &passed, &failed);
        {
            /* Print the ASCII portion of the UCS-2 name */
            int i;
            char ascii[32];
            for (i = 0; i < 31 && nameBuf[i]; i++)
                ascii[i] = (nameBuf[i] < 0x80) ? (char)nameBuf[i] : '?';
            ascii[i] = 0;
            print("  CP437 UCS-2 name (ASCII): "); print(ascii); print("\r\n");
        }
    }

    /* ── Test 8: UniFreeUconvObject ── */
    print("\r\nTest 8: UniFreeUconvObject — release both objects\r\n");
    if (obj437) {
        rc = pUniFree(obj437);
        print("  UniFreeUconvObject(obj437) rc="); print_num(rc); print("\r\n");
        check("UniFreeUconvObject(obj437) returns 0", rc == 0, &passed, &failed);
        obj437 = NULL;
    }
    if (obj850) {
        rc = pUniFree(obj850);
        print("  UniFreeUconvObject(obj850) rc="); print_num(rc); print("\r\n");
        check("UniFreeUconvObject(obj850) returns 0", rc == 0, &passed, &failed);
        obj850 = NULL;
    }

    /* ── Test 9: DosFreeModule ── */
    print("\r\nTest 9: DosFreeModule(UCONV)\r\n");
    rc = DosFreeModule(hmodUconv);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosFreeModule(UCONV) returns 0", rc == 0, &passed, &failed);

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");
    if (failed == 0) print("\r\nAll tests PASSED!\r\n");
    else             print("\r\nSome tests FAILED!\r\n");

    {
        ULONG bytesRead;
        char ch;
        print("\r\nPress ENTER to exit...\r\n");
        DosRead(0, &ch, 1, &bytesRead);
    }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}

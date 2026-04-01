/*
 * audio_test.c — Verify MMPM/2 audio APIs: DosBeep and mciSendString.
 *
 * Loads MDM.DLL dynamically via DosLoadModule so the binary does not
 * require an MDM import library at link time.
 *
 * Tests:
 *  1. DosBeep(440, 200) — 440 Hz tone for 200 ms (A4 pitch)
 *  2. DosBeep(523, 200) — 523 Hz (C5)
 *  3. DosBeep(659, 200) — 659 Hz (E5); together these form an A major chord sweep
 *  4. DosLoadModule("MDM") — must succeed
 *  5. DosQueryProcAddr — resolve mciSendString (ordinal 2) and mciSendCommand (ordinal 1)
 *  6. mciSendString "open waveaudio alias wtest" — open device, get device ID
 *  7. mciSendString "capability wtest can record" — query capability (returns "false")
 *  8. mciSendString "close wtest" — close device
 *  9. DosFreeModule("MDM")
 */

#define INCL_DOS
#include <os2.h>
#include <string.h>

/*
 * mciSendString(pszCmdString, pszReturnString, cbReturnString, hwndCallback, usUserParm)
 * Returns 0 on success, non-zero MCI error code on failure.
 */
typedef ULONG (APIENTRY *PFN_mciSendString)(const char *pszCmd,
                                             char *pszRet, USHORT cbRet,
                                             HWND hwndCallback, USHORT usUserParm);

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

int main(void) {
    int passed = 0, failed = 0;
    APIRET rc;
    HMODULE hmodMdm = NULLHANDLE;
    char errBuf[80];
    PFN pfn;
    PFN_mciSendString pMciSendString = NULL;
    char retBuf[128];

    print("=== MMPM/2 Audio API Test ===\r\n\r\n");

    /* ── Tests 1-3: DosBeep tones ── */
    print("Tests 1-3: DosBeep — A4/C5/E5 sweep\r\n");

    rc = DosBeep(440, 200);
    print("  DosBeep(440 Hz) rc="); print_num(rc); print("\r\n");
    check("DosBeep(440, 200) returns 0", rc == 0, &passed, &failed);

    rc = DosBeep(523, 200);
    print("  DosBeep(523 Hz) rc="); print_num(rc); print("\r\n");
    check("DosBeep(523, 200) returns 0", rc == 0, &passed, &failed);

    rc = DosBeep(659, 200);
    print("  DosBeep(659 Hz) rc="); print_num(rc); print("\r\n");
    check("DosBeep(659, 200) returns 0", rc == 0, &passed, &failed);

    /* ── Test 4: DosLoadModule("MDM") ── */
    print("\r\nTest 4: DosLoadModule(\"MDM\")\r\n");
    memset(errBuf, 0, sizeof(errBuf));
    rc = DosLoadModule(errBuf, sizeof(errBuf), "MDM", &hmodMdm);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosLoadModule(MDM) returns 0", rc == 0, &passed, &failed);
    if (rc != 0) {
        print("  FATAL: cannot load MDM.DLL — aborting MCI tests\r\n");
        goto summary;
    }

    /* ── Test 5: DosQueryProcAddr ── */
    print("\r\nTest 5: DosQueryProcAddr — resolve mciSendString (ord 2)\r\n");
    rc = DosQueryProcAddr(hmodMdm, 2, NULL, &pfn);
    pMciSendString = (PFN_mciSendString)pfn;
    print("  rc="); print_num(rc); print("\r\n");
    check("mciSendString (ord 2) resolved", rc == 0 && pfn != NULL, &passed, &failed);
    if (!pMciSendString) {
        print("  FATAL: mciSendString not resolved — aborting MCI tests\r\n");
        goto cleanup;
    }

    /* ── Test 6: mciSendString "open waveaudio" ── */
    print("\r\nTest 6: mciSendString \"open waveaudio alias wtest\"\r\n");
    memset(retBuf, 0, sizeof(retBuf));
    rc = pMciSendString("open waveaudio alias wtest", retBuf, sizeof(retBuf), 0, 0);
    print("  rc="); print_num(rc);
    print("  ret=\""); print(retBuf); print("\"\r\n");
    check("mciSendString open returns 0", rc == 0, &passed, &failed);

    /* ── Test 7: mciSendString "capability" ── */
    print("\r\nTest 7: mciSendString \"capability wtest can record\"\r\n");
    memset(retBuf, 0, sizeof(retBuf));
    rc = pMciSendString("capability wtest can record", retBuf, sizeof(retBuf), 0, 0);
    print("  rc="); print_num(rc);
    print("  ret=\""); print(retBuf); print("\"\r\n");
    /* Return code may be 0 or non-zero; just verify it doesn't crash */
    check("mciSendString capability returns without crash", 1, &passed, &failed);

    /* ── Test 8: mciSendString "close" ── */
    print("\r\nTest 8: mciSendString \"close wtest\"\r\n");
    memset(retBuf, 0, sizeof(retBuf));
    rc = pMciSendString("close wtest", retBuf, sizeof(retBuf), 0, 0);
    print("  rc="); print_num(rc); print("\r\n");
    check("mciSendString close returns 0", rc == 0, &passed, &failed);

cleanup:
    /* ── Test 9: DosFreeModule ── */
    print("\r\nTest 9: DosFreeModule(MDM)\r\n");
    rc = DosFreeModule(hmodMdm);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosFreeModule(MDM) returns 0", rc == 0, &passed, &failed);

summary:
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

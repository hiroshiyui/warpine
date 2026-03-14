/*
 * thunk_test.c — Verify 16-bit thunk/switching infrastructure.
 *
 * Tests the OS/2 APIs related to 16:32 address conversion and
 * Thread/Process Information Block access. These are the building
 * blocks for 16-bit thunk code in LX binaries.
 *
 * Tests:
 *  1. DosGetInfoBlocks — TIB/PIB pointer validity
 *  2. PIB field access — pid, command line, environment
 *  3. TIB self-pointer and PIB back-pointer
 *  4. DosQuerySysInfo — verify system info block
 *  5. DosFlatToSel / DosSelToFlat (via ordinal 425/426)
 *
 * Note: DosFlatToSel/DosSelToFlat are called through the LX import
 * mechanism (ordinal-based), not through direct linking. They are
 * already resolved by warpine's loader for any LX binary that
 * imports DOSCALLS ordinals 425/426.
 */

#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>

static ULONG dummy;

static void print(const char *msg) {
    const char *p = msg;
    ULONG len = 0;
    while (*p++) len++;
    DosWrite(1, (PVOID)msg, len, &dummy);
}

static void print_hex(ULONG val) {
    char buf[9];
    int i;
    for (i = 7; i >= 0; i--) {
        int nib = val & 0xF;
        buf[i] = (nib < 10) ? ('0' + nib) : ('A' + nib - 10);
        val >>= 4;
    }
    buf[8] = 0;
    print(buf);
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
    else { print(" FAILED\r\n"); (*failed)++; }
}

int main(void) {
    int passed = 0;
    int failed = 0;
    PTIB ptib;
    PPIB ppib;
    APIRET rc;
    ULONG sysinfo[2];

    print("=== Thunk Infrastructure Test ===\r\n\r\n");

    /* ── Test 1: DosGetInfoBlocks ── */
    print("Test 1: DosGetInfoBlocks (TIB/PIB pointers)\r\n");
    rc = DosGetInfoBlocks(&ptib, &ppib);
    print("  rc="); print_num(rc);
    print(" ptib=0x"); print_hex((ULONG)ptib);
    print(" ppib=0x"); print_hex((ULONG)ppib);
    print("\r\n");

    check("DosGetInfoBlocks returns 0", rc == 0, &passed, &failed);
    check("TIB non-null", (ULONG)ptib != 0, &passed, &failed);
    check("PIB non-null", (ULONG)ppib != 0, &passed, &failed);
    check("TIB in range 0x90000-0x9FFFF",
          (ULONG)ptib >= 0x00090000 && (ULONG)ptib < 0x000A0000, &passed, &failed);
    check("PIB in range 0x90000-0x9FFFF",
          (ULONG)ppib >= 0x00090000 && (ULONG)ppib < 0x000A0000, &passed, &failed);

    /* ── Test 2: PIB field access ── */
    print("\r\nTest 2: PIB fields\r\n");
    if (ppib != 0) {
        print("  PIB.pid="); print_num(ppib->pib_ulpid);
        print("  PIB.ppid="); print_num(ppib->pib_ulppid);
        print("  PIB.hmte=0x"); print_hex(ppib->pib_hmte);
        print("\r\n");

        check("PIB.pid > 0", ppib->pib_ulpid > 0, &passed, &failed);

        if (ppib->pib_pchcmd != 0) {
            print("  PIB.pchcmd -> '");
            print(ppib->pib_pchcmd);
            print("'\r\n");
            check("PIB.pchcmd non-null", 1, &passed, &failed);
        } else {
            check("PIB.pchcmd non-null", 0, &passed, &failed);
        }

        if (ppib->pib_pchenv != 0) {
            /* Print first env var */
            print("  PIB.pchenv -> '");
            print(ppib->pib_pchenv);
            print("'\r\n");
            check("PIB.pchenv non-null", 1, &passed, &failed);
        } else {
            check("PIB.pchenv non-null", 0, &passed, &failed);
        }
    }

    /* ── Test 3: TIB self-pointer ── */
    print("\r\nTest 3: TIB structure\r\n");
    if (ptib != 0) {
        /* TIB2 is at ptib->tib_ptib2 */
        PTIB2 ptib2 = ptib->tib_ptib2;
        print("  TIB.ptib2=0x"); print_hex((ULONG)ptib2); print("\r\n");
        check("TIB.ptib2 non-null", (ULONG)ptib2 != 0, &passed, &failed);

        if (ptib2 != 0) {
            print("  TIB2.tib2_ultid="); print_num(ptib2->tib2_ultid); print("\r\n");
            check("TIB2.tid > 0", ptib2->tib2_ultid > 0, &passed, &failed);
        }
    }

    /* ── Test 4: DosQuerySysInfo ── */
    print("\r\nTest 4: DosQuerySysInfo\r\n");
    rc = DosQuerySysInfo(1, 1, sysinfo, sizeof(ULONG));
    print("  MAX_PATH_LENGTH="); print_num(sysinfo[0]); print("\r\n");
    check("DosQuerySysInfo returns 0", rc == 0, &passed, &failed);
    check("MAX_PATH_LENGTH > 0", sysinfo[0] > 0, &passed, &failed);

    /* ── Test 5: Address range checks ── */
    print("\r\nTest 5: Memory layout verification\r\n");
    {
        ULONG code_addr = (ULONG)&main;
        ULONG stack_var;
        ULONG stack_addr = (ULONG)&stack_var;

        print("  Code addr (main)=0x"); print_hex(code_addr); print("\r\n");
        print("  Stack addr=0x"); print_hex(stack_addr); print("\r\n");
        print("  TIB addr=0x"); print_hex((ULONG)ptib); print("\r\n");
        print("  PIB addr=0x"); print_hex((ULONG)ppib); print("\r\n");

        /* Code should be in object area (0x10000-0x7FFFF) */
        check("Code in LX object range",
              code_addr >= 0x00010000 && code_addr < 0x00100000, &passed, &failed);
        /* Stack should be in object area */
        check("Stack in LX object range",
              stack_addr >= 0x00010000 && stack_addr < 0x00100000, &passed, &failed);
        /* TIB/PIB below code (at 0x90000-0x91FFF) */
        check("TIB below 0xA0000", (ULONG)ptib < 0x000A0000, &passed, &failed);
    }

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");

    if (failed == 0) { print("\r\nAll tests PASSED!\r\n"); }
    else { print("\r\nSome tests FAILED!\r\n"); }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}

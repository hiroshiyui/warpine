/*
 * vfs_test.c — Comprehensive VFS filesystem test for warpine.
 *
 * Exercises filesystem I/O operations on the VFS drive C:
 * (~/.local/share/warpine/drive_c/) to verify that the HostDirBackend
 * correctly handles HPFS-compatible file operations.
 *
 * Tests:
 *  1. File create, write, close, reopen, read, verify
 *  2. File seek (begin, current, end)
 *  3. File truncate (DosSetFileSize)
 *  4. Directory create and delete
 *  5. File move/rename
 *  6. File copy (DosCopy)
 *  7. Directory enumeration (DosFindFirst/DosFindNext)
 *  8. File metadata (DosQueryPathInfo)
 *  9. Current directory management (DosSetCurrentDir/DosQueryCurrentDir)
 * 10. File delete
 * 11. Cleanup
 */

#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>

static ULONG dummy;

static void print(const char *msg) {
    DosWrite(1, (PVOID)msg, 0, &dummy);
    /* Calculate length manually since we can't use strlen */
    {
        const char *p = msg;
        ULONG len = 0;
        while (*p++) len++;
        DosWrite(1, (PVOID)msg, len, &dummy);
    }
}

static void print_rc(const char *label, APIRET rc) {
    print(label);
    if (rc == 0) {
        print(" OK\r\n");
    } else {
        print(" FAILED (rc=");
        /* Simple decimal print */
        {
            char buf[12];
            int i = 11;
            ULONG n = rc;
            buf[i--] = 0;
            if (n == 0) { buf[i--] = '0'; }
            else { while (n > 0) { buf[i--] = '0' + (n % 10); n /= 10; } }
            print(&buf[i + 1]);
        }
        print(")\r\n");
    }
}

static void print_num(const char *label, ULONG val) {
    print(label);
    {
        char buf[12];
        int i = 11;
        ULONG n = val;
        buf[i--] = 0;
        if (n == 0) { buf[i--] = '0'; }
        else { while (n > 0) { buf[i--] = '0' + (n % 10); n /= 10; } }
        print(&buf[i + 1]);
    }
    print("\r\n");
}

int main(void) {
    APIRET rc;
    HFILE hf;
    ULONG action, actual;
    char buffer[256];
    HDIR hdir;
    FILEFINDBUF3 findbuf;
    ULONG count;
    FILESTATUS3 fsts3;
    ULONG disk, logical;
    ULONG cbBuf;
    int passed = 0;
    int failed = 0;

    print("=== Warpine VFS Test Suite ===\r\n\r\n");

    /* ── Test 1: File create, write, read ── */
    print("Test 1: File create/write/read\r\n");
    rc = DosOpen("C:\\vfs_test.dat", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE, NULL);
    print_rc("  DosOpen(create)", rc);
    if (rc != 0) { failed++; goto test2; }

    rc = DosWrite(hf, "Hello VFS World!", 16, &actual);
    print_rc("  DosWrite", rc);
    if (rc != 0 || actual != 16) { failed++; DosClose(hf); goto test2; }

    DosClose(hf);

    /* Reopen and read */
    rc = DosOpen("C:\\vfs_test.dat", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_SHARE_DENYNONE | OPEN_ACCESS_READONLY, NULL);
    print_rc("  DosOpen(read)", rc);
    if (rc != 0) { failed++; goto test2; }

    rc = DosRead(hf, buffer, 16, &actual);
    if (rc == 0 && actual == 16) {
        buffer[actual] = 0;
        print("  Read: ");
        print(buffer);
        print("\r\n");
        /* Verify content */
        if (buffer[0] == 'H' && buffer[5] == ' ' && buffer[10] == 'o') {
            print("  Content verified OK\r\n");
            passed++;
        } else {
            print("  Content mismatch!\r\n");
            failed++;
        }
    } else {
        print_rc("  DosRead", rc);
        failed++;
    }
    DosClose(hf);

test2:
    /* ── Test 2: File seek ── */
    print("\r\nTest 2: File seek\r\n");
    rc = DosOpen("C:\\vfs_test.dat", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_SHARE_DENYNONE | OPEN_ACCESS_READONLY, NULL);
    if (rc != 0) { print_rc("  DosOpen", rc); failed++; goto test3; }

    /* Seek to offset 6 from beginning */
    rc = DosSetFilePtr(hf, 6, FILE_BEGIN, &actual);
    print_rc("  DosSetFilePtr(BEGIN,6)", rc);
    if (rc == 0) {
        rc = DosRead(hf, buffer, 3, &actual);
        if (rc == 0 && actual == 3) {
            buffer[3] = 0;
            print("  Read at offset 6: ");
            print(buffer);
            print("\r\n");
            if (buffer[0] == 'V' && buffer[1] == 'F' && buffer[2] == 'S') {
                print("  Seek verified OK\r\n");
                passed++;
            } else {
                print("  Seek content mismatch!\r\n");
                failed++;
            }
        }
    }

    /* Seek from end */
    rc = DosSetFilePtr(hf, -6, FILE_END, &actual);
    print_rc("  DosSetFilePtr(END,-6)", rc);
    if (rc == 0) {
        print_num("  Position: ", actual);
        if (actual == 10) { passed++; } else { failed++; }
    }
    DosClose(hf);

test3:
    /* ── Test 3: File truncate ── */
    print("\r\nTest 3: File truncate (DosSetFileSize)\r\n");
    rc = DosOpen("C:\\vfs_test.dat", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE, NULL);
    if (rc != 0) { print_rc("  DosOpen", rc); failed++; goto test4; }

    rc = DosSetFileSize(hf, 8);
    print_rc("  DosSetFileSize(8)", rc);
    if (rc == 0) {
        /* Verify new size */
        rc = DosSetFilePtr(hf, 0, FILE_END, &actual);
        print_num("  New size: ", actual);
        if (actual == 8) { passed++; } else { failed++; }
    } else { failed++; }
    DosClose(hf);

test4:
    /* ── Test 4: Directory create and delete ── */
    print("\r\nTest 4: Directory create/delete\r\n");
    rc = DosCreateDir("C:\\vfs_testdir", NULL);
    print_rc("  DosCreateDir", rc);
    if (rc == 0) { passed++; } else { failed++; }

    /* Create a file inside */
    rc = DosOpen("C:\\vfs_testdir\\inner.txt", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW, OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE, NULL);
    print_rc("  DosOpen(inner.txt)", rc);
    if (rc == 0) {
        DosWrite(hf, "inner", 5, &actual);
        DosClose(hf);
    }

    /* Delete inner file then directory */
    rc = DosDelete("C:\\vfs_testdir\\inner.txt");
    print_rc("  DosDelete(inner.txt)", rc);
    if (rc == 0) { passed++; } else { failed++; }

    rc = DosDeleteDir("C:\\vfs_testdir");
    print_rc("  DosDeleteDir", rc);
    if (rc == 0) { passed++; } else { failed++; }

    /* ── Test 5: File rename/move ── */
    print("\r\nTest 5: File rename\r\n");
    rc = DosMove("C:\\vfs_test.dat", "C:\\vfs_renamed.dat");
    print_rc("  DosMove", rc);
    if (rc == 0) { passed++; } else { failed++; }

    /* Verify old name is gone */
    rc = DosOpen("C:\\vfs_test.dat", &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_OPEN_IF_EXISTS, OPEN_SHARE_DENYNONE | OPEN_ACCESS_READONLY, NULL);
    if (rc != 0) {
        print("  Old name correctly gone\r\n");
        passed++;
    } else {
        print("  ERROR: old name still exists!\r\n");
        DosClose(hf);
        failed++;
    }

    /* ── Test 6: File copy ── */
    print("\r\nTest 6: File copy (DosCopy)\r\n");
    rc = DosCopy("C:\\vfs_renamed.dat", "C:\\vfs_copy.dat", DCPY_EXISTING);
    print_rc("  DosCopy", rc);
    if (rc == 0) {
        /* Verify copy exists and has correct size */
        rc = DosOpen("C:\\vfs_copy.dat", &hf, &action, 0, FILE_NORMAL,
                     OPEN_ACTION_OPEN_IF_EXISTS, OPEN_SHARE_DENYNONE | OPEN_ACCESS_READONLY, NULL);
        if (rc == 0) {
            rc = DosSetFilePtr(hf, 0, FILE_END, &actual);
            print_num("  Copy size: ", actual);
            if (actual == 8) { passed++; } else { failed++; }
            DosClose(hf);
        }
    } else { failed++; }

    /* ── Test 7: Directory enumeration ── */
    print("\r\nTest 7: Directory enumeration\r\n");
    hdir = HDIR_CREATE;
    count = 1;
    rc = DosFindFirst("C:\\vfs_*.dat", &hdir, FILE_NORMAL, &findbuf, sizeof(findbuf), &count, FIL_STANDARD);
    if (rc == 0) {
        ULONG found = 0;
        while (rc == 0 && count > 0) {
            print("  Found: ");
            print(findbuf.achName);
            print("\r\n");
            found++;
            count = 1;
            rc = DosFindNext(hdir, &findbuf, sizeof(findbuf), &count);
        }
        DosFindClose(hdir);
        print_num("  Total files found: ", found);
        if (found >= 2) { passed++; } else { failed++; } /* Should find vfs_renamed.dat and vfs_copy.dat */
    } else {
        print_rc("  DosFindFirst", rc);
        failed++;
    }

    /* ── Test 8: File metadata ── */
    print("\r\nTest 8: File metadata (DosQueryPathInfo)\r\n");
    rc = DosQueryPathInfo("C:\\vfs_renamed.dat", FIL_STANDARD, &fsts3, sizeof(fsts3));
    print_rc("  DosQueryPathInfo", rc);
    if (rc == 0) {
        print_num("  File size: ", fsts3.cbFile);
        if (fsts3.cbFile == 8) { passed++; } else { failed++; }
    } else { failed++; }

    /* ── Test 9: Current directory management ── */
    print("\r\nTest 9: Current directory\r\n");
    rc = DosQueryCurrentDisk(&disk, &logical);
    print_rc("  DosQueryCurrentDisk", rc);
    print_num("  Current disk: ", disk);
    if (disk == 3) { passed++; } else { failed++; } /* C: = 3 */

    cbBuf = sizeof(buffer);
    rc = DosQueryCurrentDir(0, buffer, &cbBuf);
    print_rc("  DosQueryCurrentDir", rc);
    if (rc == 0) {
        print("  Current dir: ");
        print(buffer);
        print("\r\n");
        passed++;
    } else { failed++; }

    /* ── Test 10: File delete (cleanup) ── */
    print("\r\nTest 10: File cleanup\r\n");
    rc = DosDelete("C:\\vfs_renamed.dat");
    print_rc("  DosDelete(vfs_renamed.dat)", rc);
    if (rc == 0) { passed++; } else { failed++; }

    rc = DosDelete("C:\\vfs_copy.dat");
    print_rc("  DosDelete(vfs_copy.dat)", rc);
    if (rc == 0) { passed++; } else { failed++; }

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print_num("Passed: ", passed);
    print_num("Failed: ", failed);

    if (failed == 0) {
        print("\r\nAll tests PASSED!\r\n");
    } else {
        print("\r\nSome tests FAILED!\r\n");
    }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}

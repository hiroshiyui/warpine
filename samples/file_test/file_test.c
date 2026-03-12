#define INCL_DOS
#include <os2.h>

int main(void) {
    HFILE hf;
    ULONG action;
    ULONG written;
    ULONG actual;
    APIRET rc;
    char buffer[100];
    char *msg = "Warpine File Test Data";

    // 1. Create file
    rc = DosOpen("test.txt", &hf, &action, 0, 0, 0x0012, 0x0012, NULL); 
    // flags: OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS (0x12)
    // mode: OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE (0x12)
    if (rc != 0) {
        DosWrite(1, "DosOpen Create failed\r\n", 21, &written);
        DosExit(1, 1);
    }

    // 2. Write to file
    rc = DosWrite(hf, msg, 22, &actual);
    if (rc != 0) {
        DosWrite(1, "DosWrite failed\r\n", 17, &written);
        DosExit(1, 1);
    }

    // 3. Close file
    DosClose(hf);

    // 4. Open for reading
    rc = DosOpen("test.txt", &hf, &action, 0, 0, 0x0001, 0x0040, NULL);
    // flags: OPEN_ACTION_OPEN_IF_EXISTS (0x01)
    // mode: OPEN_SHARE_DENYWRITE | OPEN_ACCESS_READONLY (0x40)
    if (rc != 0) {
        DosWrite(1, "DosOpen Read failed\r\n", 19, &written);
        DosExit(1, 1);
    }

    // 5. Read from file
    rc = DosRead(hf, buffer, 22, &actual);
    if (rc == 0 && actual == 22) {
        buffer[actual] = 0;
        DosWrite(1, "Read data: ", 11, &written);
        DosWrite(1, buffer, actual, &written);
        DosWrite(1, "\r\n", 2, &written);
    }

    // 6. Close and exit
    DosClose(hf);
    DosExit(1, 0);
    return 0;
}

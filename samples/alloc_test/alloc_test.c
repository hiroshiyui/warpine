#define INCL_DOS
#include <os2.h>

int main(void) {
    PVOID pBuffer;
    APIRET rc;
    ULONG written;
    char *msg = "Data in allocated memory\r\n";
    char *buf;
    int i;

    rc = DosAllocMem(&pBuffer, 4096, 0x00000010 | 0x00000001 | 0x00000002); // PAG_COMMIT | PAG_READ | PAG_WRITE
    if (rc != 0) {
        DosWrite(1, "Alloc failed\r\n", 14, &written);
        DosExit(1, 1);
    }

    DosWrite(1, "Alloc succeeded\r\n", 17, &written);

    // Test writing to the memory
    buf = (char*)pBuffer;
    for (i = 0; i < 26; i++) {
        buf[i] = msg[i];
    }
    
    DosWrite(1, pBuffer, 26, &written);

    rc = DosFreeMem(pBuffer);
    if (rc == 0) {
        DosWrite(1, "Free succeeded\r\n", 16, &written);
    }

    DosExit(1, 0);
    return 0;
}

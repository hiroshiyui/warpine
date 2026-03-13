#define INCL_DOS
#include <os2.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    HFILE hfRead, hfWrite;
    APIRET rc;
    ULONG actual;
    char buffer[100];
    char *msg = "Pipe Test Data";

    rc = DosCreatePipe(&hfRead, &hfWrite, 4096);
    printf("DosCreatePipe rc=%lu, hRead=%lu, hWrite=%lu\n", rc, hfRead, hfWrite);

    if (rc == 0) {
        DosWrite(hfWrite, msg, strlen(msg), &actual);
        printf("Wrote to pipe: %lu bytes\n", actual);

        memset(buffer, 0, sizeof(buffer));
        DosRead(hfRead, buffer, sizeof(buffer), &actual);
        printf("Read from pipe: '%s' (%lu bytes)\n", buffer, actual);

        DosClose(hfRead);
        DosClose(hfWrite);
    }

    return 0;
}

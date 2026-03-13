#define INCL_DOS
#include <os2.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    HQUEUE hq;
    APIRET rc;
    REQUESTDATA req;
    ULONG len;
    PVOID ptr;
    BYTE priority;
    HEV hev;

    /* DosCreateQueue(PHQUEUE phq, ULONG flAttr, PCSZ pszName) */
    rc = DosCreateQueue(&hq, QUE_FIFO, "\\QUEUES\\TESTQUE");
    printf("DosCreateQueue rc=%lu, hq=%lu\n", rc, hq);

    if (rc == 0) {
        char *msg = "Queue Message";
        /* DosWriteQueue(HQUEUE hq, ULONG ulEvent, ULONG cbBuf, PVOID pbBuf, ULONG ulPriority) */
        rc = DosWriteQueue(hq, 0, strlen(msg) + 1, msg, 0);
        printf("DosWriteQueue rc=%lu\n", rc);

        /* DosReadQueue(HQUEUE hq, PREQUESTDATA pRequest, PULONG pcbBuf, PPVOID ppbuf, 
                        ULONG element, BOOL32 wait, PBYTE ppriority, HEV hsem) */
        rc = DosReadQueue(hq, &req, &len, &ptr, 0, DCWW_WAIT, &priority, 0);
        printf("DosReadQueue rc=%lu, len=%lu, msg='%s'\n", rc, len, (char*)ptr);

        DosCloseQueue(hq);
    }

    return 0;
}

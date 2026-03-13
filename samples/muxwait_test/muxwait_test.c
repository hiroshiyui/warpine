#define INCL_DOS
#include <os2.h>
#include <stdio.h>

void _Optlink poster_thread(ULONG param) {
    HEV hev = (HEV)param;
    DosSleep(1000);
    DosPostEventSem(hev);
}

int main(void) {
    HEV hev1, hev2;
    HMUX hmux;
    SEMRECORD records[2];
    ULONG user;
    APIRET rc;
    TID tid;

    DosCreateEventSem(NULL, &hev1, 0, FALSE);
    DosCreateEventSem(NULL, &hev2, 0, FALSE);

    records[0].hsemCur = (HSEM)hev1;
    records[0].ulUser = 100;
    records[1].hsemCur = (HSEM)hev2;
    records[1].ulUser = 200;

    rc = DosCreateMuxWaitSem(NULL, &hmux, 2, records, DCMW_WAIT_ANY);
    printf("DosCreateMuxWaitSem rc=%lu\n", rc);

    DosCreateThread(&tid, (PFNTHREAD)poster_thread, (ULONG)hev2, 0, 8192);

    printf("Waiting for any semaphore in mux...\n");
    rc = DosWaitMuxWaitSem(hmux, SEM_INDEFINITE_WAIT, &user);
    printf("DosWaitMuxWaitSem rc=%lu, user=%lu\n", rc, user);

    DosCloseMuxWaitSem(hmux);
    DosCloseEventSem(hev1);
    DosCloseEventSem(hev2);
    return 0;
}

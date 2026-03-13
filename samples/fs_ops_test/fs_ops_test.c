#define INCL_DOS
#include <os2.h>
#include <stdio.h>

int main(void) {
    APIRET rc;
    HFILE hf;
    ULONG action;
    ULONG written;

    /* 1. Create a directory */
    rc = DosCreateDir("testdir", NULL);
    printf("DosCreateDir rc=%lu\n", rc);

    /* 2. Create a file in it */
    rc = DosOpen("testdir/testfile.txt", &hf, &action, 0, 0, 0x0012, 0x0012, NULL);
    if (rc == 0) {
        DosWrite(hf, "FS Ops Test", 11, &written);
        DosClose(hf);
        printf("Created testdir/testfile.txt\n");
    } else {
        printf("DosOpen failed rc=%lu\n", rc);
    }

    /* 3. Move/Rename the file */
    rc = DosMove("testdir/testfile.txt", "testdir/moved.txt");
    printf("DosMove rc=%lu\n", rc);

    /* 4. Delete the file */
    rc = DosDelete("testdir/moved.txt");
    printf("DosDelete rc=%lu\n", rc);

    /* 5. Remove the directory */
    rc = DosDeleteDir("testdir");
    printf("DosDeleteDir rc=%lu\n", rc);

    return 0;
}

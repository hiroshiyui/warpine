/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * shsem_test.c - shared semaphores between processes: named and
 * DC_SEM_SHARED event and mutex semaphores used by a parent and a child
 * (two Warpine processes of one session)
 *
 * The program starts a copy of itself; the copy finds its mode in its
 * command line ("child <handle>", the second string at pib_pchcmd), never
 * in argv. The handle is the parent's unnamed DC_SEM_SHARED event, which
 * the child opens by handle.
 *
 * The parent owns a named mutex while the child tries it, then releases it
 * and posts a named event the child waits for; the child takes the mutex,
 * waits on a muxwait list of shared events, posts the unnamed event and
 * ends still owning the mutex: the parent's next request gets
 * ERROR_SEM_OWNER_DIED. A semaphore only the child had open is gone with
 * it.
 *
 * Each check is written to stdout:
 *
 *   shsem_test: <name>: ok
 *   shsem_test: <name>: FAIL got <value> want <value>
 *
 * (the child's prefixed "child:"), followed by
 * "shsem_test: done <passed>/<total>". The child's exit code is its number
 * of failures.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define READY_NAME "\\SEM32\\SHTEST\\READY"
#define GO_NAME    "\\SEM32\\SHTEST\\GO"
#define MTX_NAME   "\\SEM32\\SHTEST\\MTX"
#define LONE_NAME  "\\SEM32\\SHTEST\\CHILDONLY"
#define WAIT_MS    10000

static int n_checks = 0, n_passed = 0;
static const char *prefix = "";

static void check( const char *name, ULONG got, ULONG want )
{
    n_checks++;
    if( got == want ) {
        n_passed++;
        printf( "shsem_test: %s%s: ok\n", prefix, name );
    } else {
        printf( "shsem_test: %s%s: FAIL got %lu want %lu\n", prefix, name, got, want );
    }
    fflush( stdout );
}

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *command_args( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

static int child_main( const char *args )
{
    PTIB      ptib;
    PPIB      ppib;
    HEV       ready = 0, go = 0, lone;
    HEV       anon = (HEV)strtoul( args, NULL, 16 );
    HMTX      mtx = 0, wrong = 0;
    HMUX      mux;
    SEMRECORD recs[2];
    PID       pid;
    TID       tid;
    ULONG     count, user;

    prefix = "child: ";
    DosGetInfoBlocks( &ptib, &ppib );
    check( "DosOpenEventSem(by name)", DosOpenEventSem( (PSZ)READY_NAME, &ready ), NO_ERROR );
    check( "DosOpenEventSem(GO)", DosOpenEventSem( (PSZ)GO_NAME, &go ), NO_ERROR );
    check( "DosOpenMutexSem(by name)", DosOpenMutexSem( (PSZ)MTX_NAME, &mtx ), NO_ERROR );
    check( "DosOpenEventSem(by handle)", DosOpenEventSem( NULL, &anon ), NO_ERROR );
    check( "DosOpenMutexSem(an event's name)", DosOpenMutexSem( (PSZ)READY_NAME, &wrong ), ERROR_SEM_NOT_FOUND );
    check( "DosQueryMutexSem(the parent owns it)", DosQueryMutexSem( mtx, &pid, &tid, &count ), NO_ERROR );
    check( "-> the parent's process id", pid, ppib->pib_ulppid );
    check( "DosRequestMutexSem(owned by the parent)", DosRequestMutexSem( mtx, 0 ), ERROR_TIMEOUT );
    check( "DosPostEventSem(READY)", DosPostEventSem( ready ), NO_ERROR );
    check( "DosWaitEventSem(GO, posted by the parent)", DosWaitEventSem( go, WAIT_MS ), NO_ERROR );
    check( "DosRequestMutexSem(released by the parent)", DosRequestMutexSem( mtx, WAIT_MS ), NO_ERROR );
    check( "DosQueryMutexSem(own)", DosQueryMutexSem( mtx, &pid, &tid, &count ), NO_ERROR );
    check( "-> this process", pid, ppib->pib_ulpid );
    check( "-> count", count, 1 );
    recs[0].hsemCur = (HSEM)anon; recs[0].ulUser = 1;
    recs[1].hsemCur = (HSEM)go;   recs[1].ulUser = 2;
    check( "DosCreateMuxWaitSem(shared events)", DosCreateMuxWaitSem( NULL, &mux, 2, recs, DCMW_WAIT_ANY ), NO_ERROR );
    check( "DosWaitMuxWaitSem", DosWaitMuxWaitSem( mux, WAIT_MS, &user ), NO_ERROR );
    check( "-> the posted one", user, 2 );
    DosCloseMuxWaitSem( mux );
    check( "DosCreateEventSem(left open at the end)",
           DosCreateEventSem( (PSZ)LONE_NAME, &lone, 0, FALSE ), NO_ERROR );
    check( "DosPostEventSem(by handle)", DosPostEventSem( anon ), NO_ERROR );
    /* Ends owning the mutex and with LONE open. */
    return n_checks - n_passed;
}

static APIRET start_child( HEV anon, PID *pid )
{
    PTIB        ptib;
    PPIB        ppib;
    char        me[CCHMAXPATH], args[CCHMAXPATH + 64];
    RESULTCODES res;
    APIRET      rc;
    int         n;

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( me ), me ) != NO_ERROR ) return 1;
    memset( args, 0, sizeof( args ) );
    strcpy( args, me );
    n = strlen( me ) + 1;
    sprintf( args + n, "child %lx", (ULONG)anon );
    rc = DosExecPgm( NULL, 0, EXEC_ASYNCRESULT, (PSZ)args, NULL, &res, (PSZ)me );
    *pid = res.codeTerminate;
    return rc;
}

int main( void )
{
    const char *args = command_args();
    PTIB        ptib;
    PPIB        ppib;
    HEV         ready, go, anon, other;
    HMTX        mtx;
    PID         pid, ended, owner;
    TID         tid;
    ULONG       count;
    RESULTCODES res;

    if( strncmp( args, "child ", 6 ) == 0 ) return child_main( args + 6 );
    DosGetInfoBlocks( &ptib, &ppib );

    check( "DosCreateEventSem(named)", DosCreateEventSem( (PSZ)READY_NAME, &ready, 0, FALSE ), NO_ERROR );
    check( "DosCreateEventSem(GO)", DosCreateEventSem( (PSZ)GO_NAME, &go, 0, FALSE ), NO_ERROR );
    check( "DosCreateMutexSem(named, owned)", DosCreateMutexSem( (PSZ)MTX_NAME, &mtx, 0, TRUE ), NO_ERROR );
    check( "DosCreateEventSem(DC_SEM_SHARED)", DosCreateEventSem( NULL, &anon, DC_SEM_SHARED, FALSE ), NO_ERROR );
    check( "the same name again", DosCreateEventSem( (PSZ)GO_NAME, &other, 0, FALSE ), ERROR_DUPLICATE_NAME );
    check( "a name of another kind", DosCreateMutexSem( (PSZ)READY_NAME, (PHMTX)&other, 0, FALSE ), ERROR_DUPLICATE_NAME );
    check( "a name outside \\SEM32\\", DosCreateEventSem( (PSZ)"\\SEM\\X", &other, 0, FALSE ), ERROR_INVALID_NAME );

    check( "DosExecPgm(child)", start_child( anon, &pid ), NO_ERROR );
    check( "DosWaitEventSem(READY, posted by the child)", DosWaitEventSem( ready, WAIT_MS ), NO_ERROR );
    check( "DosQueryEventSem", DosQueryEventSem( ready, &count ), NO_ERROR );
    check( "-> one post", count, 1 );
    check( "DosReleaseMutexSem", DosReleaseMutexSem( mtx ), NO_ERROR );
    check( "DosPostEventSem(GO)", DosPostEventSem( go ), NO_ERROR );
    check( "DosWaitEventSem(by handle, posted by the child)", DosWaitEventSem( anon, WAIT_MS ), NO_ERROR );
    check( "DosWaitChild", DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &res, &ended, pid ), NO_ERROR );
    check( "-> the child's checks passed", res.codeResult, 0 );
    check( "DosRequestMutexSem(the child ended owning it)", DosRequestMutexSem( mtx, WAIT_MS ), ERROR_SEM_OWNER_DIED );
    check( "DosQueryMutexSem", DosQueryMutexSem( mtx, &owner, &tid, &count ), NO_ERROR );
    check( "-> this process owns it now", owner, ppib->pib_ulpid );
    check( "DosReleaseMutexSem(again)", DosReleaseMutexSem( mtx ), NO_ERROR );
    check( "a semaphore only the child had open is gone",
           DosOpenEventSem( (PSZ)LONE_NAME, &other ), ERROR_SEM_NOT_FOUND );
    check( "DosCloseEventSem(READY)", DosCloseEventSem( ready ), NO_ERROR );
    check( "DosCloseEventSem(twice)", DosCloseEventSem( ready ), ERROR_INVALID_HANDLE );
    other = 0;
    check( "a closed name is gone", DosOpenEventSem( (PSZ)READY_NAME, &other ), ERROR_SEM_NOT_FOUND );
    check( "DosCloseEventSem(GO)", DosCloseEventSem( go ), NO_ERROR );
    check( "DosCloseMutexSem", DosCloseMutexSem( mtx ), NO_ERROR );
    check( "DosCloseEventSem(DC_SEM_SHARED)", DosCloseEventSem( anon ), NO_ERROR );

    printf( "shsem_test: done %d/%d\n", n_passed, n_checks );
    return n_passed == n_checks ? 0 : 1;
}

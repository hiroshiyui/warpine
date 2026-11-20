/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * shmem_test.c - shared memory between processes: DosAllocSharedMem,
 * DosGetNamedSharedMem, DosGetSharedMem, DosGiveSharedMem and DosFreeMem
 * across a parent and a child (two Warpine processes of one session)
 *
 * The program starts a copy of itself; the copy finds its mode in its
 * command line ("child <giveable> <gettable>", the second string at
 * pib_pchcmd), never in argv.
 *
 * The parent makes a named block, a giveable block (given to the child
 * right after it starts, then let go by the parent) and a gettable block.
 * The child gets the named block by name and the gettable one by address,
 * reads what the parent wrote, answers in the named block, leaves a named
 * block of its own unfreed (gone when it ends) and gives the parent a
 * block it then lets go (kept by the parent).
 *
 * Each check is written to stdout:
 *
 *   shmem_test: <name>: ok
 *   shmem_test: <name>: FAIL got <value> want <value>
 *
 * (the child's prefixed "child:"), followed by
 * "shmem_test: done <passed>/<total>". The child's exit code is its number
 * of failures.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define NAMED      "\\SHAREMEM\\SHMTEST"
#define CHILD_NAME "\\SHAREMEM\\SHMCHILD"
#define MAGIC      0x51A2ED00UL
#define ACCESS     (PAG_READ | PAG_WRITE)

/* The named block's layout. */
typedef struct {
    ULONG magic;
    ULONG go;            /* the parent has given the block */
    ULONG child_named;   /* where the child found the named block */
    ULONG kept;          /* the block the child gives the parent */
    char  reply[32];
} SHARED;

static int n_checks = 0, n_passed = 0;
static const char *prefix = "";

static void check( const char *name, ULONG got, ULONG want )
{
    n_checks++;
    if( got == want ) {
        n_passed++;
        printf( "shmem_test: %s%s: ok\n", prefix, name );
    } else {
        printf( "shmem_test: %s%s: FAIL got %lu want %lu\n", prefix, name, got, want );
    }
    fflush( stdout );
}

static void check_str( const char *name, const char *got, const char *want )
{
    check( name, strcmp( got, want ) == 0, 1 );
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
    PTIB    ptib;
    PPIB    ppib;
    PVOID   named, other, kept;
    SHARED *sh;
    char   *end;
    char   *given    = (char *)strtoul( args, &end, 16 );
    char   *gettable = (char *)strtoul( end, NULL, 16 );
    int     i;

    prefix = "child: ";
    DosGetInfoBlocks( &ptib, &ppib );
    check( "DosGetNamedSharedMem", DosGetNamedSharedMem( &named, (PSZ)NAMED, ACCESS ), NO_ERROR );
    sh = (SHARED *)named;
    check( "the parent's data is there", sh->magic, MAGIC );
    sh->child_named = (ULONG)named;
    for( i = 0; i < 100 && !sh->go; i++ ) DosSleep( 100 );
    check( "the parent gave its block", sh->go, 1 );
    check_str( "a given block outlives the giver's DosFreeMem", given, "given" );
    check( "DosGetSharedMem(gettable)", DosGetSharedMem( gettable, PAG_READ ), NO_ERROR );
    check_str( "a gettable block's data", gettable, "gettable" );
    check( "DosGetSharedMem(not gettable)", DosGetSharedMem( given, PAG_READ ), ERROR_ACCESS_DENIED );
    check( "DosAllocSharedMem(left to the process end)",
           DosAllocSharedMem( &other, (PSZ)CHILD_NAME, 4096, PAG_COMMIT | ACCESS ), NO_ERROR );
    check( "DosAllocSharedMem(for the parent)",
           DosAllocSharedMem( &kept, NULL, 4096, PAG_COMMIT | ACCESS | OBJ_GIVEABLE ), NO_ERROR );
    strcpy( (char *)kept, "kept" );
    check( "DosGiveSharedMem(to the parent)", DosGiveSharedMem( kept, ppib->pib_ulppid, ACCESS ), NO_ERROR );
    sh->kept = (ULONG)kept;
    check( "DosFreeMem(given away)", DosFreeMem( kept ), NO_ERROR );
    check( "DosFreeMem(the given block)", DosFreeMem( given ), NO_ERROR );
    check( "DosFreeMem(twice)", DosFreeMem( given ), ERROR_INVALID_ADDRESS );
    strcpy( sh->reply, "child was here" );
    DosFreeMem( gettable );
    DosFreeMem( named );
    return n_checks - n_passed;
}

static APIRET start_child( PVOID given, PVOID gettable, PID *pid )
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
    sprintf( args + n, "child %lx %lx", (ULONG)given, (ULONG)gettable );
    rc = DosExecPgm( NULL, 0, EXEC_ASYNCRESULT, (PSZ)args, NULL, &res, (PSZ)me );
    /* EXEC_ASYNC(RESULT): the child's process id is in codeTerminate. */
    *pid = res.codeTerminate;
    return rc;
}

int main( void )
{
    const char *args = command_args();
    PVOID       named, given, gettable, again, other;
    SHARED     *sh;
    PID         pid, ended;
    RESULTCODES res;

    if( strncmp( args, "child ", 6 ) == 0 ) return child_main( args + 6 );

    check( "DosAllocSharedMem(named)",
           DosAllocSharedMem( &named, (PSZ)NAMED, sizeof( SHARED ), PAG_COMMIT | ACCESS ), NO_ERROR );
    sh = (SHARED *)named;
    check( "a new block is zero", sh->magic, 0 );
    sh->magic = MAGIC;
    check( "same name again",
           DosAllocSharedMem( &again, (PSZ)NAMED, 4096, PAG_COMMIT | ACCESS ), ERROR_ALREADY_EXISTS );
    check( "a name outside \\SHAREMEM\\",
           DosAllocSharedMem( &again, (PSZ)"\\PIPE\\X", 4096, PAG_COMMIT | ACCESS ), ERROR_INVALID_NAME );
    check( "DosGetNamedSharedMem(no access flag)",
           DosGetNamedSharedMem( &again, (PSZ)NAMED, PAG_COMMIT ), ERROR_INVALID_PARAMETER );
    check( "DosGetNamedSharedMem(bad name)",
           DosGetNamedSharedMem( &again, (PSZ)"\\NOTSHARE\\X", PAG_READ ), ERROR_INVALID_NAME );
    check( "DosGetNamedSharedMem(own block)", DosGetNamedSharedMem( &again, (PSZ)"\\sharemem\\shmtest", PAG_READ ), NO_ERROR );
    check( "-> at the same address", again == named, 1 );
    check( "DosAllocSharedMem(giveable)",
           DosAllocSharedMem( &given, NULL, 4096, PAG_COMMIT | ACCESS | OBJ_GIVEABLE ), NO_ERROR );
    strcpy( (char *)given, "given" );
    check( "DosAllocSharedMem(gettable)",
           DosAllocSharedMem( &gettable, NULL, 4096, PAG_COMMIT | ACCESS | OBJ_GETTABLE ), NO_ERROR );
    strcpy( (char *)gettable, "gettable" );
    check( "DosGetSharedMem(not gettable)", DosGetSharedMem( given, PAG_READ ), ERROR_ACCESS_DENIED );
    check( "DosGetSharedMem(not a block)", DosGetSharedMem( (PBYTE)gettable + 16, PAG_READ ), ERROR_INVALID_ADDRESS );

    check( "DosExecPgm(child)", start_child( given, gettable, &pid ), NO_ERROR );
    check( "DosGiveSharedMem(to the child)", DosGiveSharedMem( given, pid, ACCESS ), NO_ERROR );
    check( "DosGiveSharedMem(unknown process)", DosGiveSharedMem( given, 0x7777, PAG_READ ), ERROR_INVALID_PROCID );
    check( "DosGiveSharedMem(more than allowed)", DosGiveSharedMem( gettable, pid, PAG_READ ), ERROR_ACCESS_DENIED );
    check( "DosFreeMem(given away)", DosFreeMem( given ), NO_ERROR );
    sh->go = 1;
    check( "DosWaitChild", DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &res, &ended, pid ), NO_ERROR );
    check( "-> the child's checks passed", res.codeResult, 0 );
    check_str( "the child's answer", sh->reply, "child was here" );
    check( "the child saw the named block at the same address", sh->child_named, (ULONG)named );
    check( "a named block the child left is gone with it",
           DosGetNamedSharedMem( &other, (PSZ)CHILD_NAME, PAG_READ ), ERROR_FILE_NOT_FOUND );
    check( "a block the child gave outlives it", sh->kept != 0 && strcmp( (char *)sh->kept, "kept" ) == 0, 1 );
    check( "DosFreeMem(the kept block)", DosFreeMem( (PVOID)sh->kept ), NO_ERROR );
    check( "DosFreeMem(gettable)", DosFreeMem( gettable ), NO_ERROR );
    check( "DosFreeMem(named)", DosFreeMem( named ), NO_ERROR );
    check( "the name is free again",
           DosGetNamedSharedMem( &again, (PSZ)NAMED, PAG_READ ), ERROR_FILE_NOT_FOUND );
    check( "DosAllocSharedMem(name again)",
           DosAllocSharedMem( &again, (PSZ)NAMED, 4096, PAG_COMMIT | ACCESS ), NO_ERROR );
    check( "-> zero again", ((SHARED *)again)->magic, 0 );
    DosFreeMem( again );

    printf( "shmem_test: done %d/%d\n", n_passed, n_checks );
    return n_passed == n_checks ? 0 : 1;
}

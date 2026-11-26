/* SPDX-License-Identifier: GPL-3.0-or-later */
/* nx_test.c - execute protection (ADR-0131): code copied into memory
   without PAG_EXECUTE (a DosAllocMem page, the stack, a data object) traps
   with XCPT_ACCESS_VIOLATION / XCPT_EXECUTE_ACCESS when called; with
   PAG_EXECUTE (DosAllocMem, or DosSetMem afterwards) it runs, as the
   program's own code does. With the argument "off" (run with
   WARPINE_NX=off) every copy runs.

   The handler records the fault and returns from the copied code for it
   (EIP from the stack, EAX = NOT_RUN), so each check goes on.

   Writes "nx_test: <name>: ok" or "... FAIL got <v> want <v>", then
   "nx_test: done <p>/<t>". */
#define INCL_DOS
#define INCL_DOSERRORS
#define INCL_DOSEXCEPTIONS
#include <os2.h>
#include <stdio.h>
#include <string.h>

#define RAN     0x1234UL
#define NOT_RUN 0xDEADUL

typedef ULONG (*CODE)( void );

/* mov eax, 0x1234 ; ret */
static const unsigned char code_bytes[] = { 0xB8, 0x34, 0x12, 0x00, 0x00, 0xC3 };
static unsigned char data_copy[16] = { 0xB8, 0x34, 0x12, 0x00, 0x00, 0xC3 };

static int    passed = 0, total = 0;
static ULONG  faults = 0, fault_access = 0, fault_addr = 0;

static void check( const char *what, ULONG got, ULONG want )
{
    total++;
    if( got == want ) { passed++; printf( "nx_test: %s: ok\n", what ); }
    else printf( "nx_test: %s: FAIL got %lx want %lx\n", what, got, want );
    fflush( stdout );
}

static ULONG APIENTRY handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                               PCONTEXTRECORD ctx, PVOID unused )
{
    if( rep->ExceptionNum != XCPT_ACCESS_VIOLATION ) return XCPT_CONTINUE_SEARCH;
    faults++;
    fault_access = rep->ExceptionInfo[0];
    fault_addr = rep->ExceptionInfo[1];
    /* Return from the copied code: pop its return address. */
    ctx->ctx_RegEip = *(ULONG *)ctx->ctx_RegEsp;
    ctx->ctx_RegEsp += 4;
    ctx->ctx_RegEax = NOT_RUN;
    return XCPT_CONTINUE_EXECUTION;
}

/* Copy the code to `where` and call it: RAN, or NOT_RUN after a fault. */
static ULONG run_at( void *where )
{
    memcpy( where, code_bytes, sizeof code_bytes );
    return ( (CODE)where )();
}

static ULONG own_code( void ) { return RAN; }

/* What a call at `where` should give, and that it faulted as it should. */
static void expect( const char *what, ULONG got, int nx, void *where )
{
    char name[96];
    check( what, got, nx ? NOT_RUN : RAN );
    if( nx ) {
        sprintf( name, "-> %s: XCPT_EXECUTE_ACCESS", what );
        check( name, fault_access, XCPT_EXECUTE_ACCESS );
        sprintf( name, "-> %s: at the code", what );
        check( name, fault_addr, (ULONG)where );
    }
}

int main( int argc, char **argv )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    int     nx = !( argc > 1 && strcmp( argv[1], "off" ) == 0 );
    PVOID   page, xpage;
    ULONG   before, size, flags;
    unsigned char stack_copy[16];

    memset( &rec, 0, sizeof rec );
    rec.ExceptionHandler = handler;
    check( "DosSetExceptionHandler", DosSetExceptionHandler( &rec ), NO_ERROR );

    check( "the program's own code runs", own_code(), RAN );
    size = 4;
    check( "DosQueryMem(own code)", DosQueryMem( (PVOID)own_code, &size, &flags ), NO_ERROR );
    check( "-> PAG_EXECUTE", ( flags & PAG_EXECUTE ) != 0, 1 );

    /* Memory without PAG_EXECUTE. */
    check( "DosAllocMem(read-write)", DosAllocMem( &page, 4096, PAG_COMMIT | PAG_READ | PAG_WRITE ), NO_ERROR );
    before = faults;
    expect( "code in read-write memory", run_at( page ), nx, page );
    check( "-> faults", faults - before, nx ? 1 : 0 );

    /* Made executable afterwards. */
    check( "DosSetMem(PAG_EXECUTE)", DosSetMem( page, 4096, PAG_READ | PAG_EXECUTE ), NO_ERROR );
    before = faults;
    check( "code after DosSetMem(PAG_EXECUTE) runs", ( (CODE)page )(), RAN );
    check( "-> no fault", faults - before, 0 );
    check( "DosSetMem(no PAG_EXECUTE again)", DosSetMem( page, 4096, PAG_READ | PAG_WRITE ), NO_ERROR );
    before = faults;
    expect( "code after losing PAG_EXECUTE", ( (CODE)page )(), nx, page );
    DosFreeMem( page );

    /* Allocated executable. */
    check( "DosAllocMem(PAG_EXECUTE)",
           DosAllocMem( &xpage, 4096, PAG_COMMIT | PAG_READ | PAG_WRITE | PAG_EXECUTE ), NO_ERROR );
    check( "code in PAG_EXECUTE memory runs", run_at( xpage ), RAN );
    DosFreeMem( xpage );

    /* The stack and a data object. */
    expect( "code on the stack", run_at( stack_copy ), nx, stack_copy );
    expect( "code in a data object", ( (CODE)data_copy )(), nx, data_copy );

    DosUnsetExceptionHandler( &rec );
    printf( "nx_test: done %d/%d\n", passed, total );
    return passed == total ? 0 : 1;
}

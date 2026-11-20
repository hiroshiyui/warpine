/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * npipe_cut_test.c - a named-pipe write blocked in the middle of a message
 * is interrupted without breaking the pipe (ADR-0093).
 *
 * The message (512 KB, more than the pipe takes while nobody reads) blocks
 * its writer part way through. Then:
 *   - DosKillThread ends the thread blocked in it; the reader still gets
 *     the whole message, and a later one after it;
 *   - a signal exception interrupts thread 1 blocked in it: the handler
 *     runs, DosWrite returns with the whole message written, and the
 *     reader gets it whole.
 *
 * Each check is written as "npipe_cut_test: <name>: ok" or "... FAIL got
 * <v> want <v>", then "npipe_cut_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

#define PIPE_NAME   "\\PIPE\\WARPINE\\CUTTEST"
#define BIG         (512UL * 1024)

static unsigned char *big, *got;   /* DosAllocMem: BIG and BIG + 16 bytes */
static HPIPE hs, hc;
static volatile ULONG write_rc = 0xFFFF, write_n;
static volatile ULONG got_signal;
static int n_checks, n_passed;

static void check( const char *what, ULONG got_v, ULONG want )
{
    n_checks++;
    if( got_v == want ) { n_passed++; printf( "npipe_cut_test: %s: ok\n", what ); }
    else printf( "npipe_cut_test: %s: FAIL got %lu want %lu\n", what, got_v, want );
}

/* A connected pair: server `hs`, client `hc`, both blocking, message mode. */
static APIRET open_pipe( void )
{
    ULONG action;
    APIRET rc = DosCreateNPipe( (PSZ)PIPE_NAME, &hs, NP_ACCESS_DUPLEX,
                                NP_NOWAIT | NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1, 4096, 4096, 0 );
    if( rc != NO_ERROR ) return rc;
    DosConnectNPipe( hs );
    rc = DosOpen( (PSZ)PIPE_NAME, &hc, &action, 0, 0, OPEN_ACTION_OPEN_IF_EXISTS,
                  OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL );
    if( rc != NO_ERROR ) return rc;
    while( DosConnectNPipe( hs ) == ERROR_PIPE_NOT_CONNECTED ) DosSleep( 1 );
    DosSetNPHState( hs, NP_WAIT | NP_READMODE_MESSAGE );
    return DosSetNPHState( hc, NP_WAIT | NP_READMODE_MESSAGE );
}

/* The first offset where `got` differs from `big` (BIG when equal). */
static ULONG first_difference( void )
{
    ULONG i;
    for( i = 0; i < BIG; i++ ) {
        if( got[i] != big[i] ) {
            printf( "npipe_cut_test: first difference at %lu: %02x %02x %02x, want %02x %02x %02x\n", i,
                    got[i], got[i + 1], got[i + 2], big[i], big[i + 1], big[i + 2] );
            return i;
        }
    }
    return BIG;
}

static void close_pipe( void )
{
    DosClose( hc );
    DosDisConnectNPipe( hs );
    DosClose( hs );
}

/* Read one message from the client end: its length, 0 on error. */
static ULONG read_message( void )
{
    ULONG n = 0;
    return DosRead( hc, got, BIG + 16, &n ) == NO_ERROR ? n : 0;
}

static VOID APIENTRY big_writer( ULONG arg )
{
    ULONG n = 0;
    (void)arg;
    write_rc = DosWrite( hs, big, BIG, &n );
    write_n = n;
}

static VOID APIENTRY signaller( ULONG arg )
{
    PPIB ppib;
    PTIB ptib;
    (void)arg;
    DosSleep( 300 );
    DosGetInfoBlocks( &ptib, &ppib );
    DosSendSignalException( ppib->pib_ulpid, XCPT_SIGNAL_INTR );
}

static ULONG APIENTRY handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                               PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == XCPT_SIGNAL ) {
        got_signal = rep->ExceptionInfo[0];
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

int main( void )
{
    ULONG i, n, times, start, end;
    TID tid;
    EXCEPTIONREGISTRATIONRECORD rec;

    if( DosAllocMem( (PPVOID)&big, BIG, PAG_COMMIT | PAG_READ | PAG_WRITE ) != NO_ERROR
        || DosAllocMem( (PPVOID)&got, BIG + 16, PAG_COMMIT | PAG_READ | PAG_WRITE ) != NO_ERROR ) {
        printf( "npipe_cut_test: no memory\n" );
        return 2;
    }
    for( i = 0; i < BIG; i++ ) big[i] = (unsigned char)(i * 7 + i / 251);

    /* DosKillThread on a writer blocked in the middle of the message. */
    check( "pipe opened", open_pipe(), NO_ERROR );
    DosCreateThread( &tid, big_writer, 0, 0, 65536 );
    DosSleep( 300 );
    check( "the writer is blocked", write_rc, 0xFFFF );
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &start, sizeof start );
    check( "DosKillThread", DosKillThread( tid ), NO_ERROR );
    check( "DosWaitThread", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &end, sizeof end );
    check( "the killed writer ended within 2 s", end - start < 2000, 1 );
    n = read_message();
    check( "the reader gets the whole message", n, BIG );
    check( "-> intact", first_difference(), BIG );
    check( "a later message", DosWrite( hs, "after", 5, &n ), NO_ERROR );
    n = read_message();
    check( "-> comes whole after it", n == 5 && memcmp( got, "after", 5 ) == 0, 1 );
    close_pipe();

    /* A signal to thread 1 blocked in the middle of the message. */
    check( "pipe opened again", open_pipe(), NO_ERROR );
    rec.prev_structure = NULL;
    rec.ExceptionHandler = handler;
    DosSetExceptionHandler( &rec );
    DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );
    DosCreateThread( &tid, signaller, 0, 0, 65536 );
    n = 0;
    check( "DosWrite interrupted mid-message returns", DosWrite( hs, big, BIG, &n ), NO_ERROR );
    check( "-> with the whole message written", n, BIG );
    check( "-> the handler got XCPT_SIGNAL_INTR", got_signal, XCPT_SIGNAL_INTR );
    DosWaitThread( &tid, DCWW_WAIT );
    n = read_message();
    check( "the reader gets the whole message", n, BIG );
    check( "-> intact", first_difference(), BIG );
    DosUnsetExceptionHandler( &rec );
    close_pipe();

    printf( "npipe_cut_test: done %d/%d\n", n_passed, n_checks );
    return n_passed == n_checks ? 0 : 1;
}

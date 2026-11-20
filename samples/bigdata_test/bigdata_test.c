/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * bigdata_test.c - a program with 1.5 MB of static data owns all of it
 * (ADR-0094). Warpine once kept its GDT, IDT, TIB, PIB, thread-local page
 * and information segments at 0x80000-0xBFFFF, inside such a program's
 * data. Checks that every byte of the arrays keeps what the program and
 * the API calls put there, that the TIB and PIB lie outside them, and that
 * another thread, thread-local memory and DosGetInfoSeg leave them alone.
 *
 * Each check is written as "bigdata_test: <name>: ok" or "... FAIL got <v>
 * want <v>", then "bigdata_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

#define BIG (768UL * 1024)

static unsigned char one[BIG], two[BIG];
static int n_checks, n_passed;

APIRET16 APIENTRY16 DOS16GETINFOSEG( PSEL psel_global, PSEL psel_local );

static void check( const char *what, ULONG got, ULONG want )
{
    n_checks++;
    if( got == want ) { n_passed++; printf( "bigdata_test: %s: ok\n", what ); }
    else printf( "bigdata_test: %s: FAIL got %lx want %lx\n", what, got, want );
}

static unsigned char pattern( ULONG i, int salt )
{
    return (unsigned char)( i * 13 + i / 4093 + salt );
}

/* The first offset of `a` that does not hold the pattern (BIG: none). */
static ULONG first_bad( const unsigned char *a, int salt )
{
    ULONG i;
    for( i = 0; i < BIG; i++ ) if( a[i] != pattern( i, salt ) ) return i;
    return BIG;
}

static int inside( const void *p )
{
    const unsigned char *c = (const unsigned char *)p;
    return ( c >= one && c < one + BIG ) || ( c >= two && c < two + BIG );
}

static VOID APIENTRY other_thread( ULONG arg )
{
    (void)arg;
    DosSleep( 10 );
}

int main( void )
{
    PTIB ptib;
    PPIB ppib;
    PULONG tlm = NULL;
    HFILE hf;
    ULONG i, action, n;
    TID tid;
    SEL gsel, lsel;

    for( i = 0; i < BIG; i++ ) { one[i] = pattern( i, 1 ); two[i] = pattern( i, 2 ); }
    check( "the program's own writes stay (first array)", first_bad( one, 1 ), BIG );
    check( "the program's own writes stay (second array)", first_bad( two, 2 ), BIG );

    DosGetInfoBlocks( &ptib, &ppib );
    check( "the TIB is not in the program's data", inside( ptib ), 0 );
    check( "the PIB is not in the program's data", inside( ppib ), 0 );

    /* Another thread, thread-local memory and the information segments
       are Warpine's structures; none may land in the arrays. */
    check( "a thread runs", DosCreateThread( &tid, other_thread, 0, 0, 65536 ), NO_ERROR );
    DosWaitThread( &tid, DCWW_WAIT );
    check( "DosAllocThreadLocalMemory", DosAllocThreadLocalMemory( 1, &tlm ), NO_ERROR );
    if( tlm != NULL ) *tlm = 0x5A5A5A5A;
    check( "-> outside the program's data", inside( tlm ), 0 );
    check( "DosGetInfoSeg", DOS16GETINFOSEG( &gsel, &lsel ), NO_ERROR );
    check( "the arrays are untouched (first)", first_bad( one, 1 ), BIG );
    check( "the arrays are untouched (second)", first_bad( two, 2 ), BIG );

    /* API calls write the whole range: the second array to a file and
       back into the first. */
    DosForceDelete( (PSZ)"BIGDATA.TMP" );
    DosOpen( (PSZ)"BIGDATA.TMP", &hf, &action, 0, FILE_NORMAL,
             OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
             OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL );
    check( "DosWrite of the second array", DosWrite( hf, two, BIG, &n ) == NO_ERROR && n == BIG, 1 );
    memset( one, 0, BIG );
    DosSetFilePtr( hf, 0, FILE_BEGIN, &n );
    check( "DosRead into the first array", DosRead( hf, one, BIG, &n ) == NO_ERROR && n == BIG, 1 );
    DosClose( hf );
    DosForceDelete( (PSZ)"BIGDATA.TMP" );
    check( "-> every byte arrived", first_bad( one, 2 ), BIG );

    printf( "bigdata_test: done %d/%d\n", n_passed, n_checks );
    return n_passed == n_checks ? 0 : 1;
}

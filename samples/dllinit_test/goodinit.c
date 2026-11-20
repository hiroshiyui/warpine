/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * goodinit.c - a DLL whose initialization succeeds; its export is named
 * in the resident names table, as Open Watcom's linker does.
 */
#define INCL_DOS
#include <os2.h>
#include <string.h>

static void say( const char *s )
{
    ULONG n;
    DosWrite( 1, (PVOID)s, strlen( s ), &n );
}

/* Called by Open Watcom's DLL start-up code: nonzero = success. */
int __dll_initialize( void )
{
    say( "goodinit: initialized\r\n" );
    return( 1 );
}

int __dll_terminate( void )
{
    return( 1 );
}

int _System GoodValue( void )
{
    return( 42 );
}

/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * badstart.c - imports badinit.dll, whose initialization fails: OS/2 does
 * not start the program, so this never prints.
 */
#include <stdio.h>

int _System BadValue( void );

int main( void )
{
    printf( "badstart: started (it should not have), %d\n", BadValue() );
    return( 0 );
}

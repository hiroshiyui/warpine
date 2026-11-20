/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * badinit.c - a DLL whose initialization fails.
 */
int __dll_initialize( void )
{
    return( 0 );
}

int _System BadValue( void )
{
    return( 13 );
}

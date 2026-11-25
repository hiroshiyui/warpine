/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpdataf.c - WPDataFile and M_WPDataFile (ADR-0129): a data file's
 * object. The code is Warpine's own.
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include "wpdataf.ih"

/* The title the Reference gives the class's template. */
SOM_Scope PSZ SOMLINK wpdfM_wpclsQueryTitle( M_WPDataFile *somSelf )
{
    (void)somSelf;
    return( "Data file" );
}

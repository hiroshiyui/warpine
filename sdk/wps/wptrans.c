/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wptrans.c - WPTransient (ADR-0129): objects without a storage medium,
 * never stored. The code is Warpine's own.
 */
#define INCL_WINWORKPLACE
#include <os2.h>
#include "wptrans.ih"

/* Nothing to write: the object lasts as long as the process. */
SOM_Scope BOOL SOMLINK wptr_wpSaveImmediate( WPTransient *somSelf )
{
    (void)somSelf;
    return( TRUE );
}

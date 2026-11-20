/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * dllinit.c - Warpine's check of DLL initialization and module names
 * (ADR-0063):
 *   - goodinit.dll, imported, initializes before main;
 *   - DosLoadModule of badinit.dll fails with ERROR_INIT_ROUTINE_FAILED;
 *   - DosLoadModule by a full OS/2 path (C:\LIBS\GOODINIT.DLL, which the
 *     test puts on drive C) finds the module already loaded;
 *   - DosQueryProcAddr by name finds an export of the resident names table;
 *   - a module name leading out of drive C is not found.
 * badstart.exe imports badinit.dll and must not start at all.
 */
#define INCL_DOS
#include <os2.h>
#include <stdio.h>

int _System GoodValue( void );
typedef int _System value_fn( void );

int main( void )
{
    HMODULE  h = 0, h2 = 0;
    APIRET   rc;
    PFN      fn;
    char     fail[64];

    setvbuf( stdout, NULL, _IONBF, 0 );
    printf( "main: GoodValue() = %d\n", GoodValue() );

    rc = DosLoadModule( fail, sizeof( fail ), "BADINIT", &h );
    printf( "DosLoadModule(BADINIT) = %lu, handle %s\n", rc, h ? "set" : "0" );

    rc = DosLoadModule( fail, sizeof( fail ), "GOODINIT", &h );
    rc = DosLoadModule( fail, sizeof( fail ), "C:\\LIBS\\GOODINIT.DLL", &h2 );
    printf( "DosLoadModule(C:\\LIBS\\GOODINIT.DLL) = %lu, %s\n", rc,
            h2 && h2 == h ? "the loaded module" : "another module" );
    rc = DosQueryProcAddr( h2, 0, "GoodValue", &fn );
    printf( "DosQueryProcAddr(GoodValue) = %lu, value %d\n", rc, rc == 0 ? ((value_fn *)fn)() : -1 );

    rc = DosLoadModule( fail, sizeof( fail ), "C:\\..\\..\\..\\..\\etc\\GOODINIT.DLL", &h2 );
    printf( "DosLoadModule(outside drive C) = %lu\n", rc );
    rc = DosLoadModule( fail, sizeof( fail ), "..\\GOODINIT", &h2 );
    printf( "DosLoadModule(..\\GOODINIT) = %lu\n", rc );
    return( 0 );
}

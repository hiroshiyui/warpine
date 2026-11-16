/*
 * bmpres.c - a resource DLL for pm_bitmap_test
 *
 * The DLL only carries resources (bmpres.rc): an RT_BITMAP with the same
 * id as the executable's, so pm_bitmap_test can check that
 * GpiLoadBitmap( hps, hmod, id, ... ) loads from the module it names.
 * One export keeps the linker happy.
 */
#include <os2.h>

unsigned long APIENTRY BmpResVersion( void )
{
    return 1;
}

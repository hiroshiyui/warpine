/* viowrap.c - 32-bit VIO/KBD import pragmas for 4OS2 under Warpine.
 * Ordinals are those of Open Watcom's os2386.lib (VIO16* / KBD16*,
 * doc/os2_ordinals.md). Nothing in 4OS2 references the _WP_* symbols today:
 * its calls go through h/bsesub.h's _Pascal declarations instead. */
#pragma import(_WP_VIOGETMODE, "VIOCALLS", 21)
#pragma import(_WP_VIOGETCURPOS, "VIOCALLS", 9)
#pragma import(_WP_VIOSETCURPOS, "VIOCALLS", 15)
#pragma import(_WP_VIOWRTTTY, "VIOCALLS", 19)
#pragma import(_WP_VIOSCROLLUP, "VIOCALLS", 7)
#pragma import(_WP_VIOSCROLLDN, "VIOCALLS", 47)
#pragma import(_WP_VIOWRTCHARSTRATT, "VIOCALLS", 48)
#pragma import(_WP_VIOWRTNCELL, "VIOCALLS", 52)
#pragma import(_WP_VIOWRTNATTR, "VIOCALLS", 26)
#pragma import(_WP_VIOREADCELLSTR, "VIOCALLS", 24)
#pragma import(_WP_VIOSETCURTYPE, "VIOCALLS", 32)
#pragma import(_WP_VIOSETSTATE, "VIOCALLS", 51)
#pragma import(_WP_VIOGETCONFIG, "VIOCALLS", 46)
#pragma import(_WP_VIOSETANSI, "VIOCALLS", 5)
#pragma import(_WP_VIOGETANSI, "VIOCALLS", 3)
#pragma import(_WP_VIOSETCP, "VIOCALLS", 42)
#pragma import(_WP_KBDCHARIN, "KBDCALLS", 4)
#pragma import(_WP_KBDGETSTATUS, "KBDCALLS", 10)
#pragma import(_WP_KBDSTRINGIN, "KBDCALLS", 9)

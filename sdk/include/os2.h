/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2.h - Warpine's OS/2 API headers for GCC and clang (ADR-0096).
 *
 * Written from IBM's public Control Program Programming Reference, not
 * copied from any toolkit. As with the familiar headers, a program
 * defines INCL_* names before including this file to choose what it
 * declares:
 *
 *   INCL_BASE        INCL_DOS, INCL_SUB and INCL_DOSERRORS
 *   INCL_DOS         every Dos* call; INCL_DOSFILEMGR, INCL_DOSPROCESS,
 *                    INCL_DOSMEMMGR ... each select the same header
 *   INCL_DOSERRORS   the ERROR_* codes
 *   INCL_SUB         INCL_VIO, INCL_KBD and INCL_MOU
 *   INCL_PM          INCL_WIN and INCL_GPI
 *   INCL_WIN         the window manager; INCL_WINWINDOWMGR,
 *                    INCL_WINMESSAGEMGR, INCL_WINDIALOGS ... select it too
 *   INCL_GPI         graphics; INCL_GPIPRIMITIVES, INCL_GPILCIDS ... too
 *
 * Every declared function is an import lx-link resolves through
 * targets/os2api.def; `gen_api check-headers` keeps the two in step.
 * The Presentation Manager's declarations are a subset (ADR-0097). */
#ifndef WARPINE_OS2_H
#define WARPINE_OS2_H

#include <os2/os2def.h>

#ifdef INCL_BASE
#  define INCL_DOS
#  define INCL_SUB
#  define INCL_DOSERRORS
#endif

#if defined(INCL_DOS) || defined(INCL_DOSFILEMGR) || defined(INCL_DOSPROCESS) \
    || defined(INCL_DOSMEMMGR) || defined(INCL_DOSSEMAPHORES) \
    || defined(INCL_DOSDATETIME) || defined(INCL_DOSMODULEMGR) \
    || defined(INCL_DOSRESOURCES) || defined(INCL_DOSNMPIPES) \
    || defined(INCL_DOSQUEUES) || defined(INCL_DOSMISC) \
    || defined(INCL_DOSEXCEPTIONS) || defined(INCL_DOSDEVICES) \
    || defined(INCL_DOSNLS)
#  include <os2/dos.h>
#endif

#if defined(INCL_DOSERRORS) || defined(INCL_ERRORS)
#  include <os2/errors.h>
#endif

#ifdef INCL_SUB
#  define INCL_VIO
#  define INCL_KBD
#  define INCL_MOU
#endif

#if defined(INCL_VIO) || defined(INCL_KBD) || defined(INCL_MOU)
#  include <os2/sub.h>
#endif

#ifdef INCL_PM
#  define INCL_WIN
#  define INCL_GPI
#endif
#if defined(INCL_WINWINDOWMGR) || defined(INCL_WINMESSAGEMGR) || defined(INCL_WININPUT) \
    || defined(INCL_WINDIALOGS) || defined(INCL_WINFRAMEMGR) || defined(INCL_WINSYS) \
    || defined(INCL_WINTIMER) || defined(INCL_WINPOINTERS) || defined(INCL_WINMENUS) \
    || defined(INCL_WINBUTTONS) || defined(INCL_WINENTRYFIELDS) || defined(INCL_WINLISTBOXES) \
    || defined(INCL_WINSTATICS) || defined(INCL_WINSCROLLBARS) || defined(INCL_WINRECTANGLES) \
    || defined(INCL_WINACCELERATORS) || defined(INCL_WINCLIPBOARD) || defined(INCL_WINATOM) \
    || defined(INCL_WINERRORS) || defined(INCL_WINCOUNTRY) || defined(INCL_WINPROGRAMLIST)
#  ifndef INCL_WIN
#    define INCL_WIN
#  endif
#endif
#if defined(INCL_GPIPRIMITIVES) || defined(INCL_GPILCIDS) || defined(INCL_GPIBITMAPS) \
    || defined(INCL_GPIREGIONS) || defined(INCL_GPICONTROL) || defined(INCL_GPIPATHS) \
    || defined(INCL_GPILOGCOLORTABLE)
#  ifndef INCL_GPI
#    define INCL_GPI
#  endif
#endif

#if defined(INCL_WIN) || defined(INCL_GPI)
#  include <os2/pm.h>
#endif

#endif /* WARPINE_OS2_H */

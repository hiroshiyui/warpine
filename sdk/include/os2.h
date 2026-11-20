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
 *
 * Every declared function is an import lx-link resolves through
 * targets/os2api.def; `gen_api check-headers` keeps the two in step.
 * The Presentation Manager is not declared yet. */
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

#endif /* WARPINE_OS2_H */

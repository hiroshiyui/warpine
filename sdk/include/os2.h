/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2.h - Warpine's OS/2 API headers for GCC and clang (ADR-0096).
 *
 * Written from IBM's public Control Program and PM Programming
 * References, not copied from any toolkit. It declares everything: the
 * Control Program (os2/dos.h, os2/errors.h), the VIO / KBD / MOU
 * subsystems (os2/sub.h) and the Presentation Manager (os2/pm.h and the
 * module headers it includes). A program's INCL_* definitions are
 * accepted and need not select anything: programs written for headers
 * that do select by them build unchanged (ADR-0101).
 *
 * Every declared function is an import lx-link resolves through
 * targets/os2api.def; `gen_api check-headers` keeps the two in step.
 * The Presentation Manager's declarations are a subset (ADR-0097). */
#ifndef WARPINE_OS2_H
#define WARPINE_OS2_H

#include <os2/os2def.h>
#include <os2/dos.h>
#include <os2/errors.h>
#include <os2/sub.h>
#include <os2/pm.h>

#endif /* WARPINE_OS2_H */

/* viodirect.h - Included AFTER os2.h via source file modification.
   Redefines APIENTRY16 to eliminate 16-bit thunks for warpine. */
#ifndef _VIODIRECT_H
#define _VIODIRECT_H

/* Override APIENTRY16 after os2def.h has defined it */
#undef APIENTRY16
#define APIENTRY16 _System

#undef PASCAL16
#define PASCAL16 _System

#endif

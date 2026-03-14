/* viodirect.h - Included AFTER os2.h via source file modification.
   Redefines APIENTRY16 to eliminate 16-bit thunks for warpine.
   Note: _Seg16 is NOT overridden here — it is needed by
   DosGetInfoSeg for selector-to-flat pointer conversion. */
#ifndef _VIODIRECT_H
#define _VIODIRECT_H

#undef APIENTRY16
#define APIENTRY16 _System

#undef PASCAL16
#define PASCAL16 _System

#endif

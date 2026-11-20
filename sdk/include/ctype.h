/* SPDX-License-Identifier: GPL-3.0-or-later */
/* ctype.h - Warpine's small C runtime (ADR-0096): the "C" locale only. */
#ifndef WARPINE_CTYPE_H
#define WARPINE_CTYPE_H

static __inline__ int isdigit(int c)  { return c >= '0' && c <= '9'; }
static __inline__ int isupper(int c)  { return c >= 'A' && c <= 'Z'; }
static __inline__ int islower(int c)  { return c >= 'a' && c <= 'z'; }
static __inline__ int isalpha(int c)  { return isupper(c) || islower(c); }
static __inline__ int isalnum(int c)  { return isalpha(c) || isdigit(c); }
static __inline__ int isxdigit(int c) { return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }
static __inline__ int isspace(int c)  { return c == ' ' || (c >= '\t' && c <= '\r'); }
static __inline__ int isblank(int c)  { return c == ' ' || c == '\t'; }
static __inline__ int iscntrl(int c)  { return (c >= 0 && c < 0x20) || c == 0x7f; }
static __inline__ int isprint(int c)  { return c >= 0x20 && c < 0x7f; }
static __inline__ int isgraph(int c)  { return c > 0x20 && c < 0x7f; }
static __inline__ int ispunct(int c)  { return isgraph(c) && !isalnum(c); }
static __inline__ int toupper(int c)  { return islower(c) ? c - 'a' + 'A' : c; }
static __inline__ int tolower(int c)  { return isupper(c) ? c - 'A' + 'a' : c; }

#endif

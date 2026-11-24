/* SPDX-License-Identifier: GPL-3.0-or-later */
/* resolv.h - the resolver's name packing (OS/2 TCP/IP 4.0, TCP32DLL), for
 * GCC and clang. */
#ifndef WARPINE_RESOLV_H
#define WARPINE_RESOLV_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

int APIENTRY dn_comp(u_char *exp_dn, u_char *comp_dn, int length, u_char **dnptrs, u_char **lastdnptr);
int APIENTRY dn_expand(u_char *msg, u_char *eomorig, u_char *comp_dn, u_char *exp_dn, int length);
int APIENTRY dn_skipname(u_char *comp_dn, u_char *eom);
u_short APIENTRY _getshort(u_char *msgp);
u_long APIENTRY _getlong(u_char *msgp);
void APIENTRY putshort(u_short s, u_char *msgp);
void APIENTRY putlong(u_long l, u_char *msgp);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_RESOLV_H */

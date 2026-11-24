/* SPDX-License-Identifier: GPL-3.0-or-later */
/* arpa/inet.h - address conversion (OS/2 TCP/IP 4.0, TCP32DLL), for GCC
 * and clang. */
#ifndef WARPINE_ARPA_INET_H
#define WARPINE_ARPA_INET_H

#include <netinet/in.h>

#ifdef __cplusplus
extern "C" {
#endif

u_long APIENTRY inet_addr(char *cp);
u_long APIENTRY inet_network(char *cp);
char * APIENTRY inet_ntoa(struct in_addr in);
u_long APIENTRY inet_lnaof(struct in_addr in);
u_long APIENTRY inet_netof(struct in_addr in);

/* inet_makeaddr returns its in_addr in EAX, as OS/2's _System returns a
 * 4-byte structure; GCC's i386 code returns structures in memory, so the
 * import is declared as returning the address itself. */
u_long APIENTRY _wrp_inet_makeaddr(u_long net, u_long lna) OS2_IMPORT(inet_makeaddr);
/* The inline function gets a symbol of its own: a compiler that emits it
 * out of line must not give it the import's name (os2/sub.h does the same). */
static __inline__ struct in_addr inet_makeaddr(u_long net, u_long lna) OS2_IMPORT(_wrp_inet_makeaddr_inline);
static __inline__ struct in_addr inet_makeaddr(u_long net, u_long lna)
{
    struct in_addr a;
    a.s_addr = _wrp_inet_makeaddr(net, lna);
    return a;
}

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_ARPA_INET_H */

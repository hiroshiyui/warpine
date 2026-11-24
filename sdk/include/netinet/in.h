/* SPDX-License-Identifier: GPL-3.0-or-later */
/* netinet/in.h - Internet addresses and byte order (OS/2 TCP/IP 4.0,
 * TCP32DLL), for GCC and clang. */
#ifndef WARPINE_NETINET_IN_H
#define WARPINE_NETINET_IN_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define IPPROTO_IP    0
#define IPPROTO_ICMP  1
#define IPPROTO_TCP   6
#define IPPROTO_UDP   17

#define INADDR_ANY        0x00000000UL
#define INADDR_BROADCAST  0xffffffffUL
#define INADDR_LOOPBACK   0x7f000001UL
#define INADDR_NONE       0xffffffffUL

struct in_addr {
    u_long s_addr;
};

struct sockaddr_in {
    short          sin_family;
    u_short        sin_port;
    struct in_addr sin_addr;
    char           sin_zero[8];
};

u_long APIENTRY htonl(u_long x);
u_short APIENTRY htons(u_short x);
u_long APIENTRY ntohl(u_long x);
u_short APIENTRY ntohs(u_short x);
u_short APIENTRY bswap(u_short x);
u_long APIENTRY lswap(u_long x);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_NETINET_IN_H */

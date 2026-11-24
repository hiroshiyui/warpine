/* SPDX-License-Identifier: GPL-3.0-or-later */
/* netdb.h - host, service and protocol lookup (OS/2 TCP/IP 4.0,
 * TCP32DLL), for GCC and clang. The structures returned are the
 * process's, overwritten by the next call. */
#ifndef WARPINE_NETDB_H
#define WARPINE_NETDB_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct hostent {
    char  *h_name;
    char **h_aliases;
    int    h_addrtype;
    int    h_length;
    char **h_addr_list;
#define h_addr h_addr_list[0]
};

struct servent {
    char  *s_name;
    char **s_aliases;
    int    s_port;
    char  *s_proto;
};

struct protoent {
    char  *p_name;
    char **p_aliases;
    int    p_proto;
};

struct hostent * APIENTRY gethostbyname(char *name);
struct hostent * APIENTRY gethostbyaddr(char *addr, int len, int type);
int APIENTRY gethostname(char *name, int namelen);
struct servent * APIENTRY getservbyname(char *name, char *proto);
struct servent * APIENTRY getservbyport(int port, char *proto);
struct protoent * APIENTRY getprotobyname(char *name);
struct protoent * APIENTRY getprotobynumber(int proto);
void APIENTRY setprotoent(int stayopen);
struct protoent * APIENTRY getprotoent(void);
void APIENTRY endprotoent(void);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_NETDB_H */

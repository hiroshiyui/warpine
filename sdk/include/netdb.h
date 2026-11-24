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
void APIENTRY setservent(int stayopen);
struct servent * APIENTRY getservent(void);
void APIENTRY endservent(void);

/* The resolver's last error. */
int APIENTRY tcp_h_errno(void);
#define h_errno (tcp_h_errno())
#define HOST_NOT_FOUND 1
#define TRY_AGAIN      2
#define NO_RECOVERY    3
#define NO_DATA        4

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_NETDB_H */

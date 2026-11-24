/* SPDX-License-Identifier: GPL-3.0-or-later */
/* sys/socket.h - the socket calls of OS/2's TCP/IP 4.0 (SO32DLL; IBM's
 * TCP/IP Programming Reference), for GCC and clang. BSD 4.3 layouts: a
 * sockaddr begins with a 16-bit family. Option values are OS/2's BSD
 * numbering, which Warpine translates (tcpip.rs). */
#ifndef WARPINE_SYS_SOCKET_H
#define WARPINE_SYS_SOCKET_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SOCK_STREAM    1
#define SOCK_DGRAM     2
#define SOCK_RAW       3

#define AF_UNSPEC      0
#define AF_INET        2
#define PF_UNSPEC      AF_UNSPEC
#define PF_INET        AF_INET

#define SOL_SOCKET     0xffff

#define SO_DEBUG       0x0001
#define SO_ACCEPTCONN  0x0002
#define SO_REUSEADDR   0x0004
#define SO_KEEPALIVE   0x0008
#define SO_DONTROUTE   0x0010
#define SO_BROADCAST   0x0020
#define SO_LINGER      0x0080
#define SO_OOBINLINE   0x0100
#define SO_SNDBUF      0x1001
#define SO_RCVBUF      0x1002
#define SO_SNDLOWAT    0x1003
#define SO_RCVLOWAT    0x1004
#define SO_SNDTIMEO    0x1005
#define SO_RCVTIMEO    0x1006
#define SO_ERROR       0x1007
#define SO_TYPE        0x1008

#define MSG_OOB        0x1
#define MSG_PEEK       0x2
#define MSG_DONTROUTE  0x4

#define SOMAXCONN      5

struct sockaddr {
    u_short sa_family;
    char    sa_data[14];
};

struct linger {
    int l_onoff;
    int l_linger;
};

int APIENTRY accept(int s, struct sockaddr *name, int *namelen);
int APIENTRY bind(int s, struct sockaddr *name, int namelen);
int APIENTRY connect(int s, struct sockaddr *name, int namelen);
u_long APIENTRY gethostid(void);
int APIENTRY getpeername(int s, struct sockaddr *name, int *namelen);
int APIENTRY getsockname(int s, struct sockaddr *name, int *namelen);
int APIENTRY getsockopt(int s, int level, int optname, char *optval, int *optlen);
int APIENTRY listen(int s, int backlog);
int APIENTRY recv(int s, char *buf, int len, int flags);
int APIENTRY recvfrom(int s, char *buf, int len, int flags, struct sockaddr *name, int *namelen);
int APIENTRY select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds, struct timeval *timeout);
int APIENTRY send(int s, char *msg, int len, int flags);
int APIENTRY sendto(int s, char *msg, int len, int flags, struct sockaddr *to, int tolen);
int APIENTRY setsockopt(int s, int level, int optname, char *optval, int optlen);
int APIENTRY shutdown(int s, int how);
int APIENTRY socket(int domain, int type, int protocol);
int APIENTRY soclose(int s);
int APIENTRY sock_errno(void);
void APIENTRY psock_errno(char *s);
int APIENTRY sock_init(void);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_SYS_SOCKET_H */

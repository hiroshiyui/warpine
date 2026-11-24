/* SPDX-License-Identifier: GPL-3.0-or-later */
/* types.h - the TCP/IP types of OS/2's TCP/IP 4.0 programming interface
 * (IBM's TCP/IP Programming Reference), for GCC and clang. A socket
 * program includes it before <sys/socket.h>. The socket calls are in
 * SO32DLL and TCP32DLL; Warpine's ordinals for them are its own, linked by
 * name (ADR-0123). */
#ifndef WARPINE_TCP_TYPES_H
#define WARPINE_TCP_TYPES_H

#include <os2/os2def.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned char  u_char;
typedef unsigned short u_short;
typedef unsigned int   u_int;
typedef unsigned long  u_long;
typedef char          *caddr_t;

/* select's set: a count and the socket numbers themselves, not a bit mask
 * (OS/2 TCP/IP), packed to 4 bytes. */
#define FD_SETSIZE 64

#pragma pack(push, 4)
typedef struct fd_set {
    u_short fd_count;
    int     fd_array[FD_SETSIZE];
} fd_set;
#pragma pack(pop)

#define FD_ZERO(set) (((fd_set *)(set))->fd_count = 0)
#define FD_SET(fd, set) do { \
        if (((fd_set *)(set))->fd_count < FD_SETSIZE) \
            ((fd_set *)(set))->fd_array[((fd_set *)(set))->fd_count++] = (int)(fd); \
    } while (0)

static __inline__ int FD_ISSET(int fd, const fd_set *set)
{
    u_short i;
    for (i = 0; i < set->fd_count; i++)
        if (set->fd_array[i] == fd) return 1;
    return 0;
}

static __inline__ void FD_CLR(int fd, fd_set *set)
{
    u_short i;
    for (i = 0; i < set->fd_count; i++) {
        if (set->fd_array[i] == fd) {
            for (; i + 1 < set->fd_count; i++) set->fd_array[i] = set->fd_array[i + 1];
            set->fd_count--;
            return;
        }
    }
}

struct timeval {
    long tv_sec;
    long tv_usec;
};

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_TCP_TYPES_H */

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* sys/ioctl.h - socket ioctl requests (OS/2 TCP/IP 4.0, SO32DLL), for GCC
 * and clang. */
#ifndef WARPINE_SYS_IOCTL_H
#define WARPINE_SYS_IOCTL_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FIONREAD   0x4004667fUL
#define FIONBIO    0x8004667eUL
#define SIOCATMARK 0x40047307UL

int APIENTRY ioctl(int s, int cmd, char *data, int lendata);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_SYS_IOCTL_H */

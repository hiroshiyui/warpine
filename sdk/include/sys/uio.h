/* SPDX-License-Identifier: GPL-3.0-or-later */
/* sys/uio.h - scatter / gather socket I/O (OS/2 TCP/IP 4.0, SO32DLL), for
 * GCC and clang. */
#ifndef WARPINE_SYS_UIO_H
#define WARPINE_SYS_UIO_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct iovec {
    caddr_t iov_base;
    int     iov_len;
};

int APIENTRY readv(int s, struct iovec *iov, int iovcnt);
int APIENTRY writev(int s, struct iovec *iov, int iovcnt);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_SYS_UIO_H */

/* SPDX-License-Identifier: GPL-3.0-or-later */
/* errno.h - Warpine's small C runtime (ADR-0096). One errno for the
 * process: not per thread yet. */
#ifndef WARPINE_ERRNO_H
#define WARPINE_ERRNO_H

extern int errno;

#define EPERM    1
#define ENOENT   2
#define EIO      5
#define EBADF    9
#define EAGAIN  11
#define ENOMEM  12
#define EACCES  13
#define EEXIST  17
#define ENOTDIR 20
#define EISDIR  21
#define EINVAL  22
#define EMFILE  24
#define ENOSPC  28
#define ESPIPE  29
#define EPIPE   32
#define EDOM    33
#define ERANGE  34

#endif

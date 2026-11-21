/* SPDX-License-Identifier: GPL-3.0-or-later */
/* gdb_point.h - the structure c_gdb_test and gdbdll share */
#ifndef GDB_POINT_H
#define GDB_POINT_H

struct point {
    int x, y;
};

int scale_point(struct point *p, int k);

#endif

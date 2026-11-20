#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Write the OS/2 pointer files of pm_pointer_test.

The format is the documented OS/2 bitmap file format (OS/2 Warp 4 Toolkit
<pmgpi.h>, Toolkit "Bitmap File Format", PM Guide "Mouse Pointers and
Icons"); no other tool is used:

  BITMAPFILEHEADER   USHORT usType ("PT" pointer, "CP" colour pointer),
                     ULONG cbSize, SHORT xHotspot, SHORT yHotspot,
                     ULONG offBits (from the start of the file),
                     BITMAPINFOHEADER: ULONG cbFix (12), USHORT cx, cy,
                     cPlanes (1), cBitCount; then RGB colour table entries
                     (blue, green, red)
  1-bit masks        cy = twice the image height; rows bottom-up, padded to
                     4 bytes; the lower half is the XOR mask, the upper half
                     the AND mask (AND 0 / XOR 0 black, 0 / 1 white,
                     1 / 0 transparent, 1 / 1 inverted)
  colour pointer     a second BITMAPFILEHEADER right after the first's
                     colour table, for the colour bitmap (real height)
  bitmap array       BITMAPARRAYFILEHEADER: USHORT usType ("BA"),
                     ULONG cbSize, ULONG offNext (0 = last), USHORT
                     cxDisplay, cyDisplay; then the pointer's header(s)

Usage: make_ptr.py OUTDIR   (writes cross.ptr and array.ptr)
"""
import struct
import sys

BFT_POINTER = 0x5450
BFT_COLORPOINTER = 0x5043
BFT_BITMAPARRAY = 0x4142
FILE_HEADER_LEN = 26          # BITMAPFILEHEADER incl. the 12-byte info header
ARRAY_HEADER_LEN = 14         # BITMAPARRAYFILEHEADER fields before it
OFF_BITS = 10                 # offset of offBits in BITMAPFILEHEADER


def file_header(ty, hot, off_bits, cx, cy, bits):
    return struct.pack('<HIhhIIHHHH', ty, FILE_HEADER_LEN, hot[0], hot[1],
                       off_bits, 12, cx, cy, 1, bits)


def rows_1bpp(rows):
    """Bottom-up rows of 0/1 values, packed and padded to 4 bytes."""
    out = b''
    for r in rows:
        b = bytearray(((len(r) + 31) // 32) * 4)
        for x, v in enumerate(r):
            if v:
                b[x // 8] |= 0x80 >> (x % 8)
        out += bytes(b)
    return out


def rows_4bpp(rows):
    out = b''
    for r in rows:
        b = bytearray(((len(r) * 4 + 31) // 32) * 4)
        for x, v in enumerate(r):
            b[x // 2] |= (v & 0xF) << (4 if x % 2 == 0 else 0)
        out += bytes(b)
    return out


BW_TABLE = bytes([0, 0, 0, 0xFF, 0xFF, 0xFF])          # black, white
# 16 colours: black, red, green, blue, then grey.
COLOR_TABLE = (bytes([0, 0, 0, 0, 0, 0xFF, 0, 0xFF, 0, 0xFF, 0, 0])
               + bytes([0x80] * 36))


def masks(n, pel):
    """XOR rows then AND rows (bottom-up) of an n×n image; pel(x, y) is
    'b' black, 'w' white or None transparent."""
    xor, and_ = [], []
    for y in range(n):
        xr, ar = [], []
        for x in range(n):
            v = pel(x, y)
            ar.append(0 if v else 1)
            xr.append(1 if v == 'w' else 0)
        xor.append(xr)
        and_.append(ar)
    return xor + and_


def cross(n, base=0):
    """A black-and-white cross pointer of n×n pels, hot spot in the middle,
    placed at file offset `base`."""
    c = n // 2

    def pel(x, y):
        if abs(x - c) <= 1 or abs(y - c) <= 1:
            return 'w' if (x == c or y == c) else 'b'
        return None
    off = base + FILE_HEADER_LEN + len(BW_TABLE)
    return (file_header(BFT_POINTER, (c, c), off, n, 2 * n, 1) + BW_TABLE
            + rows_1bpp(masks(n, pel)))


def color_square(n, base):
    """A colour pointer at file offset `base`: a red square with a green
    border, hot spot at the bottom-left corner."""
    def color(x, y):
        return 2 if x in (0, n - 1) or y in (0, n - 1) else 1
    mask = rows_1bpp(masks(n, lambda x, y: 'b'))
    colors = rows_4bpp([[color(x, y) for x in range(n)] for y in range(n)])
    headers_len = 2 * FILE_HEADER_LEN + len(BW_TABLE) + len(COLOR_TABLE)
    mask_off = base + headers_len
    color_off = mask_off + len(mask)
    return (file_header(BFT_COLORPOINTER, (0, 0), mask_off, n, 2 * n, 1) + BW_TABLE
            + file_header(BFT_COLORPOINTER, (0, 0), color_off, n, n, 4) + COLOR_TABLE
            + mask + colors)


def array():
    """A bitmap array: a 16×16 black-and-white cross, then a 32×32 colour
    pointer (the display's pointer size, which WinLoadPointer prefers)."""
    first_body = cross(16, ARRAY_HEADER_LEN)
    second_at = ARRAY_HEADER_LEN + len(first_body)
    first = struct.pack('<HIIHH', BFT_BITMAPARRAY, 40, second_at, 0, 0) + first_body
    second = (struct.pack('<HIIHH', BFT_BITMAPARRAY, 40, 0, 0, 0)
              + color_square(32, second_at + ARRAY_HEADER_LEN))
    return first + second


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    with open(out + '/cross.ptr', 'wb') as f:
        f.write(cross(32))
    with open(out + '/array.ptr', 'wb') as f:
        f.write(array())


if __name__ == '__main__':
    main()

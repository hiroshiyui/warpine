# SPDX-License-Identifier: GPL-3.0-or-later
# sdk/Makefile - Warpine's small C runtime for GCC and clang (ADR-0096):
#   make            the runtime for gcc, in build/gcc/
#   make CC=clang   the runtime for clang, in build/clang/
# It writes crt0.o (link it first) and libos2c.a (link it last).

WARPINE_SDK_BUILDING = 1
include os2.mk

LIBC_SRCS = string.c stdlib.c malloc.c stdio.c printf.c dtoa.c divdi3.c
LIBC_OBJS = $(LIBC_SRCS:%.c=$(OS2_BUILD)/%.o)
HEADERS   = $(wildcard include/*.h include/os2/*.h) libc/libc_internal.h

# The runtime's own memcpy and memset must not be turned into calls to
# themselves.
LIBC_CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra -Werror
ifeq ($(findstring clang,$(notdir $(CC))),)
LIBC_CFLAGS += -fno-tree-loop-distribute-patterns
endif

all: $(OS2_CRT0) $(OS2_LIBC)

$(OS2_BUILD)/%.o: libc/%.c $(HEADERS) | $(OS2_BUILD)
	$(CC) $(LIBC_CFLAGS) -c $< -o $@

$(OS2_LIBC): $(LIBC_OBJS)
	rm -f $@
	$(AR) rcs $@ $^

$(OS2_BUILD):
	mkdir -p $@

clean:
	rm -rf build

.PHONY: all clean

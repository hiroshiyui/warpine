# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_socket_test/Makefile - the TCP/IP calls through the SDK's socket headers, built by GCC
# or clang (make CC=clang). Build lx-link first: cargo build --bin lx_link.
# BUILD is where the objects and c_socket_test.exe go.

include ../../sdk/os2.mk

BUILD ?= .
CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_socket_test: skipped, $(LX_LINK) not built (cargo build --bin lx_link)"
else
all: $(BUILD)/c_socket_test.exe
endif

$(BUILD)/c_socket_test.o: c_socket_test.c $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/*/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/c_socket_test.exe: $(BUILD)/c_socket_test.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe

.PHONY: all clean

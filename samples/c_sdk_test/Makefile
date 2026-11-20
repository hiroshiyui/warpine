# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_sdk_test/Makefile - a program built with Warpine's SDK by GCC
# or clang (make CC=clang). Build lx-link first: cargo build --bin lx_link.
# BUILD is where the objects and c_sdk_test.exe go.

include ../../sdk/os2.mk

BUILD ?= .
CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_sdk_test: skipped, $(LX_LINK) not built (cargo build --bin lx_link)"
else
all: $(BUILD)/c_sdk_test.exe
endif

$(BUILD)/c_sdk_test.o: c_sdk_test.c $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/c_sdk_test.exe: $(BUILD)/c_sdk_test.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe

.PHONY: all clean

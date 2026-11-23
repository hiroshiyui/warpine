# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_som_test/Makefile - SOM from GCC or clang with the SDK's som.h
# and Warpine's initializer protocol (ADR-0122).
# `make` (CC=gcc) or `make CC=clang`; BUILD is where everything goes.
# Build the linker first: cargo build --bin lx_link.
include ../../sdk/os2.mk

BUILD ?= .
CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_som_test: skipped, lx-link not built (cargo build --bin lx_link)"
else
all: $(BUILD)/c_som_test.exe
endif

$(BUILD)/c_som_test.o: c_som_test.c $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/c_som_test.exe: $(BUILD)/c_som_test.o $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/c_som_test.o $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe

.PHONY: all clean

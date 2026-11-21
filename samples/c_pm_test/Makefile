# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_pm_test/Makefile - a PM program built with Warpine's SDK by GCC
# or clang (make CC=clang), its resources compiled by os2rc. Build the
# tools first: cargo build --bin lx_link --bin os2rc.
# BUILD is where the objects and c_pm_test.exe go.

include ../../sdk/os2.mk

BUILD ?= .
CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra

ifeq ($(and $(wildcard $(LX_LINK)),$(wildcard $(OS2_RC))),)
all:
	@echo "c_pm_test: skipped, lx-link or os2rc not built (cargo build --bin lx_link --bin os2rc)"
else
all: $(BUILD)/c_pm_test.exe
endif

$(BUILD)/c_pm_test.o: c_pm_test.c c_pm_test.h $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/c_pm_test.res: c_pm_test.rc c_pm_test.h
	$(OS2_RC) $(OS2_RCFLAGS) $< -o $@

$(BUILD)/c_pm_test.exe: $(BUILD)/c_pm_test.o $(BUILD)/c_pm_test.res $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $< $(BUILD)/c_pm_test.res $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.res *.exe

.PHONY: all clean

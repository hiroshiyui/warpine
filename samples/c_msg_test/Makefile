# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_msg_test/Makefile - DosGetMessage with Warpine's message files
# (ADR-0105): app.msg bound into the program, dsk.msg read from a drive.
# `make` (CC=gcc) or `make CC=clang`; BUILD is where everything goes.
# Build the tools first: cargo build --bin lx_link --bin os2msg.
include ../../sdk/os2.mk

BUILD ?= .
CFLAGS = $(OS2_CFLAGS) -std=gnu11 -Wall -Wextra

ifeq ($(and $(wildcard $(LX_LINK)),$(wildcard $(OS2_MSG))),)
all:
	@echo "c_msg_test: skipped, lx-link or os2msg not built (cargo build --bin lx_link --bin os2msg)"
else
all: $(BUILD)/c_msg_test.exe $(BUILD)/dsk.msg
endif

$(BUILD)/%.msg: %.txt
	$(OS2_MSG) --codepage 850 $< -o $@

$(BUILD)/c_msg_test.o: c_msg_test.c $(wildcard $(WARPINE_SDK)/include/*.h $(WARPINE_SDK)/include/os2/*.h)
	$(CC) $(CFLAGS) -c $< -o $@

# app.msg among the inputs: lx-link binds it (__warpine_message_segment).
$(BUILD)/c_msg_test.exe: $(BUILD)/c_msg_test.o $(BUILD)/app.msg $(OS2_CRT0) $(OS2_LIBC)
	$(LX_LINK) --gc-sections $(OS2_CRT0) $(BUILD)/c_msg_test.o $(BUILD)/app.msg $(OS2_LIBC) -o $@

clean:
	rm -f *.o *.exe *.msg

.PHONY: all clean

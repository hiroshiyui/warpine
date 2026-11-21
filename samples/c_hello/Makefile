# SPDX-License-Identifier: GPL-3.0-or-later
# samples/c_hello/Makefile - a C program from a modern compiler (M1):
# GCC (or clang: make CC=clang) builds i386 ELF objects, lx-link makes the
# OS/2 LX executable. Build lx-link first: cargo build --bin lx_link.

CC      ?= gcc
CARGO_TARGET_DIR ?= ../../target
LX_LINK ?= $(CARGO_TARGET_DIR)/debug/lx_link
CFLAGS   = -m32 -O2 -ffreestanding -fno-pic -fno-stack-protector \
           -fno-asynchronous-unwind-tables -ffunction-sections -fdata-sections \
           -fcommon -Wall -Wextra
ifeq ($(CC),clang)
CFLAGS  += --target=i386-unknown-none
endif

OBJS = crt0.o hello.o util.o

ifeq ($(wildcard $(LX_LINK)),)
all:
	@echo "c_hello: skipped, $(LX_LINK) not built (cargo build --bin lx_link)"
else
all: c_hello.exe
endif

%.o: %.c os2min.h
	$(CC) $(CFLAGS) -c $< -o $@

c_hello.exe: $(OBJS)
	$(LX_LINK) --gc-sections $(OBJS) -o $@

clean:
	rm -f *.o *.exe

.PHONY: all clean

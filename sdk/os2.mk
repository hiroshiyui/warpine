# SPDX-License-Identifier: GPL-3.0-or-later
# sdk/os2.mk - build OS/2 programs with GCC or clang and Warpine's SDK
# (ADR-0096). A program's Makefile includes this file, then links with
#   $(LX_LINK) --gc-sections $(OS2_CRT0) <objects> $(OS2_LIBC) -o prog.exe
# after building the runtime with `make -C $(WARPINE_SDK)` (the
# $(OS2_LIBC) rule below does it).
#
# CC picks the compiler (gcc by default; anything with "clang" in its name
# counts as clang). OS2_BUILD is where the runtime's objects go, one
# directory per compiler.

# Included before the program's own rules: keep `all` the default goal.
.DEFAULT_GOAL := all

WARPINE_SDK := $(patsubst %/,%,$(dir $(abspath $(lastword $(MAKEFILE_LIST)))))

ifeq ($(origin CC),default)
CC = gcc
endif

OS2_BUILD ?= $(WARPINE_SDK)/build/$(notdir $(CC))
LX_LINK   ?= $(WARPINE_SDK)/../target/debug/lx_link
# The resource compiler (ADR-0099): `$(OS2_RC) $(OS2_RCFLAGS) x.rc -o x.res`,
# then pass x.res to $(LX_LINK) with the objects.
OS2_RC    ?= $(WARPINE_SDK)/../target/debug/os2rc
OS2_RCFLAGS = -I $(WARPINE_SDK)/include
OS2_CRT0   = $(OS2_BUILD)/crt0.o
OS2_DLL0   = $(OS2_BUILD)/dll0.o
OS2_LIBCXX = $(OS2_BUILD)/libos2cxx.a
OS2_LIBC   = $(OS2_BUILD)/libos2c.a

# Freestanding i386 ELF that lx-link can link: no position-independent
# code, no stack protector or unwind tables, a section per function so
# --gc-sections can drop what is unused, and Warpine's headers in place of
# the host's. OS/2 only keeps the stack 4-byte aligned, so the compilers
# realign where they need more.
OS2_BASEFLAGS = -m32 -O2 -ffreestanding -fno-pic -fno-stack-protector \
                -fno-asynchronous-unwind-tables -ffunction-sections -fdata-sections \
                -D_System= -D__syscall= -D_Optlink=
# (Other OS/2 compilers' convention keywords: all three are the cdecl both
# compilers use by default, also in sources that include no header.)
ifneq ($(findstring clang,$(notdir $(CC))),)
OS2_BASEFLAGS += --target=i386-unknown-none -mstack-alignment=4
else
OS2_BASEFLAGS += -mincoming-stack-boundary=2
endif
OS2_COMPILER_INCLUDE := $(shell $(CC) -m32 -print-file-name=include)
OS2_CFLAGS = $(OS2_BASEFLAGS) -nostdinc -isystem $(WARPINE_SDK)/include -isystem $(OS2_COMPILER_INCLUDE)

# C++ (ADR-0103): g++ beside gcc, clang++ beside clang (a path keeps its
# directory), Warpine's C++ headers before the C ones, exceptions and
# run-time type information on (exceptions bring their unwind tables).
# Hosted, not freestanding: a freestanding clang++ mangles main.
# Link $(OS2_LIBCXX) before $(OS2_LIBC).
OS2_CXX ?= $(patsubst %gcc,%g++,$(patsubst %clang,%clang++,$(CC)))
OS2_CXXFLAGS = $(filter-out -ffreestanding,$(OS2_BASEFLAGS)) -nostdinc -nostdinc++ -isystem $(WARPINE_SDK)/include/c++ \
               -isystem $(WARPINE_SDK)/include -isystem $(OS2_COMPILER_INCLUDE) \
               -std=gnu++17 -fexceptions -frtti

ifndef WARPINE_SDK_BUILDING
$(OS2_LIBC) $(OS2_CRT0) $(OS2_DLL0) $(OS2_LIBCXX):
	$(MAKE) -C $(WARPINE_SDK) CC=$(CC) OS2_BUILD=$(OS2_BUILD)
endif

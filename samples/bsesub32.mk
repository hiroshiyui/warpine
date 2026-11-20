# SPDX-License-Identifier: GPL-3.0-or-later
# samples/bsesub32.mk — included by the VIO / KBD / MOU samples.
#
# Open Watcom's bsesub.h declares the Vio*, Kbd* and Mou* calls APIENTRY16,
# which makes the compiler call them through 16-bit thunks. These samples
# want direct 32-bit imports, so they are built against a copy of that
# header with APIENTRY16 replaced by _Pascal, generated here from the
# local Open Watcom install (vendor/watcom, not in the repository) into the
# sample's own build directory. No Open Watcom text is kept in the tree.

# Included before the sample's own rules: keep `all` the default target.
.DEFAULT_GOAL := all

BSESUB32_DIR = $(CURDIR)/build-bsesub32
BSESUB32_H   = $(BSESUB32_DIR)/bsesub.h

$(BSESUB32_H): $(WATCOM_ROOT)/h/os2/bsesub.h
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	mkdir -p $(BSESUB32_DIR)
	sed 's/APIENTRY16/_Pascal/' $< > $@

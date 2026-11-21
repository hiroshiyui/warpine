# samples/Makefile — build every Open Watcom sample in one go.
#
#   ./vendor/setup_watcom.sh      # once, from the repository root
#   make -C samples               # build all Watcom samples
#   make -C samples clean
#
# Each sample keeps its own Makefile; this one only visits them. Left out:
#   rust_hello a Rust guest built with cargo +nightly (see its README section)
# c_hello, c_sdk_test and c_pm_test are built with GCC (or clang), lx-link
# and os2rc, not Open Watcom; they are skipped until those are built
# (cargo build --bin lx_link --bin os2rc).

SAMPLES  := $(patsubst %/Makefile,%,$(wildcard */Makefile))

.PHONY: all clean $(SAMPLES)

all: $(SAMPLES)

$(SAMPLES):
	$(MAKE) -C $@

clean:
	@for d in $(SAMPLES); do $(MAKE) -C $$d clean || exit 1; done

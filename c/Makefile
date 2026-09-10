# Two compilers, because the gap between them is a finding this port exists to
# show: on 1M rows gcc takes 5.34s and clang 4.77s from the same source.
#
# -pthread is for both paths; the text one divides inside the file and the
# columnar one by column.
#
# `-march=native` under `-Werror` is a hazard, and it fired: on a runner whose
# CPU advertises avx10.1-256, clang 18 refuses with
#
#   invalid feature combination: +avx10.1-256; will be promoted to avx10.1-512
#
# before it reads a line of this code. That is the driver describing the host,
# not a diagnostic about the source, and turning it into a build failure means
# the port stops compiling when a cloud provider refreshes its fleet. The
# warning is suppressed if the compiler in use knows the flag, and left alone if
# it does not, so this stays quiet on gcc and on older clangs.
#
# `-march=native` is itself probed, for the same reason and a different host:
# Apple clang on arm64 rejects the flag outright, so a Mac could not build this
# at all. Asking the compiler whether it takes the flag costs one preprocessor
# run at configure time and means the Makefile does not have to know which
# compiler on which architecture it is talking to.
CC     ?= cc
NATIVE_QUIRK := $(shell $(CC) -Werror -Wno-invalid-feature-combination -E - </dev/null >/dev/null 2>&1 \
                    && echo -Wno-invalid-feature-combination)
NATIVE_ARCH  := $(shell $(CC) -march=native -E - </dev/null >/dev/null 2>&1 && echo -march=native)
CFLAGS ?= -std=c11 -O2 $(NATIVE_ARCH) -Wall -Wextra -Werror -pthread $(NATIVE_QUIRK)
LDFLAGS ?= -pthread
BIN     = csvdiff
# Two programs live here, each with its own main, so the sources are named
# rather than globbed. `c/*.c` would put both mains in one link -- which is
# exactly how CI broke when the generator arrived.
ENGINE  = csvdiff.c parquet.c pqdiff.c parallel.c
GEN     = gen-data.c pqwrite.c parallel.c
OBJ     = csvdiff.o parquet.o pqdiff.o parallel.o

all: $(BIN) gen-data

$(BIN): $(OBJ)
	$(CC) $(CFLAGS) -o $@ $(OBJ) $(LDFLAGS)

csvdiff.o:  csvdiff.c pqdiff.h parquet.h parallel.h
parquet.o:  parquet.c parquet.h
pqdiff.o:   pqdiff.c pqdiff.h parquet.h parallel.h
parallel.o: parallel.c parallel.h

# The generator writes this port's own fixtures and benchmark inputs, byte for
# byte the same files the other generators write. Having it here is what lets a
# check run without a second toolchain built first.
gen-data: $(GEN) pqwrite.h parallel.h
	$(CC) $(CFLAGS) -o $@ $(GEN)

# The engine under AddressSanitizer and UndefinedBehaviorSanitizer. It is a
# target rather than a command in the workflow so that the source list has one
# home: a second copy in CI is a copy that goes stale, and did.
asan: $(ENGINE)
	$(CC) -std=c11 -O1 -g -fsanitize=address,undefined -Wall -Wextra -pthread \
	    $(NATIVE_QUIRK) -o csvdiff-asan $(ENGINE)

clean:
	rm -f $(BIN) gen-data csvdiff-asan $(OBJ)

.PHONY: all asan clean

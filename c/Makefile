# Two compilers, because the gap between them is a finding this port exists to
# show: on 1M rows gcc takes 5.34s and clang 4.77s from the same source.
#
# -pthread is for the Parquet path: the CSV path is still single-threaded, and
# the columnar one is not, because a column is the natural unit of parallel work
# and pretending otherwise would measure the wrong thing.
CC     ?= cc
CFLAGS ?= -std=c11 -O2 -march=native -Wall -Wextra -Werror -pthread
LDFLAGS ?= -pthread
BIN     = csvdiff
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
gen-data: gen-data.c pqwrite.c pqwrite.h
	$(CC) $(CFLAGS) -o $@ gen-data.c pqwrite.c

clean:
	rm -f $(BIN) gen-data $(OBJ)

.PHONY: all clean

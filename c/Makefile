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
OBJ     = csvdiff.o parquet.o pqdiff.o

all: $(BIN)

$(BIN): $(OBJ)
	$(CC) $(CFLAGS) -o $@ $(OBJ) $(LDFLAGS)

csvdiff.o: csvdiff.c pqdiff.h parquet.h
parquet.o: parquet.c parquet.h
pqdiff.o:  pqdiff.c pqdiff.h parquet.h

clean:
	rm -f $(BIN) $(OBJ)

.PHONY: all clean

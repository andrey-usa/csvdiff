# Awkward input

One pair of files holding every shape that has broken an engine in this project, or plausibly
could. Both wrong answers this tool has shipped needed neither an unusual option nor a malformed
file — one wanted a key in the last eight bytes of the file, the other a key outside ASCII — so the
generated benchmark data, which is uniform and pure ASCII, could never have found either.

| Row | What it is there for |
|---|---|
| `CAFÉ` / `café` | a non-ASCII key differing only in case |
| `  padded  ` / `padded` | whitespace around a key, which only `--trim` may ignore |
| `K` (U+212A) / `k` | a case fold that crosses the ASCII boundary and changes the byte length |
| `"has,comma"` | a quoted key holding the delimiter |
| `"a""b"` | a quoted key holding a doubled quote, the one value that is not a slice of the file |
| `"two\nlines"` | a quoted key holding a newline, which row splitting must not treat as a row end |
| `dup` twice | a duplicate key |
| `blank` | an empty value |
| `gone` / `extra` | a key on one side only |
| `z` | a short key in the last row, so it lands within eight bytes of the end of the file |

What the tests assert is not a particular answer but that the implementations cannot disagree about
one. Under the default options `CAFÉ` and `café` are different keys; under `--ignore-case` they are
the same. Either is a defensible answer. Two implementations giving different ones is not.

`EngineParityTest.awkwardInputAgrees` runs it across the Java engines and `parity.yml` runs it
across all five languages.

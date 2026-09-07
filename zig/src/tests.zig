//! The test root: every module, so `zig build test` covers all of them.
//!
//! The engine's own tests are in `csvdiff.zig`; the readers and the decoders keep
//! theirs beside the code they check, because an off-by-one in a bit-packed
//! group is a wrong value rather than a failure, and a wrong value in a
//! comparison tool is the worst kind of bug there is.

test {
    _ = @import("scan.zig");
    _ = @import("field.zig");
    _ = @import("slab.zig");
    _ = @import("text.zig");
    _ = @import("thrift.zig");
    _ = @import("codec.zig");
    _ = @import("encoding.zig");
    _ = @import("parquet.zig");
}

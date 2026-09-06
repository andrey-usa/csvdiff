//! The command line.
//!
//! `--max-memory` is what this port is for. It is not a target the engine tries
//! to respect: it is a fixed buffer the allocator hands out from, so a
//! comparison that would exceed it fails at the allocation that would have
//! crossed the line, with the budget named. Every other implementation here can
//! only be *measured* and hoped about.
//!
//! Exit codes: 0 identical, 1 differences found, 2 error.

const std = @import("std");
const csvdiff = @import("csvdiff.zig");

const usage =
    \\csvdiff - composite-key CSV comparison, byte-level, with a memory budget
    \\
    \\usage:
    \\  csvdiff compare A B -k COLS [options]
    \\
    \\options:
    \\  -k, --key COLS        comma-separated key column(s), required
    \\  -c, --compare COLS    columns to compare (default: all common non-key columns)
    \\  -i, --ignore COLS     columns to skip
    \\      --trim            strip whitespace before comparing
    \\      --ignore-case     ASCII only; a non-ASCII byte is refused by name
    \\      --empty-is-null   treat an empty string and an absent value as equal
    \\      --tolerance N     absolute numeric tolerance
    \\      --max-rows N      rows embedded per section (default 50000)
    \\      --delimiter D     force the delimiter (default: sniff it)
    \\      --max-memory MB   fail rather than exceed this much memory
    \\      --json PATH       write the JSON summary here
    \\
    \\exit codes: 0 identical, 1 differences found, 2 error
    \\
;

fn split(gpa: std.mem.Allocator, s: []const u8) ![][]const u8 {
    var out: std.ArrayList([]const u8) = .empty;
    var it = std.mem.splitScalar(u8, s, ',');
    while (it.next()) |part| {
        if (part.len > 0) try out.append(gpa, part);
    }
    return out.toOwnedSlice(gpa);
}

pub fn main(init: std.process.Init) !u8 {
    // Zig 0.16 hands main its arena, its general-purpose allocator and its
    // arguments; the budget below is carved out of the latter.
    const boot = init.gpa;
    // Argument lists live as long as the process, so they come from the arena
    // the runtime already cleans up rather than being freed by hand.
    const arena = init.arena.allocator();
    const args = try init.minimal.args.toSlice(arena);

    const io = init.io;
    var stdout_buf: [4096]u8 = undefined;
    var stdout = std.Io.File.stdout().writer(io, &stdout_buf);
    var stderr_buf: [4096]u8 = undefined;
    var stderr = std.Io.File.stderr().writer(io, &stderr_buf);

    if (args.len < 2 or std.mem.eql(u8, args[1], "-h") or std.mem.eql(u8, args[1], "--help")) {
        try stdout.interface.writeAll(usage);
        try stdout.interface.flush();
        return if (args.len < 2) 2 else 0;
    }
    if (!std.mem.eql(u8, args[1], "compare")) {
        try stderr.interface.print("error: unknown command: {s}\n", .{args[1]});
        try stderr.interface.flush();
        return 2;
    }

    var opt = csvdiff.Options{ .key = &.{} };
    var files: std.ArrayList([]const u8) = .empty;
    defer files.deinit(boot);
    var json_path: ?[]const u8 = null;
    var max_memory_mb: ?usize = null;

    var i: usize = 2;
    while (i < args.len) : (i += 1) {
        const f = args[i];
        const need = struct {
            fn next(a: []const [:0]const u8, at: *usize) ?[]const u8 {
                if (at.* + 1 >= a.len) return null;
                at.* += 1;
                return a[at.*];
            }
        }.next;

        if (std.mem.eql(u8, f, "-k") or std.mem.eql(u8, f, "--key")) {
            opt.key = try split(arena, need(args, &i) orelse return fail(&stderr, "--key needs a value"));
        } else if (std.mem.eql(u8, f, "-c") or std.mem.eql(u8, f, "--compare")) {
            opt.compare = try split(arena, need(args, &i) orelse return fail(&stderr, "--compare needs a value"));
        } else if (std.mem.eql(u8, f, "-i") or std.mem.eql(u8, f, "--ignore")) {
            opt.ignore = try split(arena, need(args, &i) orelse return fail(&stderr, "--ignore needs a value"));
        } else if (std.mem.eql(u8, f, "--trim")) {
            opt.trim = true;
        } else if (std.mem.eql(u8, f, "--ignore-case")) {
            opt.ignore_case = true;
        } else if (std.mem.eql(u8, f, "--empty-is-null")) {
            opt.empty_is_null = true;
        } else if (std.mem.eql(u8, f, "--tolerance")) {
            opt.tolerance = std.fmt.parseFloat(f64, need(args, &i) orelse "") catch
                return fail(&stderr, "--tolerance needs a number");
        } else if (std.mem.eql(u8, f, "--max-rows")) {
            opt.max_rows = std.fmt.parseInt(usize, need(args, &i) orelse "", 10) catch
                return fail(&stderr, "--max-rows needs a number");
        } else if (std.mem.eql(u8, f, "--max-memory")) {
            max_memory_mb = std.fmt.parseInt(usize, need(args, &i) orelse "", 10) catch
                return fail(&stderr, "--max-memory needs a number of megabytes");
        } else if (std.mem.eql(u8, f, "--delimiter")) {
            const d = need(args, &i) orelse return fail(&stderr, "--delimiter needs a value");
            opt.delimiter = d[0];
        } else if (std.mem.eql(u8, f, "--json")) {
            json_path = need(args, &i) orelse return fail(&stderr, "--json needs a path");
        } else if (std.mem.eql(u8, f, "--engine") or std.mem.eql(u8, f, "-o") or std.mem.eql(u8, f, "--out")) {
            _ = need(args, &i); // accepted so the flag set is portable; there is one engine and no report
        } else if (f.len > 0 and f[0] == '-') {
            return fail(&stderr, "unknown option");
        } else {
            try files.append(boot, f);
        }
    }

    if (files.items.len != 2) return fail(&stderr, "compare needs exactly two files");
    if (opt.key.len == 0) return fail(&stderr, "--key is required");

    // The budget, if one was given. A FixedBufferAllocator cannot hand out more
    // than it holds, so exceeding it is an error at the allocation rather than a
    // number someone notices afterwards.
    var budget: ?[]u8 = null;
    defer if (budget) |bs| boot.free(bs);
    var fixed: std.heap.FixedBufferAllocator = undefined;
    var gpa = boot;
    if (max_memory_mb) |mb| {
        budget = boot.alloc(u8, mb * 1024 * 1024) catch {
            try stderr.interface.print("error: cannot reserve {d} MB\n", .{mb});
            try stderr.interface.flush();
            return 2;
        };
        fixed = std.heap.FixedBufferAllocator.init(budget.?);
        gpa = fixed.allocator();
    }

    var result = csvdiff.compare(io, gpa, files.items[0], files.items[1], opt) catch |err| {
        const message = switch (err) {
            error.OutOfMemory => blk: {
                if (max_memory_mb) |mb| {
                    try stderr.interface.print(
                        "error: the comparison needs more than the {d} MB it was given\n",
                        .{mb},
                    );
                    try stderr.interface.flush();
                    return 2;
                }
                break :blk "out of memory";
            },
            csvdiff.Error.KeyColumnMissing => "key column(s) missing from one of the files",
            csvdiff.Error.ComparedColumnMissing => "compared column missing from one of the files",
            csvdiff.Error.NoHeaderRow => "file has no header row",
            csvdiff.Error.FieldTooLong => "a field is larger than this engine packs",
            csvdiff.Error.CannotReadFile => "cannot read one of the files",
            csvdiff.Error.NonAsciiCaseFold => "--ignore-case outside ASCII needs Unicode case " ++
                "folding, which this port does not carry; use another implementation for that data",
        };
        try stderr.interface.print("error: {s}\n", .{message});
        try stderr.interface.flush();
        return 2;
    };

    defer result.deinit(gpa);

    const c = result.counts;
    if (json_path) |path| {
        var file = std.Io.Dir.cwd().createFile(io, path, .{}) catch
            return fail(&stderr, "cannot write the JSON summary");
        defer file.close(io);
        var buf: [8192]u8 = undefined;
        var w = file.writer(io, &buf);
        try w.interface.print(
            \\{{"counts":{{"a_rows":{d},"b_rows":{d},"a_keys":{d},"b_keys":{d},"matched":{d},
            ++ "\"unchanged\":{d},\"changed\":{d},\"added\":{d},\"removed\":{d}," ++
                "\"a_dup_keys\":{d},\"a_dup_rows\":{d},\"b_dup_keys\":{d},\"b_dup_rows\":{d}}},\"columns\":[",
            .{ c.a_rows, c.b_rows, c.a_keys, c.b_keys, c.matched, c.unchanged, c.changed, c.added, c.removed, c.a_dup_keys, c.a_dup_rows, c.b_dup_keys, c.b_dup_rows },
        );
        for (result.columns, 0..) |col, n| {
            if (n > 0) try w.interface.writeAll(",");
            try w.interface.print(
                "{{\"name\":\"{s}\",\"changed\":{d},\"blanked\":{d},\"filled\":{d}}}",
                .{ col.name, col.changed, col.blanked, col.filled },
            );
        }
        try w.interface.writeAll("]}");
        try w.interface.flush();
    }

    try stdout.interface.print(
        "A {d} rows | B {d} rows | matched {d} (changed {d}) | added {d} | removed {d} | dup keys A {d} B {d} | turbo\n",
        .{ c.a_rows, c.b_rows, c.matched, c.changed, c.added, c.removed, c.a_dup_keys, c.b_dup_keys },
    );
    try stdout.interface.flush();
    return if (result.identical()) 0 else 1;
}

fn fail(stderr: anytype, message: []const u8) !u8 {
    try stderr.interface.print("error: {s}\n", .{message});
    try stderr.interface.flush();
    return 2;
}

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{ .preferred_optimize_mode = .ReleaseFast });

    // How many bytes a scan step takes: 8 is SWAR, 32 and 64 are a vector
    // register's worth, which on x86 needs a CPU target that has them. A build
    // option rather than a runtime switch, so a measurement of an instruction set
    // is not measuring a branch.
    //
    // The default follows the target: thirty-two bytes where it has AVX2, SWAR
    // everywhere else. `zig build` targets the host, so that is every x86 machine
    // of the last decade; a `-Dcpu=baseline` release build still gets SWAR and
    // runs anywhere. It was SWAR everywhere while the vector step measured
    // bimodal (BENCHMARKS, "Zig's scan step"); that was before the sweep's key
    // buffers stopped sharing a cache line (#143).
    const has_avx2 = target.result.cpu.arch == .x86_64 and
        std.Target.x86.featureSetHas(target.result.cpu.features, .avx2);
    const scan_width = b.option(u16, "scan", "bytes per scan step: 8 (SWAR), 32 or 64") orelse
        @as(u16, if (has_avx2) 32 else 8);
    const options = b.addOptions();
    options.addOption(u16, "scan_width", scan_width);

    const exe = b.addExecutable(.{
        .name = "csvdiff",
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/main.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    exe.root_module.addOptions("build_options", options);
    b.installArtifact(exe);

    const run = b.addRunArtifact(exe);
    if (b.args) |args| run.addArgs(args);
    b.step("run", "Run the comparison").dependOn(&run.step);

    const tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/tests.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    tests.root_module.addOptions("build_options", options);
    b.step("test", "Run the unit tests").dependOn(&b.addRunArtifact(tests).step);
}

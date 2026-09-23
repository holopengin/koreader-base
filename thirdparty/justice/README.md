# justice

A paragraph justifier, vendored here so KOADER can build it without network
access and without asking anything of the kokobo container.

Upstream: <https://github.com/kitlangton/justice> (MIT), vendored at v0.3.0.
`vendor/` and `.cargo/config.toml` are `cargo vendor`'s doing, and are what
make `cargo build --offline` work.

## What crengine does with it

crengine lays Latin prose out greedily: it decides each line's break as soon
as the next word no longer fits, and only then spreads the leftover space over
that line's gaps. justice instead solves the whole paragraph at once, choosing
breaks and per-line spacing together, so it can give up a little room here for
a much better line three lines down.

It is wired in as an *optimisation of the same contract*: a line still only
breaks where crengine would have allowed a break, and every line still ends at
the same usable width. Anything crengine models that justice does not — CJK,
vertical text, BiDi, floats, inline boxes, preformatted text, hard newlines,
non-justified alignment — is declined up front in `justiceEligible()`, and the
paragraph then takes the greedy path exactly as before.

The seam between the two is one function: crengine answers only "how wide is
byte range [s,e)?", from the advances it already measured while shaping. All
text policy stays in Rust.

Whether it is actually in the build shows up as `USE_JUSTICE` in crengine's
`crsetup.h`, and at runtime via crengine's diagnostics:

```lua
doc._document:resetJusticeStats()
-- ... render ...
local seen, planned, lines, spaced = doc._document:getJusticeStats()
```

## Building

`CMakeLists.txt` finds `cargo`/`rustc`, maps the C toolchain's target onto a
rustc one (from `gcc -dumpmachine`, so renamed CHOSTs like
`arm-kobo-linux-gnueabihf` still work), checks that this rustc actually has
std for it, then builds into `cargo-target/` — outside both this directory
(which the build system prunes) and the CMake build dir (which a toolchain
change deletes).

If any of that fails, `staging/lib/libjustice.a` is still produced, as a valid
empty archive, so the hand-written link list in
`thirdparty/cmake_modules/koreader_thirdparty_libs.cmake` can name it
unconditionally — and `justice.pc` is withheld, which is how crengine's
`featopt` probe decides `USE_JUSTICE=0`. The build warns loudly rather than
quietly dropping the feature.

### Targets built in a container

The kokobo image has no Rust. Since everything lives under `$PWD`, which is
what gets mounted, build the archive on the host first and the container will
find it:

```sh
cd base/thirdparty/justice
cargo build --release --offline --target arm-unknown-linux-gnueabihf \
    --target-dir ../../build/arm-kobo-linux-gnueabihf/thirdparty/justice/cargo-target
```

The CMake configure step prints this exact command, with the right paths, if
it finds itself in a container without one.

### auxv_compat.c

Rust's std calls `getauxval()` unconditionally on linux-gnu, and glibc only
grew that function in 2.16 — the `kobo` target sits on 2.15. The shim reads
`/proc/self/auxv` instead, is compiled by the *target* compiler (so its idea
of "old glibc" is the sysroot's), and is folded into `libjustice.a` by the
install step so the link list needs no extra entry for it. It compiles to
nothing on glibc ≥ 2.16, which is why the emulator build is unaffected.

## Updating

Take the files from upstream's `rust/`, then re-vendor:

```sh
rm -rf vendor && cargo vendor vendor > .cargo/config.toml
```

The upstream test suites (20 TS tests, 34 Rust tests) and the C ABI checks
live in the upstream repo; `check-c-abi.sh` there is what proves the C seam
crengine depends on.

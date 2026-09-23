# justice: a Rust paragraph solver crengine calls for Latin prose.
#
# The sources (and the crates it needs) are vendored right here, so the build
# never touches the network. The build itself is cargo's, not CMake's, which
# means two wrinkles this file exists to paper over:
#
#  - cargo only reads .cargo/config.toml from its working directory and above,
#    and that is what points it at vendor/, so we cd into the crate first;
#  - we build crengine for more targets than any one rustup installation can
#    have std for, so an unknown or uninstalled target turns justice off
#    instead of failing the build. crengine then justifies exactly as it did
#    before, which is the whole point of the USE_JUSTICE gate.
#
# Either way staging/lib/libjustice.a ends up there: libkoreader-cre.so is
# linked from a hand-written dependency list that names it unconditionally
# (see thirdparty/cmake_modules/koreader_thirdparty_libs.cmake), and that list
# is fixed before we get a say in whether justice is buildable. So when it is
# not, we install a valid empty archive instead and let the link mean nothing.

find_program(CARGO cargo)
find_program(RUSTC rustc)

# Map the C toolchain's target onto a rustc one. The C compiler's own
# -dumpmachine is the least ambiguous source: KOADER's CHOST is renamed
# (arm-kobo-linux-gnueabihf) but gcc still reports what it really is. This
# deliberately does not depend on cargo, because the kokobo container we build
# for the Kobo in has no Rust in it at all - only the *artifact* has to be
# reachable from there, and it is (everything lives under $PWD, which is what
# gets mounted).
#
# Deliberately unmapped: Windows. MinGW's pthread/dl story differs from the
# Unix one we encode in justice.pc, and nothing this fork ships needs it, so
# that target gets the empty archive rather than a link we have never tested.
set(JUSTICE_RUST_TARGET "")
if(CMAKE_C_COMPILER)
    execute_process(
        COMMAND ${CMAKE_C_COMPILER} -dumpmachine
        OUTPUT_VARIABLE TRIPLET
        OUTPUT_STRIP_TRAILING_WHITESPACE
        RESULT_VARIABLE TRIPLET_RC
        ERROR_QUIET
    )
    if(TRIPLET_RC EQUAL 0)
        # Most specific first: android and musl both contain "linux".
        if(TRIPLET MATCHES "^x86_64-.*-linux" OR TRIPLET MATCHES "^amd64-.*-linux")
            set(JUSTICE_RUST_TARGET x86_64-unknown-linux-gnu)
        elseif(TRIPLET MATCHES "^aarch64-.*-android" OR TRIPLET MATCHES "^arm64-.*-android")
            set(JUSTICE_RUST_TARGET aarch64-linux-android)
        elseif(TRIPLET MATCHES "^arm.*-android")
            set(JUSTICE_RUST_TARGET armv7-linux-androideabi)
        elseif(TRIPLET MATCHES "^aarch64-.*-linux.*musl")
            set(JUSTICE_RUST_TARGET aarch64-unknown-linux-musl)
        elseif(TRIPLET MATCHES "^aarch64-.*-linux" OR TRIPLET MATCHES "^arm64-.*-linux")
            set(JUSTICE_RUST_TARGET aarch64-unknown-linux-gnu)
        elseif(TRIPLET MATCHES "^arm.*linux.*eabihf" AND TRIPLET MATCHES "musl")
            set(JUSTICE_RUST_TARGET armv7-unknown-linux-musleabihf)
        elseif(TRIPLET MATCHES "^arm.*linux.*eabihf")
            # The armv6 hard-float baseline: the slowest thing that still
            # runs on every ARMv7 Kobo, which is what we care about.
            set(JUSTICE_RUST_TARGET arm-unknown-linux-gnueabihf)
        elseif(TRIPLET MATCHES "^arm.*linux.*gnueabi")
            set(JUSTICE_RUST_TARGET arm-unknown-linux-gnueabi)
        elseif(TRIPLET MATCHES "apple.*darwin" AND TRIPLET MATCHES "^arm64")
            set(JUSTICE_RUST_TARGET aarch64-apple-darwin)
        elseif(TRIPLET MATCHES "apple.*darwin" AND TRIPLET MATCHES "^x86_64")
            set(JUSTICE_RUST_TARGET x86_64-apple-darwin)
        endif()
    endif()
endif()

# ...and only if this rustc actually has std for it. --print target-libdir
# points at the real directory for the host target and at one that does not
# exist for anything else that was never `rustup target add`ed, so this is a
# rustup-independent existence check. Skipped when there is no rustc to ask.
if(JUSTICE_RUST_TARGET AND RUSTC)
    execute_process(
        COMMAND ${RUSTC} --print target-libdir --target ${JUSTICE_RUST_TARGET}
        OUTPUT_VARIABLE JUSTICE_LIBDIR
        OUTPUT_STRIP_TRAILING_WHITESPACE
        RESULT_VARIABLE JUSTICE_LIBDIR_RC
        ERROR_QUIET
    )
    if(NOT JUSTICE_LIBDIR_RC EQUAL 0 OR NOT IS_DIRECTORY "${JUSTICE_LIBDIR}")
        message(STATUS "justice: no rustc std for ${JUSTICE_RUST_TARGET}")
        set(JUSTICE_RUST_TARGET "")
    endif()
endif()

# Keep cargo's artifacts out of both the source tree (which clean_tree prunes)
# and BINARY_DIR (which a toolchain change rm -rf's). Also the path a host-side
# `cargo build` writes to, which is how the Rust-less kokobo container gets one.
set(JUSTICE_TARGET_DIR ${CMAKE_CURRENT_BINARY_DIR}/cargo-target)
set(JUSTICE_LIB ${JUSTICE_TARGET_DIR}/${JUSTICE_RUST_TARGET}/release/libjustice.a)

# An empty but structurally valid ar archive, written straight out - no
# archiver, no target knowledge, and every linker takes it for an archive
# with nothing in it (which is exactly what it is).
file(WRITE ${CMAKE_CURRENT_BINARY_DIR}/libjustice-empty.a "!<arch>\n")

list(APPEND INSTALL_CMD
    COMMAND ${CMAKE_COMMAND} -E make_directory ${STAGING_DIR}/lib
    COMMAND ${CMAKE_COMMAND} -E make_directory ${STAGING_DIR}/include
    COMMAND ${CMAKE_COMMAND} -E make_directory ${STAGING_DIR}/lib/pkgconfig
)

set(JUSTICE_ENABLED FALSE)
if(JUSTICE_RUST_TARGET AND CARGO AND RUSTC)
    set(JUSTICE_ENABLED TRUE)
    set(JUSTICE_BUILD_CARGO TRUE)
elseif(JUSTICE_RUST_TARGET AND EXISTS ${JUSTICE_LIB})
    # Built on the host before entering a container that has no Rust. Nothing
    # to run in there but install it, which is why this is not an error.
    set(JUSTICE_ENABLED TRUE)
    set(JUSTICE_BUILD_CARGO FALSE)
    message(STATUS "justice: using prebuilt ${JUSTICE_LIB}")
endif()

if(JUSTICE_ENABLED)
    message(STATUS "justice: target ${JUSTICE_RUST_TARGET} (cargo build: ${JUSTICE_BUILD_CARGO})")

    # glibc only grew getauxval() in 2.16 and the KOADER targets are on 2.15,
    # while Rust's std calls it unconditionally. Compiled by the target C
    # compiler, so its idea of "old glibc" is the sysroot's, not the host's.
    list(APPEND BUILD_CMD
        COMMAND ${CMAKE_COMMAND} -E make_directory ${CMAKE_CURRENT_BINARY_DIR}/shim
        COMMAND ${CMAKE_C_COMPILER} -O2 -fPIC
                -c ${SOURCE_DIR}/auxv_compat.c
                -o ${CMAKE_CURRENT_BINARY_DIR}/shim/auxv_compat.o
    )

    # cd has to happen in the same shell as the build, which it does: every
    # COMMAND of a step is run() by one koenv.sh instance.
    if(JUSTICE_BUILD_CARGO)
        list(APPEND BUILD_CMD
            COMMAND cd ${SOURCE_DIR}
            COMMAND ${CARGO} build --release --offline
                    --target ${JUSTICE_RUST_TARGET}
                    --target-dir ${JUSTICE_TARGET_DIR}
        )
    endif()

    # justice pays for some of its own symbols: libm everywhere we care about,
    # and the trio glibc kept in separate libraries until 2.34. On those older
    # systems they are real dependencies of libjustice.a and must follow it on
    # the link line; on newer ones and on Android they are stubs or absent, and
    # crengine links libm regardless.
    set(JUSTICE_PC_LIBS "-ljustice -lm")
    if(NOT ANDROID AND NOT APPLE AND NOT WIN32)
        set(JUSTICE_PC_LIBS "${JUSTICE_PC_LIBS} -lpthread -lrt -ldl")
    endif()

    # Written at configure time and installed at build time: pkg-config only
    # ever looks in PKG_CONFIG_LIBDIR, i.e. the staging directory, so this has
    # to land there before crengine's configure step runs its featopt probe.
    file(MAKE_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}/pkgconfig)
    file(WRITE ${CMAKE_CURRENT_BINARY_DIR}/pkgconfig/justice.pc
"prefix=${STAGING_DIR}
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: justice
Description: Rust paragraph justifier used by crengine for Latin prose
Version: 0.3.0
Libs: -L\${libdir} ${JUSTICE_PC_LIBS}
Cflags: -I\${includedir}
")

    list(APPEND INSTALL_CMD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${JUSTICE_LIB} ${STAGING_DIR}/lib/libjustice.a
        # Fold the getauxval() shim into the same archive, so the hand-written
        # link list that names libjustice.a needs no second entry for it.
        COMMAND ${CMAKE_AR} rs
                ${STAGING_DIR}/lib/libjustice.a
                ${CMAKE_CURRENT_BINARY_DIR}/shim/auxv_compat.o
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${SOURCE_DIR}/include/justice.h ${STAGING_DIR}/include/justice.h
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${CMAKE_CURRENT_BINARY_DIR}/pkgconfig/justice.pc
                ${STAGING_DIR}/lib/pkgconfig/justice.pc
    )
    set(JUSTICE_STATE "enabled:${JUSTICE_RUST_TARGET}")
else()
    # Be loud about this: crengine still builds correctly without justice, but
    # "the fork's justifier silently isn't there" is exactly the kind of thing
    # nobody notices until someone reads the output.
    if(NOT CARGO OR NOT RUSTC)
        message(WARNING
            "justice: cargo/rustc not on PATH and no prebuilt ${JUSTICE_LIB}; "
            "crengine will build without it (USE_JUSTICE=0). On a target built "
            "in a container, run cargo on the host first, e.g. "
            "`cargo build --release --offline --target ${JUSTICE_RUST_TARGET} "
            "--target-dir ${JUSTICE_TARGET_DIR}` from base/thirdparty/justice.")
    else()
        message(WARNING "justice: no rustc support for this target; crengine will build without it (USE_JUSTICE=0)")
    endif()
    list(APPEND INSTALL_CMD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${CMAKE_CURRENT_BINARY_DIR}/libjustice-empty.a
                ${STAGING_DIR}/lib/libjustice.a
        # A leftover .pc from a build that did have cargo would make crengine's
        # probe say yes and then fail to find the symbols the stub lacks.
        COMMAND ${CMAKE_COMMAND} -E rm -f
                ${STAGING_DIR}/lib/pkgconfig/justice.pc
    )
    set(JUSTICE_STATE "disabled")
endif()

external_project(
    BUILD_COMMAND ${BUILD_CMD}
    INSTALL_COMMAND ${INSTALL_CMD}
)

# setup_project() folds this into crengine's configure triggers, so crengine
# re-runs its feature probe whenever justice appears, disappears or is built
# for a different target - which is the only way it can notice a .pc file
# arriving in staging between two builds.
set_property(TARGET ${PROJECT_NAME} PROPERTY HASH "justice:${JUSTICE_STATE}")

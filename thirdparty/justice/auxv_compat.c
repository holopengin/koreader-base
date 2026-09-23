/* auxv_compat.c: getauxval() for glibc older than 2.16.
 *
 * Rust's std calls getauxval() unconditionally on linux-gnu - from both its
 * main codegen unit and its target-detection crate - and glibc only grew that
 * function in 2.16. The KOADER targets we ship to sit on 2.15, so without this
 * libkoreader-cre.so would fail to link with "getauxval: version GLIBC_2.16
 * not found".
 *
 * We are linked before libc, so our definition is the one the linker binds to
 * and no glibc version requirement is recorded. The implementation reads
 * /proc/self/auxv, which is what glibc does too; the file is tiny and this
 * runs a handful of times at most (once per target type std asks about).
 *
 * Everything here is compiled only against a glibc older than 2.16, so on any
 * current system this translation unit is empty and libc's own getauxval is
 * used untouched. See thirdparty/justice/CMakeLists.txt.
 */

#include <fcntl.h>
#include <unistd.h>

#if defined(__GLIBC__) && (__GLIBC__ * 100 + __GLIBC_MINOR__) < 216

unsigned long getauxval(unsigned long type) {
    unsigned long entry[2];
    unsigned long value = 0;
    int fd = open("/proc/self/auxv", O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return 0;
    while (read(fd, entry, sizeof(entry)) == (ssize_t)sizeof(entry)) {
        if (entry[0] == 0UL)        /* AT_NULL: end of the vector */
            break;
        if (entry[0] == type) {
            value = entry[1];
            break;
        }
    }
    close(fd);
    return value;
}

#else

/* ISO C forbids an empty translation unit. */
typedef int justice_auxv_compat_not_needed_here;

#endif

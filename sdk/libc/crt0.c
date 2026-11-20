/* SPDX-License-Identifier: GPL-3.0-or-later */
/* crt0.c - start-up for C programs built with Warpine's SDK (ADR-0096).
 *
 * Warpine enters a 32-bit program as OS/2 does: as a cdecl call with the
 * module handle, a reserved word, the environment and the command line.
 * The command line is the program name, a NUL, the arguments and two
 * NULs. _start splits the arguments into argv (blanks separate them,
 * double quotes group them, \" is a quote), runs the preinit and init
 * arrays, then main(argc, argv, envp), and leaves through exit(). */
#define INCL_DOSPROCESS
#include <os2.h>
#include <stdlib.h>
#include <string.h>

#include "libc_internal.h"

typedef void (*init_fn)(void);
extern init_fn __preinit_array_start[], __preinit_array_end[];
extern init_fn __init_array_start[], __init_array_end[];

int main(int argc, char **argv, char **envp);

/* Count the arguments in `args` and, when `argv` is set, split them into
 * `out`, a buffer at least strlen(args) + 1 bytes long. */
static int split_args(const char *args, char **argv, char *out)
{
    int argc = 0;
    const char *p = args;
    for (;;) {
        while (*p == ' ' || *p == '\t') p++;
        if (*p == '\0') break;
        if (argv) argv[argc] = out;
        argc++;
        int quoted = 0;
        while (*p && (quoted || (*p != ' ' && *p != '\t'))) {
            if (p[0] == '\\' && p[1] == '"') {
                if (out) *out++ = '"';
                p += 2;
            } else if (*p == '"') {
                quoted = !quoted;
                p++;
            } else {
                if (out) *out++ = *p;
                p++;
            }
        }
        if (out) *out++ = '\0';
    }
    return argc;
}

/* The environment block's strings as a NULL-terminated array. */
static char **split_env(char *env)
{
    int count = 0;
    for (char *p = env; p && *p; p += strlen(p) + 1) count++;
    char **envp = malloc((count + 1) * sizeof *envp);
    if (!envp) return NULL;
    int i = 0;
    for (char *p = env; p && *p; p += strlen(p) + 1) envp[i++] = p;
    envp[i] = NULL;
    return envp;
}

__attribute__((force_align_arg_pointer, noreturn, used))
void _start(ULONG hmod, ULONG reserved, char *env, char *cmd)
{
    (void)hmod; (void)reserved;
    __libc_env = env;

    const char *name = cmd ? cmd : "";
    const char *args = name + strlen(name) + 1;
    if (!cmd) args = "";
    int argc = 1 + split_args(args, NULL, NULL);
    char **argv = malloc((argc + 1) * sizeof *argv);
    char *text = malloc(strlen(args) + 1);
    if (!argv || !text) DosExit(EXIT_PROCESS, 255);
    argv[0] = (char *)name;
    split_args(args, argv + 1, text);
    argv[argc] = NULL;

    for (init_fn *f = __preinit_array_start; f < __preinit_array_end; f++) (*f)();
    for (init_fn *f = __init_array_start; f < __init_array_end; f++) (*f)();
    exit(main(argc, argv, split_env(env)));
}

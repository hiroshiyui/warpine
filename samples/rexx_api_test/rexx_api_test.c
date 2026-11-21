/* SPDX-License-Identifier: GPL-3.0-or-later */
/* rexx_api_test.c - the REXX API (REXX.DLL / REXXAPI.DLL, ADR-0085) as an
   OS/2 program uses it: RexxStart on programs in storage and in a file,
   a subcommand handler, an external function, system exits (RXINI, RXSIO
   SAY, RXHLT), the variable pool from inside them, the session queue,
   named queues, and an error's return code. Writes "ok: name" or
   "FAIL: name" for each check, then "rexx_api_test: done <passed>/<total>". */
#define INCL_DOS
#define INCL_REXXSAA
#include <os2.h>
#include <rexxsaa.h>
#include <stdio.h>
#include <string.h>

/* The queue functions (REXXAPI.1-5) have no prototype or constants in
   rexxsaa.h; these follow IBM's description of them. */
#define RXQUEUE_OK      0
#define RXQUEUE_NOTREG  9
#define RXQUEUE_FIFO    0
#define RXQUEUE_LIFO    1
#define RXQUEUE_NOWAIT  0
APIRET APIENTRY RexxCreateQueue(PSZ, ULONG, PSZ, PULONG);
APIRET APIENTRY RexxDeleteQueue(PSZ);
APIRET APIENTRY RexxAddQueue(PSZ, PRXSTRING, ULONG);
APIRET APIENTRY RexxPullQueue(PSZ, PRXSTRING, PDATETIME, ULONG);
APIRET APIENTRY RexxQueryQueue(PSZ, PULONG);

static int passed = 0, total = 0;
static char said[512];        /* what SAY gave the RXSIO exit */
static char command[128];     /* what the subcommand handler got */
static int halt_tests = 0;    /* RXHLT calls */

static void check(const char *what, int good)
{
    total++;
    if (good) { passed++; printf("ok: %s\n", what); }
    else printf("FAIL: %s\n", what);
}

/* Set or fetch one variable of the running program. */
static ULONG pool(UCHAR code, char *name, char *value, ULONG room)
{
    SHVBLOCK b;
    memset(&b, 0, sizeof b);
    b.shvcode = code;
    MAKERXSTRING(b.shvname, name, strlen(name));
    b.shvnamelen = strlen(name);
    if (code == RXSHV_SET || code == RXSHV_SYSET) {
        MAKERXSTRING(b.shvvalue, value, strlen(value));
    } else {
        MAKERXSTRING(b.shvvalue, value, room);
        b.shvvaluelen = room;
    }
    RexxVariablePool(&b);
    if (code == RXSHV_FETCH || code == RXSHV_SYFET) value[b.shvvalue.strlength] = '\0';
    return b.shvret;
}

/* ADDRESS TESTENV: remember the command, set HANDLED, answer RC 5 as an
   error. */
ULONG APIENTRY TestHandler(PRXSTRING cmd, PUSHORT flags, PRXSTRING ret)
{
    char value[64];
    ULONG n = cmd->strlength < sizeof command - 1 ? cmd->strlength : sizeof command - 1;
    memcpy(command, cmd->strptr, n);
    command[n] = '\0';
    pool(RXSHV_SET, "HANDLED", "yes", 0);
    pool(RXSHV_SYFET, "count", value, sizeof value);   /* COUNT, set by RXINI */
    strcpy(ret->strptr, "5");
    ret->strlength = 1;
    *flags = RXSUBCOM_ERROR;
    return 0;
}

/* TWICE(n): n * 2; an argument that is not a number fails. */
ULONG APIENTRY Twice(PUCHAR name, ULONG argc, PRXSTRING argv, PSZ queue, PRXSTRING ret)
{
    char text[32];
    long n;
    if (argc != 1 || argv[0].strlength == 0 || argv[0].strlength >= sizeof text) return 40;
    memcpy(text, argv[0].strptr, argv[0].strlength);
    text[argv[0].strlength] = '\0';
    if (sscanf(text, "%ld", &n) != 1) return 40;
    sprintf(ret->strptr, "%ld", n * 2);
    ret->strlength = strlen(ret->strptr);
    return 0;
}

/* The system exits: RXINI sets COUNT, RXSIO SAY collects the lines, RXHLT
   counts its calls. */
LONG APIENTRY Exits(LONG code, LONG sub, PEXIT parm)
{
    if (code == RXINI) {
        pool(RXSHV_SET, "COUNT", "3", 0);
        return RXEXIT_HANDLED;
    }
    if (code == RXSIO && sub == RXSIOSAY) {
        RXSIOSAY_PARM *p = (RXSIOSAY_PARM *)parm;
        size_t used = strlen(said);
        if (used + p->rxsio_string.strlength + 2 < sizeof said) {
            memcpy(said + used, p->rxsio_string.strptr, p->rxsio_string.strlength);
            strcpy(said + used + p->rxsio_string.strlength, "|");
        }
        return RXEXIT_HANDLED;
    }
    if (code == RXHLT && sub == RXHLTTST) {
        halt_tests++;
        return RXEXIT_HANDLED;
    }
    return RXEXIT_NOT_HANDLED;
}

static LONG run(char *source, char *name, LONG argc, PRXSTRING argv, PRXSYSEXIT exits,
                SHORT *retc, char *result, ULONG room)
{
    RXSTRING instore[2], res;
    LONG rc;
    MAKERXSTRING(instore[0], source, source ? strlen(source) : 0);
    MAKERXSTRING(instore[1], NULL, 0);
    MAKERXSTRING(res, result, room);
    rc = RexxStart(argc, argv, name, source ? instore : NULL, "TESTENV", RXCOMMAND, exits, retc, &res);
    result[res.strlength < room ? res.strlength : room - 1] = '\0';
    return rc;
}

int main(void)
{
    RXSYSEXIT exits[4];
    RXSTRING arg, line;
    SHORT retc = -1;
    char result[256], qname[64], buf[128];
    ULONG dup = 9, count = 9;
    LONG rc;
    FILE *f;

    check("register handlers",
          RexxRegisterSubcomExe("TESTENV", (PFN)TestHandler, NULL) == RXSUBCOM_OK
          && RexxRegisterFunctionExe("TWICE", (PFN)Twice) == RXFUNC_OK
          && RexxRegisterExitExe("TESTEXIT", (PFN)Exits, NULL) == RXEXIT_OK);
    {
        USHORT flag = 0;
        check("query handlers", RexxQuerySubcom("TESTENV", NULL, &flag, NULL) == RXSUBCOM_OK && flag == 1
              && RexxQueryFunction("TWICE") == RXFUNC_OK && RexxQueryFunction("NONE") == RXFUNC_NOTREG);
    }
    check("pool needs a running handler", RexxVariablePool(NULL) == RXSHV_NOAVL);

    /* A program in storage: its argument, a command for TESTENV, a
       function, SAY through the exit, its result. */
    exits[0].sysexit_name = "TESTEXIT"; exits[0].sysexit_code = RXINI;
    exits[1].sysexit_name = "TESTEXIT"; exits[1].sysexit_code = RXSIO;
    exits[2].sysexit_name = "TESTEXIT"; exits[2].sysexit_code = RXHLT;
    exits[3].sysexit_name = NULL;       exits[3].sysexit_code = RXENDLST;
    MAKERXSTRING(arg, "21", 2);
    rc = run("/* */ parse arg n\n"
             "say 'count' count\n"
             "'ping' n\n"
             "say rc handled\n"
             "return twice(n)\n",
             "INSTORE", 1, &arg, exits, &retc, result, sizeof result);
    check("RexxStart in storage", rc == 0);
    check("result and retc", strcmp(result, "42") == 0 && retc == 42);
    check("subcommand handler", strcmp(command, "ping 21") == 0);
    check("RXINI and SAY exits with the pool", strcmp(said, "count 3|5 yes|") == 0);
    check("halt exit called each clause", halt_tests >= 4);

    /* A failing function: Error 40, as a negative return code. */
    rc = run("/* */ return twice('x')", "BAD", 0, NULL, NULL, &retc, result, sizeof result);
    check("error returns minus its number", rc == -40);

    /* The session queue feeds PULL; QUEUE gives back to it. */
    MAKERXSTRING(line, "queued line", 11);
    check("add to the session queue", RexxAddQueue("SESSION", &line, RXQUEUE_FIFO) == RXQUEUE_OK);
    rc = run("/* */ pull x; queue 'back' x; return x", "Q", 0, NULL, NULL, &retc, result, sizeof result);
    check("PULL reads the session queue", rc == 0 && strcmp(result, "QUEUED LINE") == 0);
    MAKERXSTRING(line, buf, sizeof buf);
    check("the program's QUEUE reaches the caller",
          RexxQueryQueue("SESSION", &count) == RXQUEUE_OK && count == 1
          && RexxPullQueue("SESSION", &line, NULL, RXQUEUE_NOWAIT) == RXQUEUE_OK
          && line.strlength == 16 && memcmp(line.strptr, "back QUEUED LINE", 16) == 0);

    /* A named queue. */
    check("create a queue", RexxCreateQueue(qname, sizeof qname, "MYQ", &dup) == RXQUEUE_OK && dup == 0
          && strcmp(qname, "MYQ") == 0);
    MAKERXSTRING(line, "first", 5);
    RexxAddQueue("MYQ", &line, RXQUEUE_FIFO);
    MAKERXSTRING(line, "zeroth", 6);
    RexxAddQueue("MYQ", &line, RXQUEUE_LIFO);
    MAKERXSTRING(line, buf, sizeof buf);
    check("LIFO and FIFO", RexxQueryQueue("MYQ", &count) == RXQUEUE_OK && count == 2
          && RexxPullQueue("MYQ", &line, NULL, RXQUEUE_NOWAIT) == RXQUEUE_OK
          && line.strlength == 6 && memcmp(buf, "zeroth", 6) == 0);
    check("delete a queue", RexxDeleteQueue("MYQ") == RXQUEUE_OK
          && RexxQueryQueue("MYQ", &count) == RXQUEUE_NOTREG);

    /* A program file, found with .CMD added. */
    f = fopen("APITEST.CMD", "w");
    if (f) { fputs("/* REXX */\nparse source . . name\nreturn name\n", f); fclose(f); }
    rc = run(NULL, "APITEST", 0, NULL, NULL, &retc, result, sizeof result);
    check("RexxStart from a file", rc == 0 && strstr(result, "APITEST.CMD") != NULL);
    remove("APITEST.CMD");

    check("deregister", RexxDeregisterSubcom("TESTENV", NULL) == RXSUBCOM_OK
          && RexxDeregisterFunction("TWICE") == RXFUNC_OK
          && RexxDeregisterExit("TESTEXIT", NULL) == RXEXIT_OK
          && RexxQueryFunction("TWICE") == RXFUNC_NOTREG);

    printf("rexx_api_test: done %d/%d\n", passed, total);
    return total - passed;
}

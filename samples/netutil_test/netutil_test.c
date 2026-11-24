/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * netutil_test.c - the TCP/IP network utility routines and SO32DLL's
 * ioctl / readv / writev.
 *
 * Every call is found by name (DosQueryProcAddr), as a program linked by
 * name finds it: no public source gives SO32DLL's or TCP32DLL's ordinals,
 * so Warpine has its own (ADR-0123).
 *
 * TCP32DLL: inet_addr, inet_network, inet_ntoa, inet_makeaddr, inet_lnaof,
 * inet_netof, htonl / htons / ntohl / ntohs, bswap / lswap, dn_comp,
 * dn_expand, dn_skipname, _getshort / putshort, _getlong / putlong,
 * getprotobyname / getprotobynumber, setprotoent / getprotoent /
 * endprotoent.  SO32DLL: ioctl (FIONBIO, FIONREAD), writev and readv on a
 * loopback UDP socket connected to itself.
 *
 * Prints "PASS  <name>" / "FAIL  <name>", then "netutil_test: <p> passed,
 * <f> failed".
 */

#define INCL_DOS
#include <os2.h>
#include <string.h>
#include <stdio.h>

typedef unsigned long u_long;

#pragma pack(1)
struct sockaddr_in {
    short          sin_family;
    unsigned short sin_port;
    u_long         sin_addr;
    char           sin_zero[8];
};
struct protoent {
    char  *p_name;
    char **p_aliases;
    int    p_proto;
};
struct iovec {
    char *iov_base;
    int   iov_len;
};
#pragma pack()

#define AF_INET     2
#define SOCK_DGRAM  2
#define FIONBIO     0x8004667EUL
#define FIONREAD    0x4004667FUL
#define SOCEWOULDBLOCK 35

typedef u_long (_System *PFN_STR)(const char *);
typedef char  *(_System *PFN_NTOA)(u_long);
typedef u_long (_System *PFN_2)(u_long, u_long);
typedef u_long (_System *PFN_1)(u_long);
typedef int    (_System *PFN_COMP)(const char *, unsigned char *, int, unsigned char **, unsigned char **);
typedef int    (_System *PFN_EXPAND)(const unsigned char *, const unsigned char *, const unsigned char *, char *, int);
typedef int    (_System *PFN_SKIP)(const unsigned char *, const unsigned char *);
typedef u_long (_System *PFN_GET)(const unsigned char *);
typedef void   (_System *PFN_PUT)(u_long, unsigned char *);
typedef struct protoent *(_System *PFN_PNAME)(const char *);
typedef struct protoent *(_System *PFN_PNUM)(int);
typedef struct protoent *(_System *PFN_PENT)(void);
typedef void   (_System *PFN_PSET)(int);
typedef int    (_System *PFN_SOCKET)(int, int, int);
typedef int    (_System *PFN_SOCKADDR)(int, struct sockaddr_in *, int);
typedef int    (_System *PFN_GETNAME)(int, struct sockaddr_in *, int *);
typedef int    (_System *PFN_IOCTL)(int, u_long, char *, int);
typedef int    (_System *PFN_IOV)(int, struct iovec *, int);
typedef int    (_System *PFN_RECV)(int, char *, int, int);
typedef int    (_System *PFN_VOID)(void);
typedef int    (_System *PFN_CLOSE)(int);

static int passed, failed;
static HMODULE tcp, so;

static void check(const char *name, int ok)
{
    printf("%s  %s\n", ok ? "PASS" : "FAIL", name);
    if (ok) passed++; else failed++;
}

static PFN find(HMODULE m, const char *name)
{
    PFN p = NULL;
    if (DosQueryProcAddr(m, 0, (PSZ)name, &p) != 0) {
        printf("FAIL  %s not found\n", name);
        failed++;
    }
    return p;
}

static void address_checks(void)
{
    PFN_STR inet_addr = (PFN_STR)find(tcp, "inet_addr");
    PFN_STR inet_network = (PFN_STR)find(tcp, "inet_network");
    PFN_NTOA inet_ntoa = (PFN_NTOA)find(tcp, "inet_ntoa");
    PFN_2 inet_makeaddr = (PFN_2)find(tcp, "inet_makeaddr");
    PFN_1 inet_lnaof = (PFN_1)find(tcp, "inet_lnaof");
    PFN_1 inet_netof = (PFN_1)find(tcp, "inet_netof");
    PFN_1 htonl = (PFN_1)find(tcp, "htonl");
    PFN_1 htons = (PFN_1)find(tcp, "htons");
    PFN_1 ntohl = (PFN_1)find(tcp, "ntohl");
    PFN_1 ntohs = (PFN_1)find(tcp, "ntohs");
    PFN_1 bswap = (PFN_1)find(tcp, "bswap");
    PFN_1 lswap = (PFN_1)find(tcp, "lswap");
    u_long a;
    if (!inet_addr || !inet_network || !inet_ntoa || !inet_makeaddr || !inet_lnaof || !inet_netof
        || !htonl || !htons || !ntohl || !ntohs || !bswap || !lswap) return;

    a = inet_addr("192.168.1.2");
    check("inet_addr gives network order", a == 0x0201A8C0UL);
    check("inet_addr refuses a bad address", inet_addr("1.2.3.300") == 0xFFFFFFFFUL);
    check("inet_addr a.b form", inet_addr("10.1") == htonl(0x0A000001UL));
    check("inet_network", inet_network("10.1") == 0x0A01UL);
    check("inet_ntoa", strcmp(inet_ntoa(0x0100007FUL), "127.0.0.1") == 0);
    a = inet_makeaddr(10, 5);
    check("inet_makeaddr", a == htonl(0x0A000005UL));
    check("inet_lnaof", inet_lnaof(a) == 5);
    check("inet_netof", inet_netof(a) == 10);
    check("htonl / ntohl", htonl(0x12345678UL) == 0x78563412UL && ntohl(0x78563412UL) == 0x12345678UL);
    check("htons / ntohs", (htons(0x1234) & 0xFFFF) == 0x3412 && (ntohs(0x3412) & 0xFFFF) == 0x1234);
    check("bswap / lswap", (bswap(0xABCD) & 0xFFFF) == 0xCDAB && lswap(0x01020304UL) == 0x04030201UL);
}

static void resolver_checks(void)
{
    PFN_COMP dn_comp = (PFN_COMP)find(tcp, "dn_comp");
    PFN_EXPAND dn_expand = (PFN_EXPAND)find(tcp, "dn_expand");
    PFN_SKIP dn_skipname = (PFN_SKIP)find(tcp, "dn_skipname");
    PFN_GET getshort = (PFN_GET)find(tcp, "_getshort");
    PFN_GET getlong = (PFN_GET)find(tcp, "_getlong");
    PFN_PUT putshort = (PFN_PUT)find(tcp, "putshort");
    PFN_PUT putlong = (PFN_PUT)find(tcp, "putlong");
    unsigned char msg[64];
    char name[64];
    int n;
    if (!dn_comp || !dn_expand || !dn_skipname || !getshort || !getlong || !putshort || !putlong) return;

    n = dn_comp("www.example.com", msg, sizeof(msg), NULL, NULL);
    check("dn_comp", n == 17 && msg[0] == 3 && msg[4] == 7 && msg[16] == 0);
    check("dn_expand", dn_expand(msg, msg + n, msg, name, sizeof(name)) == 17 && strcmp(name, "www.example.com") == 0);
    check("dn_skipname", dn_skipname(msg, msg + n) == 17);
    check("dn_expand refuses a short buffer", dn_expand(msg, msg + n, msg, name, 4) == -1);
    putshort(0x1234, msg);
    putlong(0x89ABCDEFUL, msg + 2);
    check("putshort / putlong are big-endian", msg[0] == 0x12 && msg[1] == 0x34 && msg[2] == 0x89 && msg[5] == 0xEF);
    check("_getshort / _getlong", (getshort(msg) & 0xFFFF) == 0x1234 && getlong(msg + 2) == 0x89ABCDEFUL);
}

static void protocol_checks(void)
{
    PFN_PNAME byname = (PFN_PNAME)find(tcp, "getprotobyname");
    PFN_PNUM bynumber = (PFN_PNUM)find(tcp, "getprotobynumber");
    PFN_PSET setprotoent = (PFN_PSET)find(tcp, "setprotoent");
    PFN_PENT getprotoent = (PFN_PENT)find(tcp, "getprotoent");
    PFN_VOID endprotoent = (PFN_VOID)find(tcp, "endprotoent");
    struct protoent *p;
    int count = 0;
    if (!byname || !bynumber || !setprotoent || !getprotoent || !endprotoent) return;

    p = byname("tcp");
    check("getprotobyname(tcp)", p != NULL && p->p_proto == 6 && strcmp(p->p_name, "tcp") == 0);
    p = bynumber(17);
    check("getprotobynumber(17)", p != NULL && strcmp(p->p_name, "udp") == 0);
    check("an unknown protocol", byname("no-such-protocol") == NULL);
    setprotoent(0);
    while (getprotoent() != NULL && count < 100) count++;
    check("getprotoent lists the database", count >= 2 && count < 100);
    endprotoent();
}

static void socket_checks(void)
{
    PFN_SOCKET socket = (PFN_SOCKET)find(so, "socket");
    PFN_SOCKADDR bind = (PFN_SOCKADDR)find(so, "bind");
    PFN_SOCKADDR connect = (PFN_SOCKADDR)find(so, "connect");
    PFN_GETNAME getsockname = (PFN_GETNAME)find(so, "getsockname");
    PFN_IOCTL ioctl = (PFN_IOCTL)find(so, "ioctl");
    PFN_IOV writev = (PFN_IOV)find(so, "writev");
    PFN_IOV readv = (PFN_IOV)find(so, "readv");
    PFN_RECV recv = (PFN_RECV)find(so, "recv");
    PFN_VOID sock_errno = (PFN_VOID)find(so, "sock_errno");
    PFN_CLOSE soclose = (PFN_CLOSE)find(so, "soclose");
    struct sockaddr_in sa;
    struct iovec iov[2];
    char a[8], b[8], buf[4];
    int s, len = sizeof(sa), on = 1, waiting = 0;
    if (!socket || !bind || !connect || !getsockname || !ioctl || !writev || !readv || !recv || !sock_errno || !soclose) return;

    s = socket(AF_INET, SOCK_DGRAM, 0);
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_addr = 0x0100007FUL;
    check("a loopback UDP socket", s >= 0 && bind(s, &sa, sizeof(sa)) == 0
          && getsockname(s, &sa, &len) == 0 && connect(s, &sa, sizeof(sa)) == 0);
    check("ioctl FIONBIO", ioctl(s, FIONBIO, (char *)&on, sizeof(on)) == 0);
    check("a non-blocking recv with nothing waiting", recv(s, buf, sizeof(buf), 0) == -1 && sock_errno() == SOCEWOULDBLOCK);
    iov[0].iov_base = "hello "; iov[0].iov_len = 6;
    iov[1].iov_base = "world";  iov[1].iov_len = 5;
    check("writev", writev(s, iov, 2) == 11);
    DosSleep(50);
    check("ioctl FIONREAD", ioctl(s, FIONREAD, (char *)&waiting, sizeof(waiting)) == 0 && waiting == 11);
    memset(a, 0, sizeof(a)); memset(b, 0, sizeof(b));
    iov[0].iov_base = a; iov[0].iov_len = 4;
    iov[1].iov_base = b; iov[1].iov_len = 7;
    check("readv", readv(s, iov, 2) == 11 && memcmp(a, "hell", 4) == 0 && memcmp(b, "o world", 7) == 0);
    check("ioctl with an unknown request", ioctl(s, 0x40046601UL, (char *)&waiting, sizeof(waiting)) == -1);
    soclose(s);
}

int main(void)
{
    char fail[64];
    if (DosLoadModule(fail, sizeof(fail), "TCP32DLL", &tcp) != 0 || DosLoadModule(fail, sizeof(fail), "SO32DLL", &so) != 0) {
        printf("FAIL  DosLoadModule\n");
        return 1;
    }
    address_checks();
    resolver_checks();
    protocol_checks();
    socket_checks();
    printf("netutil_test: %d passed, %d failed\n", passed, failed);
    return failed != 0;
}

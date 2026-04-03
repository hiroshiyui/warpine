/*
 * socket_test.c — OS/2 TCP/IP socket API smoke test for Warpine
 *
 * Tests SO32DLL.DLL via DosLoadModule / DosQueryProcAddr.
 * Exercises: sock_init, socket, setsockopt, getsockopt, bind, getsockname,
 *            listen, select, connect, accept, send, recv, soclose,
 *            gethostbyname, getservbyname, sock_errno.
 *
 * Build: make -C samples/socket_test
 * Run:   cargo run -- samples/socket_test/socket_test.exe
 *
 * Requires Warpine with SO32DLL builtin module support.
 * The test is self-contained — no real network required beyond loopback.
 */

#define INCL_DOS
#define INCL_VIO
#include <os2.h>
#include <string.h>
#include <stdio.h>

/* ── Minimal BSD socket type/constant definitions ──────────────────────────── */

typedef unsigned long  SOCKET;
typedef unsigned short u_short;
typedef unsigned long  u_long;

#define AF_INET        2
#define SOCK_STREAM    1
#define SOL_SOCKET     0xFFFF
#define SO_REUSEADDR   0x0004
#define SO_TYPE        0x1008
#define INADDR_LOOPBACK 0x7F000001UL  /* 127.0.0.1 in host byte order */

#pragma pack(1)

struct in_addr {
    u_long s_addr;
};

struct sockaddr_in {
    short          sin_family;
    u_short        sin_port;
    struct in_addr sin_addr;
    char           sin_zero[8];
};

struct timeval {
    long tv_sec;
    long tv_usec;
};

/* OS/2 fd_set: 2 × u32 = 64-bit bitmask (handles 0..63) */
typedef struct {
    unsigned long fds_bits[2];
} fd_set;

#define FD_ZERO(s)       memset((s), 0, sizeof(*(s)))
#define FD_SET(h, s)     ((s)->fds_bits[(h) / 32] |= (1UL << ((h) % 32)))
#define FD_ISSET(h, s)   (((s)->fds_bits[(h) / 32] >> ((h) % 32)) & 1)

/* hostent / servent (simplified — only fields we read) */
struct hostent {
    char   *h_name;
    char  **h_aliases;
    int     h_addrtype;
    int     h_length;
    char  **h_addr_list;
};
#define h_addr h_addr_list[0]

struct servent {
    char   *s_name;
    char  **s_aliases;
    int     s_port;
    char   *s_proto;
};

#pragma pack()

/* ── Function pointer types ─────────────────────────────────────────────────── */
typedef int   (_System *PFN_sock_init)(void);
typedef int   (_System *PFN_sock_errno)(void);
typedef SOCKET(_System *PFN_socket)(int, int, int);
typedef int   (_System *PFN_soclose)(SOCKET);
typedef int   (_System *PFN_bind)(SOCKET, struct sockaddr_in *, int);
typedef int   (_System *PFN_listen)(SOCKET, int);
typedef SOCKET(_System *PFN_accept)(SOCKET, struct sockaddr_in *, int *);
typedef int   (_System *PFN_connect)(SOCKET, struct sockaddr_in *, int);
typedef int   (_System *PFN_send)(SOCKET, const char *, int, int);
typedef int   (_System *PFN_recv)(SOCKET, char *, int, int);
typedef int   (_System *PFN_select)(int, fd_set *, fd_set *, fd_set *, struct timeval *);
typedef int   (_System *PFN_getsockname)(SOCKET, struct sockaddr_in *, int *);
typedef int   (_System *PFN_setsockopt)(SOCKET, int, int, const char *, int);
typedef int   (_System *PFN_getsockopt)(SOCKET, int, int, char *, int *);
typedef struct hostent *(_System *PFN_gethostbyname)(const char *);
typedef struct servent *(_System *PFN_getservbyname)(const char *, const char *);

/* ── Helper macros ──────────────────────────────────────────────────────────── */

static int g_pass = 0, g_fail = 0;

static void check(const char *name, int ok)
{
    if (ok) {
        printf("  PASS  %s\n", name);
        g_pass++;
    } else {
        printf("  FAIL  %s\n", name);
        g_fail++;
    }
}

static u_short htons(u_short v)
{
    return (u_short)((v >> 8) | (v << 8));
}
static u_short ntohs(u_short v) { return htons(v); }
static u_long htonl(u_long v)
{
    return ((v & 0xFF) << 24) | ((v & 0xFF00) << 8) |
           ((v >> 8) & 0xFF00) | ((v >> 24) & 0xFF);
}

/* ── Main ──────────────────────────────────────────────────────────────────── */

int main(void)
{
    HMODULE hSo;
    char    szFail[64];
    APIRET  rc;

    /* Function pointers */
    PFN_sock_init    p_sock_init;
    PFN_sock_errno   p_sock_errno;
    PFN_socket       p_socket;
    PFN_soclose      p_soclose;
    PFN_bind         p_bind;
    PFN_listen       p_listen;
    PFN_accept       p_accept;
    PFN_connect      p_connect;
    PFN_send         p_send;
    PFN_recv         p_recv;
    PFN_select       p_select;
    PFN_getsockname  p_getsockname;
    PFN_setsockopt   p_setsockopt;
    PFN_getsockopt   p_getsockopt;
    PFN_gethostbyname p_gethostbyname;
    PFN_getservbyname p_getservbyname;

    printf("socket_test: OS/2 TCP/IP socket API test for Warpine\n");
    printf("=====================================================\n");

    /* 1. Load SO32DLL */
    rc = DosLoadModule(szFail, sizeof(szFail), "SO32DLL", &hSo);
    check("DosLoadModule(SO32DLL)", rc == 0);
    if (rc != 0) {
        printf("  Cannot load SO32DLL (rc=%lu, fail=%s) — aborting\n", rc, szFail);
        return 1;
    }

    /* 2. Resolve ordinals */
#define RESOLVE(var, ord) \
    rc = DosQueryProcAddr(hSo, ord, NULL, (PFN *)&var); \
    check("DosQueryProcAddr(" #ord ")", rc == 0)

    RESOLVE(p_sock_init,     22);
    RESOLVE(p_sock_errno,    18);
    RESOLVE(p_socket,        19);
    RESOLVE(p_soclose,       20);
    RESOLVE(p_bind,           2);
    RESOLVE(p_listen,        10);
    RESOLVE(p_accept,         1);
    RESOLVE(p_connect,        3);
    RESOLVE(p_send,          14);
    RESOLVE(p_recv,          11);
    RESOLVE(p_select,        13);
    RESOLVE(p_getsockname,    7);
    RESOLVE(p_setsockopt,    16);
    RESOLVE(p_getsockopt,     8);
    RESOLVE(p_gethostbyname, 40);
    RESOLVE(p_getservbyname, 42);

    /* 3. sock_init */
    check("sock_init()", p_sock_init() == 0);

    /* 4. Create listening socket */
    SOCKET srv = p_socket(AF_INET, SOCK_STREAM, 0);
    check("socket(AF_INET, SOCK_STREAM, 0)", srv != (SOCKET)-1);
    if (srv == (SOCKET)-1) {
        printf("  Cannot create socket (sock_errno=%d) — aborting\n", p_sock_errno());
        return 1;
    }

    /* 5. SO_REUSEADDR */
    {
        int one = 1;
        int ret = p_setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, (const char *)&one, sizeof(one));
        check("setsockopt(SO_REUSEADDR)", ret == 0);
    }

    /* 6. getsockopt(SO_TYPE) */
    {
        int type = 0, len = (int)sizeof(type);
        int ret = p_getsockopt(srv, SOL_SOCKET, SO_TYPE, (char *)&type, &len);
        check("getsockopt(SO_TYPE)", ret == 0 && type == SOCK_STREAM);
    }

    /* 7. Bind to 127.0.0.1:0 */
    {
        struct sockaddr_in sa;
        memset(&sa, 0, sizeof(sa));
        sa.sin_family      = AF_INET;
        sa.sin_port        = htons(0);
        sa.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        int ret = p_bind(srv, &sa, sizeof(sa));
        check("bind(127.0.0.1:0)", ret == 0);
    }

    /* 8. getsockname — read assigned port */
    u_short assigned_port = 0;
    {
        struct sockaddr_in sa;
        int len = (int)sizeof(sa);
        int ret = p_getsockname(srv, &sa, &len);
        check("getsockname()", ret == 0);
        assigned_port = ntohs(sa.sin_port);
    }

    /* 9. Listen */
    check("listen(backlog=1)", p_listen(srv, 1) == 0);

    /* 10. Connect (second socket) */
    SOCKET cli = p_socket(AF_INET, SOCK_STREAM, 0);
    check("socket(client)", cli != (SOCKET)-1);
    {
        struct sockaddr_in sa;
        memset(&sa, 0, sizeof(sa));
        sa.sin_family      = AF_INET;
        sa.sin_port        = htons(assigned_port);
        sa.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        int ret = p_connect(cli, &sa, sizeof(sa));
        check("connect(loopback)", ret == 0);
    }

    /* 11. select() on listening socket — should be readable */
    {
        fd_set rfds;
        struct timeval tv;
        FD_ZERO(&rfds);
        FD_SET(srv, &rfds);
        tv.tv_sec  = 1;
        tv.tv_usec = 0;
        int nready = p_select((int)srv + 1, &rfds, 0, 0, &tv);
        check("select(srv readable)", nready > 0 && FD_ISSET(srv, &rfds));
    }

    /* 12. Accept */
    SOCKET acc = p_accept(srv, 0, 0);
    check("accept()", acc != (SOCKET)-1);

    /* 13. Send / recv */
    {
        const char *msg = "hello";
        int sent = p_send(cli, msg, 5, 0);
        check("send(5 bytes)", sent == 5);

        char buf[8] = {0};
        int got = p_recv(acc, buf, sizeof(buf) - 1, 0);
        check("recv(5 bytes)", got == 5 && memcmp(buf, msg, 5) == 0);
    }

    /* 14. gethostbyname("localhost") */
    {
        struct hostent *he = p_gethostbyname("localhost");
        check("gethostbyname(localhost)", he != NULL && he->h_addrtype == AF_INET);
        if (he != NULL) {
            unsigned char *ip = (unsigned char *)he->h_addr;
            printf("    resolved: %u.%u.%u.%u\n", ip[0], ip[1], ip[2], ip[3]);
        }
    }

    /* 15. getservbyname("http", "tcp") */
    {
        struct servent *se = p_getservbyname("http", "tcp");
        check("getservbyname(http/tcp)", se != NULL && ntohs((u_short)se->s_port) == 80);
        if (se != NULL) {
            printf("    http port=%d\n", ntohs((u_short)se->s_port));
        }
    }

    /* 16. sock_errno() after bad close */
    {
        p_soclose(0xDEAD);  /* invalid handle */
        int err = p_sock_errno();
        check("sock_errno() after bad soclose", err != 0);
    }

    /* 17. Close all sockets */
    check("soclose(acc)", p_soclose(acc) == 0);
    check("soclose(cli)", p_soclose(cli) == 0);
    check("soclose(srv)", p_soclose(srv) == 0);

    /* 18. Free module */
    rc = DosFreeModule(hSo);
    check("DosFreeModule(SO32DLL)", rc == 0);

    /* Results */
    printf("\n%d passed, %d failed\n", g_pass, g_fail);
    return g_fail > 0 ? 1 : 0;
}

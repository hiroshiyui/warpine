/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * c_socket_test.c - the TCP/IP calls through the SDK's socket headers
 * (<types.h>, <sys/socket.h>, <netinet/in.h>, <arpa/inet.h>, <netdb.h>,
 * <sys/ioctl.h>, <sys/uio.h>, <resolv.h>, <nerrno.h>), built by GCC or
 * clang and linked by name (lx-link, os2api.def): the source-level
 * compatibility of ADR-0123. A TCP connection over loopback carries the
 * data. Prints "PASS  <name>" / "FAIL  <name>", then
 * "c_socket_test: done <passed>/<total>".
 */
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include <types.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <sys/ioctl.h>
#include <sys/uio.h>
#include <resolv.h>
#include <nerrno.h>

static int passed, total;

static void check(const char *name, int ok)
{
    total++;
    if (ok) passed++;
    printf("%s  %s\n", ok ? "PASS" : "FAIL", name);
}

static void utility_checks(void)
{
    struct in_addr a;
    struct protoent *p;
    u_char msg[64];
    char name[64];
    int n;

    a.s_addr = inet_addr("127.0.0.1");
    check("inet_addr", a.s_addr == htonl(INADDR_LOOPBACK));
    check("inet_ntoa", strcmp(inet_ntoa(a), "127.0.0.1") == 0);
    a = inet_makeaddr(10, 7);
    check("inet_makeaddr (a structure returned)", a.s_addr == htonl(0x0a000007UL));
    check("inet_netof / inet_lnaof", inet_netof(a) == 10 && inet_lnaof(a) == 7);
    check("htons / ntohs", htons(0x1234) == 0x3412 && ntohs(0x3412) == 0x1234);
    p = getprotobyname("udp");
    check("getprotobyname", p != NULL && p->p_proto == IPPROTO_UDP);
    n = dn_comp((u_char *)"warpine.example", msg, sizeof(msg), NULL, NULL);
    check("dn_comp / dn_expand", n > 0 && dn_expand(msg, msg + n, msg, (u_char *)name, sizeof(name)) == n
          && strcmp(name, "warpine.example") == 0);
}

static void socket_checks(void)
{
    struct sockaddr_in sa;
    struct timeval tv;
    struct iovec iov[2];
    fd_set rd;
    char a[8], b[8], buf[16];
    int srv, cli, conn, len = sizeof(sa), on = 1, waiting = 0;

    check("sock_init", sock_init() == 0);
    srv = socket(PF_INET, SOCK_STREAM, 0);
    cli = socket(PF_INET, SOCK_STREAM, 0);
    check("socket", srv >= 0 && cli >= 0);
    check("setsockopt SO_REUSEADDR", setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, (char *)&on, sizeof(on)) == 0);
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    check("bind / getsockname / listen", bind(srv, (struct sockaddr *)&sa, sizeof(sa)) == 0
          && getsockname(srv, (struct sockaddr *)&sa, &len) == 0 && sa.sin_port != 0 && listen(srv, SOMAXCONN) == 0);
    check("connect", connect(cli, (struct sockaddr *)&sa, sizeof(sa)) == 0);
    conn = accept(srv, NULL, NULL);
    check("accept", conn >= 0);

    iov[0].iov_base = "hello "; iov[0].iov_len = 6;
    iov[1].iov_base = "world";  iov[1].iov_len = 5;
    check("writev", writev(cli, iov, 2) == 11);
    FD_ZERO(&rd);
    FD_SET(conn, &rd);
    tv.tv_sec = 2; tv.tv_usec = 0;
    check("select sees the data", select(0, &rd, NULL, NULL, &tv) == 1 && FD_ISSET(conn, &rd));
    DosSleep(50);
    check("ioctl FIONREAD", ioctl(conn, FIONREAD, (char *)&waiting, sizeof(waiting)) == 0 && waiting == 11);
    memset(a, 0, sizeof(a)); memset(b, 0, sizeof(b));
    iov[0].iov_base = a; iov[0].iov_len = 4;
    iov[1].iov_base = b; iov[1].iov_len = 7;
    check("readv", readv(conn, iov, 2) == 11 && memcmp(a, "hell", 4) == 0 && memcmp(b, "o world", 7) == 0);
    check("send / recv", send(conn, "pong", 4, 0) == 4 && recv(cli, buf, sizeof(buf), 0) == 4 && memcmp(buf, "pong", 4) == 0);
    check("ioctl FIONBIO", ioctl(cli, FIONBIO, (char *)&on, sizeof(on)) == 0);
    check("a non-blocking recv: SOCEWOULDBLOCK", recv(cli, buf, sizeof(buf), 0) == -1 && sock_errno() == SOCEWOULDBLOCK);
    check("soclose", soclose(conn) == 0 && soclose(cli) == 0 && soclose(srv) == 0);
    check("a closed socket: SOCENOTSOCK or SOCEBADF", soclose(srv) == -1
          && (sock_errno() == SOCENOTSOCK || sock_errno() == SOCEBADF));
}

int main(void)
{
    utility_checks();
    socket_checks();
    printf("c_socket_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}

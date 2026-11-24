/* SPDX-License-Identifier: GPL-3.0-or-later */
/* resolv.h - the resolver (OS/2 TCP/IP 4.0, TCP32DLL), for GCC and clang:
 * name packing, and queries to the host's name servers. */
#ifndef WARPINE_RESOLV_H
#define WARPINE_RESOLV_H

#include <types.h>

#ifdef __cplusplus
extern "C" {
#endif

int APIENTRY dn_comp(u_char *exp_dn, u_char *comp_dn, int length, u_char **dnptrs, u_char **lastdnptr);
int APIENTRY dn_expand(u_char *msg, u_char *eomorig, u_char *comp_dn, u_char *exp_dn, int length);
int APIENTRY dn_skipname(u_char *comp_dn, u_char *eom);
u_short APIENTRY _getshort(u_char *msgp);
u_long APIENTRY _getlong(u_char *msgp);
void APIENTRY putshort(u_short s, u_char *msgp);
void APIENTRY putlong(u_long l, u_char *msgp);

/* Queries (RFC 1035): QUERY is res_mkquery's only op; C_IN the Internet
 * class; T_A, T_NS, T_CNAME, T_PTR, T_MX the common types. The results
 * are lengths, or -1 with h_errno set. */
#define QUERY   0
#define C_IN    1
#define T_A     1
#define T_NS    2
#define T_CNAME 5
#define T_PTR   12
#define T_MX    15
#define PACKETSZ 512
struct rrec;
int APIENTRY res_init(void);
int APIENTRY res_mkquery(int op, char *dname, int qclass, int type, char *data, int datalen,
                         struct rrec *newrr, char *buf, int buflen);
int APIENTRY res_send(char *msg, int msglen, char *answer, int anslen);
int APIENTRY res_query(char *dname, int qclass, int type, u_char *answer, int anslen);
int APIENTRY res_search(char *dname, int qclass, int type, u_char *answer, int anslen);
int APIENTRY res_querydomain(char *name, char *domain, int qclass, int type, u_char *answer, int anslen);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_RESOLV_H */

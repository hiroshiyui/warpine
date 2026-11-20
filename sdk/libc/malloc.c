/* SPDX-License-Identifier: GPL-3.0-or-later */
/* malloc.c - Warpine's small C runtime (ADR-0096): a first-fit heap.
 *
 * Memory comes from DosAllocMem in arenas of at least ARENA_SIZE, never
 * given back. Every block starts with a 16-byte header, so the memory
 * handed out is 16-byte aligned. Free blocks sit on one list in address
 * order, and a freed block merges with the free neighbours it touches.
 * Requests of LARGE_SIZE or more get their own DosAllocMem object, which
 * free() releases. One lock serialises the heap across threads. */
#define INCL_DOSMEMMGR
#include <os2.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <stdint.h>

#include "libc_internal.h"

#define ALIGN        16u
#define ARENA_SIZE   (256u * 1024u)
#define LARGE_SIZE   (128u * 1024u)
#define BLOCK_LARGE  0x1u     /* in `flags`: its own DosAllocMem object */
#define BLOCK_MAGIC  0x4B4C4257u

typedef struct block {
    size_t        size;    /* the whole block, header included */
    unsigned      flags;
    unsigned      magic;
    struct block *next;    /* the next free block, by address */
} block;

_Static_assert(sizeof(block) == ALIGN, "the header keeps blocks aligned");

static block *free_list;
static libc_lock heap_lock;

static size_t round_up(size_t n, size_t to) { return (n + to - 1) & ~(to - 1); }

static void *os_alloc(size_t size)
{
    PVOID p = NULL;
    if (DosAllocMem(&p, size, PAG_COMMIT | PAG_READ | PAG_WRITE) != NO_ERROR) return NULL;
    return p;
}

/* Put `b` on the free list in address order, merged with its neighbours. */
static void insert_free(block *b)
{
    block **link = &free_list;
    while (*link && *link < b) link = &(*link)->next;
    b->next = *link;
    *link = b;
    if (b->next && (char *)b + b->size == (char *)b->next) {
        b->size += b->next->size;
        b->next = b->next->next;
    }
    if (link != &free_list) {
        block *prev = (block *)((char *)link - offsetof(block, next));
        if ((char *)prev + prev->size == (char *)b) {
            prev->size += b->size;
            prev->next = b->next;
        }
    }
}

static block *take_free(size_t need)
{
    for (block **link = &free_list; *link; link = &(*link)->next) {
        block *b = *link;
        if (b->size < need) continue;
        if (b->size - need >= 2 * ALIGN) {
            block *rest = (block *)((char *)b + need);
            rest->size = b->size - need;
            rest->next = b->next;
            *link = rest;
            b->size = need;
        } else {
            *link = b->next;
        }
        return b;
    }
    return NULL;
}

void *malloc(size_t size)
{
    if (size == 0) size = 1;
    if (size > SIZE_MAX - 2 * ALIGN) { errno = ENOMEM; return NULL; }
    size_t need = round_up(size + sizeof(block), ALIGN);
    block *b;
    if (need >= LARGE_SIZE) {
        b = os_alloc(need);
        if (!b) { errno = ENOMEM; return NULL; }
        b->size = need;
        b->flags = BLOCK_LARGE;
    } else {
        __libc_lock(&heap_lock);
        b = take_free(need);
        if (!b) {
            block *arena = os_alloc(ARENA_SIZE);
            if (arena) {
                arena->size = ARENA_SIZE;
                insert_free(arena);
                b = take_free(need);
            }
        }
        __libc_unlock(&heap_lock);
        if (!b) { errno = ENOMEM; return NULL; }
        b->flags = 0;
    }
    b->magic = BLOCK_MAGIC;
    b->next = NULL;
    return b + 1;
}

/* The header of an allocation, or abort() for a pointer malloc never gave. */
static block *header_of(void *ptr)
{
    block *b = (block *)ptr - 1;
    if (((uintptr_t)ptr & (ALIGN - 1)) || b->magic != BLOCK_MAGIC) abort();
    return b;
}

void free(void *ptr)
{
    if (!ptr) return;
    block *b = header_of(ptr);
    b->magic = 0;
    if (b->flags & BLOCK_LARGE) {
        DosFreeMem(b);
        return;
    }
    __libc_lock(&heap_lock);
    insert_free(b);
    __libc_unlock(&heap_lock);
}

void *calloc(size_t count, size_t size)
{
    if (size && count > SIZE_MAX / size) { errno = ENOMEM; return NULL; }
    void *p = malloc(count * size);
    return p ? memset(p, 0, count * size) : NULL;
}

void *realloc(void *ptr, size_t size)
{
    if (!ptr) return malloc(size);
    if (size == 0) { free(ptr); return NULL; }
    size_t have = header_of(ptr)->size - sizeof(block);
    if (size <= have) return ptr;
    void *p = malloc(size);
    if (!p) return NULL;
    memcpy(p, ptr, have);
    free(ptr);
    return p;
}

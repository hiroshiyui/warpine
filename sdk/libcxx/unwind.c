/* SPDX-License-Identifier: GPL-3.0-or-later */
/* unwind.c - the Itanium ABI's base unwinder for Warpine's SDK (ADR-0103).
 *
 * Written from the System V i386 ABI, the DWARF 4 standard (call-frame
 * information, section 6.4), the Linux Standard Base's description of
 * .eh_frame and its pointer encodings, and the Itanium C++ ABI's
 * exception-handling chapter (level I).
 *
 * lx-link places every kept .eh_frame record between __EH_FRAME_BEGIN__ and
 * __EH_FRAME_END__; the unwinder finds a frame's FDE by scanning them.
 * Registers are DWARF's i386 numbers: 0 eax, 1 ecx, 2 edx, 3 ebx, 4 esp,
 * 5 ebp, 6 esi, 7 edi, 8 eip (the return address column).
 *
 * OS/2 exception handlers (DosSetExceptionHandler) are registered in the
 * thread's chain at fs:[0], in records on the stack. Before control moves
 * to a landing pad, the records of the frames being left (below its stack
 * pointer) leave the chain, as unwinding through them on OS/2 would. */
#define INCL_DOSEXCEPTIONS
#include <os2.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unwind.h>

#define DWARF_REGS   9
#define REG_EAX      0
#define REG_EDX      2
#define REG_ESP      4
#define REG_EIP      8
#define MAX_REMEMBER 8

struct _Unwind_Context {
    uint32_t reg[DWARF_REGS];
    uint32_t cfa;               /* the frame's canonical frame address */
    uint32_t args_size;         /* DW_CFA_GNU_args_size at its IP */
    uint32_t lsda;
    uint32_t func_start;
    _Unwind_Personality_Fn personality;
};

extern const unsigned char __EH_FRAME_BEGIN__[], __EH_FRAME_END__[];

/* ── Reading .eh_frame ──────────────────────────────────────────────── */

#define DW_EH_PE_omit    0xFF
#define DW_EH_PE_absptr  0x00
#define DW_EH_PE_uleb128 0x01
#define DW_EH_PE_udata2  0x02
#define DW_EH_PE_udata4  0x03
#define DW_EH_PE_udata8  0x04
#define DW_EH_PE_sleb128 0x09
#define DW_EH_PE_sdata2  0x0A
#define DW_EH_PE_sdata4  0x0B
#define DW_EH_PE_sdata8  0x0C
#define DW_EH_PE_pcrel   0x10
#define DW_EH_PE_datarel 0x30
#define DW_EH_PE_funcrel 0x40
#define DW_EH_PE_aligned 0x50
#define DW_EH_PE_indirect 0x80

static uint32_t read_uleb(const unsigned char **p)
{
    uint32_t v = 0;
    unsigned shift = 0;
    unsigned char b;
    do {
        b = *(*p)++;
        if (shift < 32) v |= (uint32_t)(b & 0x7F) << shift;
        shift += 7;
    } while (b & 0x80);
    return v;
}

static int32_t read_sleb(const unsigned char **p)
{
    uint32_t v = 0;
    unsigned shift = 0;
    unsigned char b;
    do {
        b = *(*p)++;
        if (shift < 32) v |= (uint32_t)(b & 0x7F) << shift;
        shift += 7;
    } while (b & 0x80);
    if (shift < 32 && (b & 0x40)) v |= ~0u << shift;
    return (int32_t)v;
}

static uint32_t read_u32(const unsigned char *p) { uint32_t v; memcpy(&v, p, 4); return v; }

/* A pointer in `enc` at *p (DW_EH_PE_*), with `func` for funcrel. */
uint32_t __warpine_read_encoded(const unsigned char **p, unsigned char enc, uint32_t func)
{
    const unsigned char *start = *p;
    uint32_t v;
    if (enc == DW_EH_PE_omit) return 0;
    if ((enc & 0x70) == DW_EH_PE_aligned) {
        uintptr_t a = ((uintptr_t)*p + 3) & ~(uintptr_t)3;
        v = read_u32((const unsigned char *)a);
        *p = (const unsigned char *)a + 4;
        return v;
    }
    switch (enc & 0x0F) {
    case DW_EH_PE_absptr:
    case DW_EH_PE_udata4:
    case DW_EH_PE_sdata4:  v = read_u32(*p); *p += 4; break;
    case DW_EH_PE_uleb128: v = read_uleb(p); break;
    case DW_EH_PE_sleb128: v = (uint32_t)read_sleb(p); break;
    case DW_EH_PE_udata2:  v = (uint32_t)((*p)[0] | (*p)[1] << 8); *p += 2; break;
    case DW_EH_PE_sdata2:  v = (uint32_t)(int32_t)(int16_t)((*p)[0] | (*p)[1] << 8); *p += 2; break;
    case DW_EH_PE_udata8:
    case DW_EH_PE_sdata8:  v = read_u32(*p); *p += 8; break;
    default:               return 0;
    }
    if (v == 0 && (enc & 0x70) != DW_EH_PE_pcrel) return 0;
    switch (enc & 0x70) {
    case DW_EH_PE_pcrel:   v += (uint32_t)(uintptr_t)start; break;
    case DW_EH_PE_funcrel: v += func; break;
    default: break;
    }
    if (enc & DW_EH_PE_indirect) v = read_u32((const unsigned char *)(uintptr_t)v);
    return v;
}

/* What a CIE says. */
struct cie {
    const unsigned char *insns, *end;
    uint32_t code_align;
    int32_t data_align;
    uint32_t ra_reg;
    unsigned char fde_enc, lsda_enc;
    int has_z;
    _Unwind_Personality_Fn personality;
};

static int parse_cie(const unsigned char *cie, struct cie *out)
{
    uint32_t len = read_u32(cie);
    const unsigned char *p = cie + 8, *end = cie + 4 + len;
    unsigned char version = *p++;
    const char *aug = (const char *)p;
    p += strlen(aug) + 1;
    memset(out, 0, sizeof *out);
    out->fde_enc = DW_EH_PE_absptr;
    out->lsda_enc = DW_EH_PE_omit;
    if (aug[0] == 'e' && aug[1] == 'h') p += 4;
    out->code_align = read_uleb(&p);
    out->data_align = read_sleb(&p);
    out->ra_reg = version == 1 ? *p++ : read_uleb(&p);
    if (aug[0] == 'z') {
        uint32_t aug_len = read_uleb(&p);
        const unsigned char *aug_end = p + aug_len;
        out->has_z = 1;
        for (const char *a = aug + 1; *a; a++) {
            if (*a == 'L') out->lsda_enc = *p++;
            else if (*a == 'R') out->fde_enc = *p++;
            else if (*a == 'P') {
                unsigned char enc = *p++;
                out->personality = (_Unwind_Personality_Fn)(uintptr_t)__warpine_read_encoded(&p, enc, 0);
            } else if (*a != 'S') break;
        }
        p = aug_end;
    }
    out->insns = p;
    out->end = end;
    return 1;
}

/* The FDE covering `pc`, with its CIE, function start, LSDA and
 * instructions. */
struct fde {
    struct cie cie;
    uint32_t start, range, lsda;
    const unsigned char *insns, *end;
};

static int find_fde(uint32_t pc, struct fde *out)
{
    const unsigned char *p = __EH_FRAME_BEGIN__;
    while (p + 4 <= __EH_FRAME_END__) {
        uint32_t len = read_u32(p);
        if (len == 0 || len == 0xFFFFFFFFu) break;
        const unsigned char *next = p + 4 + len;
        uint32_t id = read_u32(p + 4);
        if (id != 0) {
            const unsigned char *cie = p + 4 - id;
            struct cie c;
            if (parse_cie(cie, &c)) {
                const unsigned char *q = p + 8;
                uint32_t start = __warpine_read_encoded(&q, c.fde_enc, 0);
                uint32_t range = __warpine_read_encoded(&q, c.fde_enc & 0x0F, 0);
                if (pc >= start && pc < start + range) {
                    out->cie = c;
                    out->start = start;
                    out->range = range;
                    out->lsda = 0;
                    if (c.has_z) {
                        uint32_t aug_len = read_uleb(&q);
                        const unsigned char *aug_end = q + aug_len;
                        if (c.lsda_enc != DW_EH_PE_omit) {
                            const unsigned char *l = q;
                            out->lsda = __warpine_read_encoded(&l, c.lsda_enc, start);
                        }
                        q = aug_end;
                    }
                    out->insns = q;
                    out->end = next;
                    return 1;
                }
            }
        }
        p = next;
    }
    return 0;
}

/* ── Call-frame instructions ────────────────────────────────────────── */

enum rule { RULE_SAME, RULE_UNDEFINED, RULE_OFFSET, RULE_VAL_OFFSET, RULE_REGISTER, RULE_EXPRESSION,
            RULE_VAL_EXPRESSION };

struct reg_rule { unsigned char how; int32_t value; const unsigned char *expr; };

struct frame_state {
    struct reg_rule reg[DWARF_REGS];
    uint32_t cfa_reg;
    int32_t cfa_offset;
    const unsigned char *cfa_expr;    /* DW_CFA_def_cfa_expression, else null */
    uint32_t args_size;
};

#define DW_CFA_advance_loc        0x40
#define DW_CFA_offset             0x80
#define DW_CFA_restore            0xC0
#define DW_CFA_nop                0x00
#define DW_CFA_set_loc            0x01
#define DW_CFA_advance_loc1       0x02
#define DW_CFA_advance_loc2       0x03
#define DW_CFA_advance_loc4       0x04
#define DW_CFA_offset_extended    0x05
#define DW_CFA_restore_extended   0x06
#define DW_CFA_undefined          0x07
#define DW_CFA_same_value         0x08
#define DW_CFA_register           0x09
#define DW_CFA_remember_state     0x0A
#define DW_CFA_restore_state      0x0B
#define DW_CFA_def_cfa            0x0C
#define DW_CFA_def_cfa_register   0x0D
#define DW_CFA_def_cfa_offset     0x0E
#define DW_CFA_def_cfa_expression 0x0F
#define DW_CFA_expression         0x10
#define DW_CFA_offset_extended_sf 0x11
#define DW_CFA_def_cfa_sf         0x12
#define DW_CFA_def_cfa_offset_sf  0x13
#define DW_CFA_val_offset         0x14
#define DW_CFA_val_offset_sf      0x15
#define DW_CFA_val_expression     0x16
#define DW_CFA_GNU_args_size      0x2E
#define DW_CFA_GNU_negative_offset_extended 0x2F

static void set_rule(struct frame_state *fs, uint32_t reg, unsigned char how, int32_t value)
{
    if (reg < DWARF_REGS) { fs->reg[reg].how = how; fs->reg[reg].value = value; fs->reg[reg].expr = NULL; }
}

/* ── DWARF expressions ──────────────────────────────────────────────────
 * GCC describes frames that realign the stack with them (the CFA read
 * from the frame, registers saved relative to %ebp). The subset compilers
 * emit for call frames: constants, register-relative values, dereference
 * and arithmetic. `expr` is the ULEB128 length and the operations. */

#define EXPR_STACK 16

static int eval_expr(const unsigned char *expr, const uint32_t *reg, int push_cfa, uint32_t cfa, uint32_t *out)
{
    const unsigned char *p = expr;
    uint32_t len = read_uleb(&p);
    const unsigned char *end = p + len;
    uint32_t st[EXPR_STACK];
    int sp = 0;
    if (push_cfa) st[sp++] = cfa;
    while (p < end) {
        unsigned char op = *p++;
        uint32_t a, b;
        if (sp >= EXPR_STACK - 1) return 0;
        if (op >= 0x30 && op <= 0x4F) { st[sp++] = op - 0x30u; continue; }                 /* DW_OP_lit<n> */
        if (op >= 0x70 && op <= 0x8F) {                                                     /* DW_OP_breg<n> */
            if (op - 0x70u >= DWARF_REGS) return 0;
            st[sp++] = reg[op - 0x70] + (uint32_t)read_sleb(&p);
            continue;
        }
        switch (op) {
        case 0x06: if (!sp) return 0; st[sp - 1] = *(const uint32_t *)(uintptr_t)st[sp - 1]; break;  /* deref */
        case 0x08: st[sp++] = *p++; break;                                                  /* const1u */
        case 0x09: st[sp++] = (uint32_t)(int32_t)(int8_t)*p++; break;                       /* const1s */
        case 0x0A: st[sp++] = (uint32_t)(p[0] | p[1] << 8); p += 2; break;                  /* const2u */
        case 0x0B: st[sp++] = (uint32_t)(int32_t)(int16_t)(p[0] | p[1] << 8); p += 2; break; /* const2s */
        case 0x0C: case 0x0D: st[sp++] = read_u32(p); p += 4; break;                        /* const4u/s */
        case 0x10: st[sp++] = read_uleb(&p); break;                                         /* constu */
        case 0x11: st[sp++] = (uint32_t)read_sleb(&p); break;                               /* consts */
        case 0x12: if (!sp) return 0; st[sp] = st[sp - 1]; sp++; break;                     /* dup */
        case 0x13: if (!sp) return 0; sp--; break;                                          /* drop */
        case 0x14: if (sp < 2) return 0; st[sp] = st[sp - 2]; sp++; break;                  /* over */
        case 0x16: if (sp < 2) return 0; a = st[sp - 1]; st[sp - 1] = st[sp - 2]; st[sp - 2] = a; break; /* swap */
        case 0x23: if (!sp) return 0; st[sp - 1] += read_uleb(&p); break;                   /* plus_uconst */
        case 0x1A: case 0x1C: case 0x21: case 0x22: case 0x27:                              /* and minus or plus xor */
            if (sp < 2) return 0;
            b = st[--sp];
            a = st[sp - 1];
            st[sp - 1] = op == 0x1A ? (a & b) : op == 0x1C ? a - b : op == 0x21 ? (a | b) : op == 0x22 ? a + b : (a ^ b);
            break;
        case 0x1F: if (!sp) return 0; st[sp - 1] = 0u - st[sp - 1]; break;                  /* neg */
        case 0x96: break;                                                                   /* nop */
        default: return 0;
        }
    }
    if (!sp) return 0;
    *out = st[sp - 1];
    return 1;
}

/* Skip an expression's length and operations. */
static const unsigned char *skip_expr(const unsigned char *p)
{
    uint32_t len = read_uleb(&p);
    return p + len;
}

/* Run `insns` until the location passes `pc`; `initial` holds the CIE's
 * rules for DW_CFA_restore. Returns 0 on something not supported. */
static int run_cfa(const unsigned char *p, const unsigned char *end, const struct cie *cie, uint32_t loc,
                   uint32_t pc, struct frame_state *fs, const struct frame_state *initial)
{
    struct frame_state stack[MAX_REMEMBER];
    int depth = 0;
    while (p < end && loc <= pc) {
        unsigned char op = *p++;
        uint32_t reg, delta;
        switch (op & 0xC0) {
        case DW_CFA_advance_loc:
            loc += (op & 0x3F) * cie->code_align;
            continue;
        case DW_CFA_offset:
            set_rule(fs, op & 0x3F, RULE_OFFSET, (int32_t)read_uleb(&p) * cie->data_align);
            continue;
        case DW_CFA_restore:
            reg = op & 0x3F;
            if (initial && reg < DWARF_REGS) fs->reg[reg] = initial->reg[reg];
            continue;
        }
        switch (op) {
        case DW_CFA_nop: break;
        case DW_CFA_set_loc: loc = __warpine_read_encoded(&p, cie->fde_enc, 0); break;
        case DW_CFA_advance_loc1: loc += *p++ * cie->code_align; break;
        case DW_CFA_advance_loc2: delta = (uint32_t)(p[0] | p[1] << 8); p += 2; loc += delta * cie->code_align; break;
        case DW_CFA_advance_loc4: loc += read_u32(p) * cie->code_align; p += 4; break;
        case DW_CFA_offset_extended:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_OFFSET, (int32_t)read_uleb(&p) * cie->data_align);
            break;
        case DW_CFA_restore_extended:
            reg = read_uleb(&p);
            if (initial && reg < DWARF_REGS) fs->reg[reg] = initial->reg[reg];
            break;
        case DW_CFA_undefined: set_rule(fs, read_uleb(&p), RULE_UNDEFINED, 0); break;
        case DW_CFA_same_value: set_rule(fs, read_uleb(&p), RULE_SAME, 0); break;
        case DW_CFA_register:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_REGISTER, (int32_t)read_uleb(&p));
            break;
        case DW_CFA_remember_state:
            if (depth == MAX_REMEMBER) return 0;
            stack[depth++] = *fs;
            break;
        case DW_CFA_restore_state: {
            if (depth == 0) return 0;
            uint32_t args = fs->args_size;
            *fs = stack[--depth];
            fs->args_size = args;
            break;
        }
        case DW_CFA_def_cfa:
            fs->cfa_reg = read_uleb(&p);
            fs->cfa_offset = (int32_t)read_uleb(&p);
            fs->cfa_expr = NULL;
            break;
        case DW_CFA_def_cfa_register: fs->cfa_reg = read_uleb(&p); fs->cfa_expr = NULL; break;
        case DW_CFA_def_cfa_offset: fs->cfa_offset = (int32_t)read_uleb(&p); break;
        case DW_CFA_def_cfa_expression:
            fs->cfa_expr = p;
            p = skip_expr(p);
            break;
        case DW_CFA_expression:
        case DW_CFA_val_expression:
            reg = read_uleb(&p);
            if (reg < DWARF_REGS) {
                fs->reg[reg].how = op == DW_CFA_expression ? RULE_EXPRESSION : RULE_VAL_EXPRESSION;
                fs->reg[reg].expr = p;
            }
            p = skip_expr(p);
            break;
        case DW_CFA_offset_extended_sf:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_OFFSET, read_sleb(&p) * cie->data_align);
            break;
        case DW_CFA_def_cfa_sf:
            fs->cfa_reg = read_uleb(&p);
            fs->cfa_offset = read_sleb(&p) * cie->data_align;
            fs->cfa_expr = NULL;
            break;
        case DW_CFA_def_cfa_offset_sf: fs->cfa_offset = read_sleb(&p) * cie->data_align; break;
        case DW_CFA_val_offset:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_VAL_OFFSET, (int32_t)read_uleb(&p) * cie->data_align);
            break;
        case DW_CFA_val_offset_sf:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_VAL_OFFSET, read_sleb(&p) * cie->data_align);
            break;
        case DW_CFA_GNU_args_size: fs->args_size = read_uleb(&p); break;
        case DW_CFA_GNU_negative_offset_extended:
            reg = read_uleb(&p);
            set_rule(fs, reg, RULE_OFFSET, -(int32_t)read_uleb(&p) * cie->data_align);
            break;
        default:
            return 0;
        }
    }
    return 1;
}

/* Fill in what the frame at ctx->reg[REG_EIP] needs (its FDE, CFA, LSDA),
 * and its rules in *fs. 0: no FDE (the end of what can be unwound). */
static int frame_info(struct _Unwind_Context *ctx, struct frame_state *fs)
{
    struct fde f;
    /* A return address may be just past its call's function: look up the
     * call instruction instead. */
    uint32_t pc = ctx->reg[REG_EIP] - 1;
    if (!find_fde(pc, &f) || f.cie.ra_reg != REG_EIP) return 0;
    struct frame_state initial;
    memset(&initial, 0, sizeof initial);
    if (!run_cfa(f.cie.insns, f.cie.end, &f.cie, 0, 0xFFFFFFFFu, &initial, NULL)) return 0;
    *fs = initial;
    if (!run_cfa(f.insns, f.end, &f.cie, f.start, pc, fs, &initial)) return 0;
    if (fs->cfa_expr) {
        if (!eval_expr(fs->cfa_expr, ctx->reg, 0, 0, &ctx->cfa)) return 0;
    } else {
        if (fs->cfa_reg >= DWARF_REGS) return 0;
        ctx->cfa = ctx->reg[fs->cfa_reg] + (uint32_t)fs->cfa_offset;
    }
    ctx->args_size = fs->args_size;
    ctx->lsda = f.lsda;
    ctx->func_start = f.start;
    ctx->personality = f.cie.personality;
    return 1;
}

/* Move `ctx` to the caller of its frame, by the rules in `fs`. */
static int step(struct _Unwind_Context *ctx, const struct frame_state *fs)
{
    uint32_t old[DWARF_REGS];
    memcpy(old, ctx->reg, sizeof old);
    for (int r = 0; r < DWARF_REGS; r++) {
        const struct reg_rule *rule = &fs->reg[r];
        switch (rule->how) {
        case RULE_OFFSET:     ctx->reg[r] = *(const uint32_t *)(uintptr_t)(ctx->cfa + (uint32_t)rule->value); break;
        case RULE_VAL_OFFSET: ctx->reg[r] = ctx->cfa + (uint32_t)rule->value; break;
        case RULE_REGISTER:   if ((uint32_t)rule->value < DWARF_REGS) ctx->reg[r] = old[rule->value]; break;
        case RULE_EXPRESSION: {
            uint32_t at;
            if (!eval_expr(rule->expr, old, 1, ctx->cfa, &at)) return 0;
            ctx->reg[r] = *(const uint32_t *)(uintptr_t)at;
            break;
        }
        case RULE_VAL_EXPRESSION:
            if (!eval_expr(rule->expr, old, 1, ctx->cfa, &ctx->reg[r])) return 0;
            break;
        default: break;
        }
    }
    /* The caller's stack pointer is the frame's CFA. */
    ctx->reg[REG_ESP] = ctx->cfa;
    if (fs->reg[REG_EIP].how == RULE_SAME || fs->reg[REG_EIP].how == RULE_UNDEFINED) return 0;
    return ctx->reg[REG_EIP] != 0;
}

/* ── Installing a context ───────────────────────────────────────────── */

/* Restore ctx->reg and jump to its EIP (no return). eax, ecx, edx and
 * the target EIP travel on the target's stack. */
__attribute__((noreturn)) void __warpine_unwind_install(const uint32_t *reg);
__asm__(
    ".text\n"
    ".globl __warpine_unwind_install\n"
    "__warpine_unwind_install:\n"
    "    movl  4(%esp), %eax\n"         /* reg[] */
    "    movl  16(%eax), %edx\n"        /* the target's esp */
    "    subl  $16, %edx\n"
    "    movl  0(%eax), %ecx\n"  "    movl %ecx, 0(%edx)\n"    /* eax */
    "    movl  4(%eax), %ecx\n"  "    movl %ecx, 4(%edx)\n"    /* ecx */
    "    movl  8(%eax), %ecx\n"  "    movl %ecx, 8(%edx)\n"    /* edx */
    "    movl  32(%eax), %ecx\n" "    movl %ecx, 12(%edx)\n"   /* eip */
    "    movl  12(%eax), %ebx\n"
    "    movl  20(%eax), %ebp\n"
    "    movl  24(%eax), %esi\n"
    "    movl  28(%eax), %edi\n"
    "    movl  %edx, %esp\n"
    "    popl  %eax\n"
    "    popl  %ecx\n"
    "    popl  %edx\n"
    "    ret\n"
);

static uint32_t exception_chain(void)
{
    uint32_t head;
    __asm__ volatile("movl %%fs:0, %0" : "=r"(head));
    return head;
}

static __attribute__((noreturn)) void install(struct _Unwind_Context *ctx)
{
    uint32_t reg[DWARF_REGS];
    memcpy(reg, ctx->reg, sizeof reg);
    /* The landing pad runs with the call's pushed arguments gone. */
    reg[REG_ESP] += ctx->args_size;
    /* OS/2 handlers registered in the frames being left leave the chain. */
    for (uint32_t head = exception_chain(); head != 0xFFFFFFFFu && head != 0 && head < reg[REG_ESP];
         head = exception_chain()) {
        if (DosUnsetExceptionHandler((PEXCEPTIONREGISTRATIONRECORD)(uintptr_t)head) != NO_ERROR) break;
    }
    __warpine_unwind_install(reg);
}

/* ── The two phases ─────────────────────────────────────────────────── */

static _Unwind_Reason_Code phase2(struct _Unwind_Exception *exc, struct _Unwind_Context *ctx)
{
    for (;;) {
        struct frame_state fs;
        if (!frame_info(ctx, &fs)) return _URC_FATAL_PHASE2_ERROR;
        if (ctx->personality) {
            _Unwind_Action actions = _UA_CLEANUP_PHASE;
            if (ctx->cfa == exc->private_2) actions |= _UA_HANDLER_FRAME;
            _Unwind_Reason_Code rc = ctx->personality(1, actions, exc->exception_class, exc, ctx);
            if (rc == _URC_INSTALL_CONTEXT) install(ctx);
            if (rc != _URC_CONTINUE_UNWIND) return _URC_FATAL_PHASE2_ERROR;
            if (actions & _UA_HANDLER_FRAME) return _URC_FATAL_PHASE2_ERROR;
        }
        if (!step(ctx, &fs)) return _URC_FATAL_PHASE2_ERROR;
    }
}

/* Called by the assembly entry points with the registers of their caller
 * as it called them (eip: the return address). */
_Unwind_Reason_Code __warpine_unwind_raise(struct _Unwind_Exception *exc, const uint32_t *regs, int resume)
{
    struct _Unwind_Context start;
    memset(&start, 0, sizeof start);
    memcpy(start.reg, regs, sizeof start.reg);
    if (resume) {
        /* Phase 2 goes on from the frame that called _Unwind_Resume; it
         * only comes back if the stack cannot be unwound. */
        phase2(exc, &start);
        abort();
    }

    /* Phase 1: find a handler. */
    struct _Unwind_Context ctx = start;
    for (;;) {
        struct frame_state fs;
        if (!frame_info(&ctx, &fs)) return _URC_END_OF_STACK;
        if (ctx.personality) {
            _Unwind_Reason_Code rc = ctx.personality(1, _UA_SEARCH_PHASE, exc->exception_class, exc, &ctx);
            if (rc == _URC_HANDLER_FOUND) break;
            if (rc != _URC_CONTINUE_UNWIND) return _URC_FATAL_PHASE1_ERROR;
        }
        if (!step(&ctx, &fs)) return _URC_END_OF_STACK;
    }
    exc->private_1 = 0;
    exc->private_2 = ctx.cfa;

    /* Phase 2: clean up to it, then enter it. */
    ctx = start;
    return phase2(exc, &ctx);
}

/* The entry points save their caller's registers (eax..edi as DWARF
 * numbers them, esp as it is after they return, eip the return address)
 * and hand them to __warpine_unwind_raise. */
#define UNWIND_ENTRY(name, resume)                                           \
    __asm__(".text\n"                                                        \
            ".globl " #name "\n"                                             \
            #name ":\n"                                                      \
            "    pushl %ebp\n"                                               \
            "    movl  %esp, %ebp\n"                                         \
            "    subl  $40, %esp\n"                                          \
            "    movl  %eax, 0(%esp)\n"                                      \
            "    movl  %ecx, 4(%esp)\n"                                      \
            "    movl  %edx, 8(%esp)\n"                                      \
            "    movl  %ebx, 12(%esp)\n"                                     \
            "    leal  8(%ebp), %eax\n"  "    movl %eax, 16(%esp)\n"         \
            "    movl  0(%ebp), %eax\n"  "    movl %eax, 20(%esp)\n"         \
            "    movl  %esi, 24(%esp)\n"                                     \
            "    movl  %edi, 28(%esp)\n"                                     \
            "    movl  4(%ebp), %eax\n"  "    movl %eax, 32(%esp)\n"         \
            "    movl  %esp, %eax\n"                                         \
            "    pushl $" #resume "\n"                                       \
            "    pushl %eax\n"                                               \
            "    pushl 8(%ebp)\n"                                            \
            "    call  __warpine_unwind_raise\n"                             \
            "    addl  $12, %esp\n"                                          \
            "    movl  12(%esp), %ebx\n"                                     \
            "    movl  24(%esp), %esi\n"                                     \
            "    movl  28(%esp), %edi\n"                                     \
            "    leave\n"                                                    \
            "    ret\n")

UNWIND_ENTRY(_Unwind_RaiseException, 0);
UNWIND_ENTRY(_Unwind_Resume, 1);
UNWIND_ENTRY(_Unwind_Resume_or_Rethrow, 0);

void _Unwind_DeleteException(struct _Unwind_Exception *exc)
{
    if (exc->exception_cleanup) exc->exception_cleanup(_URC_FOREIGN_EXCEPTION_CAUGHT, exc);
}

/* Walk the stack from the caller, giving each frame to `trace`. */
_Unwind_Reason_Code _Unwind_Backtrace(_Unwind_Trace_Fn trace, void *arg)
{
    struct _Unwind_Context ctx;
    memset(&ctx, 0, sizeof ctx);
    ctx.reg[REG_EIP] = (uint32_t)(uintptr_t)__builtin_return_address(0);
    ctx.reg[REG_ESP] = (uint32_t)(uintptr_t)__builtin_frame_address(0) + 8;
    ctx.reg[5] = *(uint32_t *)__builtin_frame_address(0);
    for (;;) {
        struct frame_state fs;
        if (!frame_info(&ctx, &fs)) return _URC_END_OF_STACK;
        if (trace(&ctx, arg) != _URC_NO_REASON) return _URC_FATAL_PHASE1_ERROR;
        if (!step(&ctx, &fs)) return _URC_END_OF_STACK;
    }
}

/* ── What a personality routine asks of a context ───────────────────── */

_Unwind_Word _Unwind_GetGR(struct _Unwind_Context *ctx, int index)
{
    return index >= 0 && index < DWARF_REGS ? ctx->reg[index] : 0;
}

void _Unwind_SetGR(struct _Unwind_Context *ctx, int index, _Unwind_Word value)
{
    if (index >= 0 && index < DWARF_REGS) ctx->reg[index] = value;
}

_Unwind_Ptr _Unwind_GetIP(struct _Unwind_Context *ctx) { return ctx->reg[REG_EIP]; }

_Unwind_Ptr _Unwind_GetIPInfo(struct _Unwind_Context *ctx, int *ip_before_insn)
{
    if (ip_before_insn) *ip_before_insn = 0;
    return ctx->reg[REG_EIP];
}

void _Unwind_SetIP(struct _Unwind_Context *ctx, _Unwind_Ptr value) { ctx->reg[REG_EIP] = value; }
_Unwind_Word _Unwind_GetCFA(struct _Unwind_Context *ctx) { return ctx->cfa; }
_Unwind_Ptr _Unwind_GetRegionStart(struct _Unwind_Context *ctx) { return ctx->func_start; }
_Unwind_Ptr _Unwind_GetLanguageSpecificData(struct _Unwind_Context *ctx) { return ctx->lsda; }

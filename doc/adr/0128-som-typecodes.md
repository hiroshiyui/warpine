# ADR-0128: SOM TypeCodes in Warpine's own layout

- **Status:** Accepted; extends ADR-0122 (somidl: `TypeCode` was opaque) and ADR-0126 (`TCKind`)
- **Date:** 2026-11-25
- **Sources:**
  - IBM's OS/2 Warp 4 Toolkit *System Object Model Programming Reference* (kept in `doc/references/`, cited only): "TypeCode_alignment", "TypeCode_copy", "TypeCode_equal", "TypeCode_free", "TypeCode_kind" (with its Table 1, the parameters of each `TCKind`), "TypeCodeNew", "TypeCode_param_count", "TypeCode_parameter", "TypeCode_print", "TypeCode_setAlignment", "TypeCode_size".

## Context

A TypeCode describes an IDL data type at run time. CORBA's `any` carries one beside its value, and the Interface Repository, DSOM and the Workplace Shell's settings use them. Until now Warpine declared `TypeCode` as an opaque pointer with nothing behind it (ADR-0122), so `any` could be passed around but not described.

The Reference documents eleven functions: `TypeCodeNew`, which is variadic and takes different arguments for each kind, and `TypeCode_alignment`, `_copy`, `_equal`, `_free`, `_kind`, `_param_count`, `_parameter`, `_print`, `_setAlignment` and `_size`. It also documents the parameters each kind carries (Table 1) and the `TC_*` constants. It says that "TypeCodes are complex data structures whose actual representation is hidden". No public source gives IBM's layout or the ordinals these functions have in SOM.DLL.

## Decision

1. **Warpine's own layout.** A TypeCode is a `SOMMalloc` block in the guest: its kind, its parameter count, one word per parameter (the value of a long, the address of a string, or a TypeCode), then the strings it owns. `TypeCode_parameter` returns an `any` whose `_value` points at that parameter's word, and whose `_type` is `TC_long`, `TC_string` or `TC_TypeCode`, as the Reference describes.
2. **Host records decide.** The kernel also keeps every TypeCode in its own records (`som_typecode.rs`, `TypeCodeRec`), and every function answers from those records. A guest that overwrites a block cannot steer the kernel, and an address that is no TypeCode is never read. Unknown TypeCodes behave as follows:
   - `TypeCode_kind` gives `tk_null`;
   - `TypeCode_size`, `_alignment` and `_param_count` give 0;
   - `TypeCode_equal` gives FALSE;
   - `TypeCode_copy` gives NULL;
   - `TypeCode_free` does nothing;
   - `TypeCode_print` prints `NULL`.
3. **Ownership.** `TypeCodeNew` copies its strings and takes ownership of the TypeCodes it is given. The Reference's example builds a struct from nested `TypeCodeNew` calls and frees only the outer TypeCode, so the outer one must own the inner ones.
   - A TypeCode that another TypeCode already holds, or one given twice in the same call, is copied instead.
   - A constant is shared, not taken.
   - `TypeCode_free` frees a TypeCode together with the TypeCodes it took.
   - Freeing a constant, or a TypeCode that another TypeCode holds, does nothing.
   - `TypeCode_copy` copies everything except the constants, which last as long as the process.
4. **Sizes and alignments** are those of the C types of the SDK's i386 compilers (GCC and clang):
   - `double` is 8 bytes, aligned to 4;
   - `any` is 8 bytes;
   - a sequence is 12 bytes (`_maximum`, `_length`, `_buffer`);
   - `boolean`, `char` and `octet` are 1 byte;
   - `short` is 2 bytes; every other kind is a 4-byte word;
   - a struct is laid out as C lays it out;
   - a union is `struct { discriminator; union { members } }`;
   - an array is its element times its length;
   - a foreign type is its given size, aligned to 4, 2 or 1 depending on that size.

   `TypeCode_setAlignment` takes a power of two up to 64, and changes only the alignment, not the size. `TypeCode_equal` compares the kind, the parameters (strings by content, nested TypeCodes recursively), the union flags and the alignment.
5. **Unions.** A union member's `TCREGULAR_CASE` / `TCDEFAULT_CASE` flag is kept beside the parameters. It is not one of them: Table 1 gives a union `3N+2` parameters, label, name and type for each member. `TypeCode_print` shows the flags.
6. **Bounds.**
   - TypeCodes nest at most 32 deep.
   - A struct, union or enum has at most 1,024 members.
   - A string has at most 4,096 bytes.
   - A process holds at most 65,536 TypeCodes.

   `TypeCodeNew` returns NULL in all of these cases:
   - a call over these limits;
   - an unknown kind;
   - an argument that is no TypeCode;
   - a NULL or unreadable string, or one in Warpine's protected memory;
   - a negative bound;
   - a union flag that is neither of the two.
7. **`TypeCode_parameter` and Bounds.** An index out of range, or any index on a kind without parameters, raises the user exception `::TypeCode::Bounds` (`ex_TypeCode_Bounds`, with no value) in the `Environment` and gives an empty `any`.
8. **The calls** use Warpine's own ordinals (ADR-0123), SOM.3109–3120:
   - `TypeCodeNew` 3109, which reads its own arguments from the stack;
   - `TypeCode_alignment`, `_copy`, `_equal`, `_free`, `_kind` and `_param_count` 3110–3115;
   - `WrpTypeCode_parameter(result, tc, ev, index)` 3116;
   - `TypeCode_print`, `_setAlignment` and `_size` 3117–3119;
   - `WrpTypeCodeConstant(kind)` 3120.

   The SDK (`somapi.h`) does two things over these:
   - its `TypeCode_parameter` is a `static __inline__` function over `WrpTypeCode_parameter`, so no compiler's convention for returning a structure is involved;
   - the `TC_*` constants are macros over `WrpTypeCodeConstant`: one per basic kind, `TC_Object` (an object reference to `::SOMObject`) and `TC_string` (unbounded), each made once per process.
9. **`TypeCode_print`** writes, through `SOMOutCharRoutine`, the `TypeCodeNew` call that would make the TypeCode, as the Reference describes, followed by a newline: for example `TypeCodeNew (tk_struct, "node", "count", TypeCodeNew (tk_long), …, NULL)`.

Related fix: the kernel's guest-string reader (`som_bytes`, used for names and `somPrintf`'s format and `%s` arguments) now stops at Warpine's protected memory, so neither a TypeCode nor `somPrintf` can copy or print it.

## Consequences

- An `any` can now be described, so the Interface Repository, `somidl`'s `TC_<Name>` constants for declared types, and the Workplace Shell's settings can be built on these functions later.
- Programs built with Warpine's SDK link these functions by name. A program built against IBM's SOM.DLL ordinals for them would not link, since those ordinals are unknown (ADR-0123).
- The `TC_*` constants are function calls, so they cannot initialise static data. IBM's `somtcnst.h` declares them as data.
- `samples/c_som_test` checks the sizes and alignments against each compiler's own `sizeof` and `__alignof__`.

## Warpine's choices (unverified)

The following are Warpine's own choices, not verified against IBM's SOM:
- the block layout;
- the ownership rules: taking the TypeCodes given, copying one already held or given twice, ignoring a free of a held one;
- what an unknown TypeCode gives;
- the limits;
- `double` aligned to 4;
- a foreign type's alignment;
- `setAlignment` changing only the alignment, and its range;
- the exception's name and its lack of a value;
- `TypeCode_print`'s spacing and its newline;
- `TC_Object`'s id;
- the constants as function calls;
- `WrpTypeCode_parameter`;
- the ordinals.

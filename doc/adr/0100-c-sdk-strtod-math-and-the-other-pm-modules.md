# ADR-0100: The C SDK's strtod and math.h, the other PM modules' headers, extended attributes

- **Status:** Accepted; builds on ADR-0096–0099
- **Date:** 2026-11-21
- **Sources:**
  - The C standard (C11 7.12 and 7.22.1.3): `<math.h>` and `strtod`. IEEE 754 binary64.
  - The Intel 387 instruction set: `fsqrt`, `fsin`, `fcos`, `fptan`, `fpatan`, `fyl2x`, `f2xm1`, `fscale`, `fprem`, `frndint`.
  - IBM's *PM Programming Reference*: the profile, switch-list, program-start, file and font dialog, drag-and-drop, help and spooler calls and structures.
  - IBM's *Control Program Programming Reference*: the extended-attribute structures.
  - Warpine's code, for every layout it reads: `pm_switch.rs`, `pm_startapp.rs`, `pm_filedlg.rs`, `pm_fontdlg.rs`, `pm_drag.rs`, `helpmgr.rs`, `doscalls.rs` (EAs), and the constants in `src/`.

## Context

After ADR-0099 the SDK's TODO list held:
- `strtod` and `<math.h>`, the most common gaps in a ported program;
- headers for the PM modules besides PMWIN and PMGPI, whose calls Warpine serves and the def file lists;
- the extended-attribute structures, which `DosOpen`, `DosFindFirst` and `DosQueryPathInfo` take but the headers left opaque.

## Decision

1. **`strtod` and `atof` are correctly rounded** (`strtod.c`), the inverse of `dtoa.c`:
   - the decimal digits form a big integer D, and the value is D × 10^E exactly;
   - for E ≥ 0 the product is computed; for E < 0, D / 10^-E is divided out with 56–57 bits of quotient, by shifting the dividend up or (for very long inputs) the divisor;
   - the remainder and any digits past 800 are a sticky bit; the result is rounded once, ties to even, and assembled from its bits, subnormals included;
   - hexadecimal input, `inf`, `infinity` and `nan` are read too;
   - `ERANGE` is set for overflow, and for a non-zero input that rounds to zero.
2. **`<math.h>`** (`math.c`) gives the double functions:
   - **On the x87:** `sqrt`, `sin`, `cos`, `tan`, `atan`, `atan2`, `asin`, `acos`, `exp`, `log`, `log10`, `log2`, `pow`, `sinh`, `cosh`, `tanh`, `hypot` and `fmod`. They compute in 80-bit extended precision.
   - **Exact, on the bits:** `fabs`, `floor`, `ceil`, `trunc`, `round` (halves away from zero), `modf`, `frexp` and `ldexp`.
   - **The macros:** `isnan`, `isinf`, `isfinite`, `signbit`, `HUGE_VAL`, `INFINITY`, `NAN` and the `M_*` constants.
   - **Edge cases:** `pow` follows C's special cases (a zero base, an infinite exponent, a negative base with an integer exponent). Domain errors set `EDOM`, range errors `ERANGE`.
3. **The other PM modules' headers,** all included by `pm.h`:
   - **`os2/pmshl.h` (PMSHAPI):** the `Prf*` profile calls and `HINI_*`; the switch list (`SWCNTRL`, `SWENTRY`, `SWBLOCK`); `WinStartApp` / `WinTerminateApp` (`PROGDETAILS`).
   - **`os2/pmstddlg.h` (PMCTLS, PMDRAG):** `WinFileDlg` (`FILEDLG`, 328 bytes) and `WinFontDlg` (`FONTDLG`, 168 bytes); every `Drg*` call with `DRAGINFO`, `DRAGITEM`, `DRAGIMAGE` and `DRAGTRANSFER`, the `DM_*` messages and the `DO_*` / `DOR_*` values.
   - **`os2/pmhelp.h` (HELPMGR):** the help-instance calls, `HELPINIT` and `HELPTABLE`, some `HM_*` messages.
   - **`os2/pmspl.h` (PMSPL):** the `SplQm*` calls; `pm.h`'s `DEVOPENSTRUC` is now declared, not opaque.
4. **Extended attributes in `dos.h`:** `GEA2`, `GEA2LIST`, `FEA2`, `FEA2LIST` (packed to 1 byte), `EAOP2`, `DENA2`, `FEA_NEEDEA` and `ENUMEA_*`.
5. **`check-headers`** covers PMSHAPI, PMCTLS, HELPMGR, PMSPL and PMDRAG too. 622 calls are declared, and 1025 of the 1206 constants are compared with Warpine's, all equal.

## Warpine's choices (unverified)

- **Precision:** the x87 math functions are within about an ulp of the correctly rounded value, not always correctly rounded.
- **Ranges:** `sin`, `cos` and `tan` use the instructions' own range (|x| < 2^63).
- **`strtod`:** keeps 800 significant digits (further ones only decide rounding), and does not set `ERANGE` for subnormal results.
- **Help messages:** `HM_*` values other than `HM_DISPLAY_HELP`, `HM_SET_ACTIVE_WINDOW` and `HM_CONTROL` come from the reference without a Warpine constant to check them against.
- **Not yet declared:** the float and long double math variants, and the `Spl*Enum` / `Spl*Query` calls.

## Consequences

- **Ported code** that parses or computes with doubles builds against the SDK. `c_sdk_test` checks:
  - 21 `strtod` inputs against Python's `float()`, bit for bit: halfway cases, the smallest subnormal and its half, the largest double, hex, a 400-digit input;
  - 23 math results within an ulp, plus the exact functions and their error cases.
- **PM programs** can use profiles, the switch list, the standard dialogs, drag and drop and help from C. `c_pm_test` asserts each structure's size and key offsets against Warpine's. At run time it writes and reads a profile, makes drag string handles and a `DRAGINFO`, finds its frame's switch entry (`FCF_TASKLIST`), and queries its help instance.
- **Coverage:** of the def's entries for the covered modules, 38 are not declared yet (ADR-0098's 30, and the spooler's enumeration and query calls).
- **Tests:** `c_sdk_test` has 64 checks and `c_pm_test` 54, each passing with GCC and with clang.

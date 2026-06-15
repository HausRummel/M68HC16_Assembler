#!/usr/bin/env python3
"""Compare per-instruction byte counts: MASM OUT.LST vs our HC16_TRACE.

Both are processed top-to-bottom over the same source, so the instruction
sequences align. We diff them with difflib and report every instruction whose
byte count differs (i.e. an operand we sized wider/narrower than MASM), with the
module it came from.

Usage: lstcmp.py OUT.LST our_trace.txt mnemonics.txt
"""
import re
import sys
import difflib


def norm(src):
    # The MASM listing truncates long lines at a fixed width, cutting comments
    # mid-word; to align reliably we drop trailing comments entirely. Comments are
    # introduced by ';', '/*', or '..' (all seen in the corpus). '*' is left alone
    # (location counter / multiply in operands).
    s = src
    for delim in (";", "/*", ".."):
        idx = s.find(delim)
        if idx >= 0:
            s = s[:idx]
    s = re.sub(r"\s+", " ", s).strip().lower()
    return s


def op_of(src, mnemonics):
    # Return the mnemonic if this source line is an instruction, else None.
    toks = src.split()
    if not toks:
        return None
    # Possible leading label (no leading whitespace in original means token0 is a
    # label). The listing preserves original spacing, but we already collapsed it.
    # Try token0 then token1 as the op.
    for i in (0, 1):
        if i < len(toks) and toks[i].lower() in mnemonics:
            return toks[i].lower()
    return None


def parse_lst(path, mnemonics):
    """Ordered list of (nbytes, source_norm, module, lineno) for instruction lines.

    A long instruction/datum wraps onto continuation lines that carry more object
    bytes but no source text; those bytes belong to the preceding source line, so
    we accumulate them into the current anchor's byte count."""
    out = []
    module = "?"
    # loc, obj-field, optional source (>=2 spaces then non-space).
    emit_re = re.compile(
        r"^\s*\d+(?:\s+\d+[a-z]?)?\s+([0-9A-F]{6})\s+"
        r"([0-9A-F]+(?: [0-9A-F]+)*?)(?:\s{2,}(\S.*))?\s*$"
    )
    mod_re = re.compile(r"^68HC16 - (\S+) -")
    cur = None  # [nbytes, source_norm, module, lineno, op]

    def flush():
        if cur and cur[4] is not None:
            out.append((cur[0], cur[1], cur[2], cur[3]))

    with open(path, "r", errors="replace") as f:
        for raw in f:
            line = raw.rstrip("\n").rstrip("\r")
            m = mod_re.match(line)
            if m:
                module = m.group(1)
                continue
            m = emit_re.match(line)
            if not m:
                continue
            loc, objf, src = m.group(1), m.group(2), m.group(3)
            nb = len(objf.replace(" ", "")) // 2
            if src is not None:
                # New anchor line.
                flush()
                op = op_of(norm(src), mnemonics)
                cur = [nb, norm(src), module, int(loc, 16), op]
            elif cur is not None:
                # Continuation of the current anchor.
                cur[0] += nb
        flush()
    return out


def parse_trace(path, mnemonics):
    """Ordered list of (nbytes, source_norm, addr) for instruction lines.

    Trace format: 'ADDR<TAB>NBYTES<TAB>SOURCE' (SOURCE may contain tabs)."""
    out = []
    with open(path, "r", errors="replace") as f:
        for raw in f:
            parts = raw.rstrip("\n").split("\t")
            if len(parts) < 3:
                continue
            try:
                addr = int(parts[0], 16)
                nbytes = int(parts[1])
                src = "\t".join(parts[2:])
            except ValueError:
                continue
            n = norm(src)
            if op_of(n, mnemonics) is None:
                continue
            out.append((nbytes, n, addr))
    return out


def main():
    lst_path, trace_path, mnem_path = sys.argv[1:4]
    mnemonics = set(w.strip().lower() for w in open(mnem_path) if w.strip())
    lst = parse_lst(lst_path, mnemonics)
    trace = parse_trace(trace_path, mnemonics)
    print(f"LST instructions: {len(lst)}   trace instructions: {len(trace)}")

    a = [r[1] for r in lst]
    b = [r[1] for r in trace]
    sm = difflib.SequenceMatcher(a=a, b=b, autojunk=False)

    if "--drift" in sys.argv:
        # Track our_addr - masm_loc across aligned instructions. Both reset at the
        # same org/boundary directives, so a CHANGE in this delta between two
        # consecutive aligned instructions means a size mismatch happened in the
        # gap (possibly inside a nolist block the listing omits).
        prev = None
        changes = 0
        for tag, i1, i2, j1, j2 in sm.get_opcodes():
            if tag != "equal":
                continue
            for k in range(i2 - i1):
                masm_loc = lst[i1 + k][3]
                our_addr = trace[j1 + k][2]
                delta = our_addr - masm_loc
                if prev is not None and delta != prev[0]:
                    changes += 1
                    if changes <= 60:
                        print(f"  drift {prev[0]:+#x} -> {delta:+#x}  (step {delta-prev[0]:+d})  "
                              f"near masm={masm_loc:#07x} {lst[i1+k][2]}  '{lst[i1+k][1][:50]}'")
                prev = (delta, masm_loc)
        print(f"total drift-change points: {changes}")
        return
    mismatches = []   # (module, lineno, src, masm_n, ours_n)
    text_diffs = 0
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            for k in range(i2 - i1):
                ln, ls, mod, lno = lst[i1 + k]
                tn, ts = trace[j1 + k]
                if ln != tn:
                    mismatches.append((mod, lno, ls, ln, tn))
        else:
            text_diffs += max(i2 - i1, j2 - j1)
    print(f"size mismatches: {len(mismatches)}   (unaligned text regions: {text_diffs})")

    # Group by module.
    from collections import Counter
    bymod = Counter(m[0] for m in mismatches)
    print("by module (top 20):")
    for mod, c in bymod.most_common(20):
        print(f"  {mod:24} {c}")

    # Direction.
    wider = sum(1 for m in mismatches if m[4] > m[3])   # ours wider than masm
    narrower = sum(1 for m in mismatches if m[4] < m[3])
    print(f"ours wider than MASM: {wider}   ours narrower: {narrower}")

    if "--regions" in sys.argv:
        print("=== unaligned text regions (first 30) ===")
        shown = 0
        for tag, i1, i2, j1, j2 in sm.get_opcodes():
            if tag == "equal":
                continue
            print(f"--- {tag}: LST[{i1}:{i2}] vs trace[{j1}:{j2}] ---")
            for k in range(i1, min(i2, i1 + 4)):
                print(f"   LST  ({lst[k][2]} L{lst[k][3]}): {a[k]}")
            for k in range(j1, min(j2, j1 + 4)):
                print(f"   ours: {b[k]}")
            shown += 1
            if shown >= 30:
                break
        return

    print("=== OURS WIDER than MASM (we over-size) ===")
    for mod, lno, src, mn, tn in mismatches:
        if tn > mn:
            print(f"  {mod:16} L{lno}  masm={mn} ours={tn}  {src}")
    print("=== first 25 OURS NARROWER ===")
    shown = 0
    for mod, lno, src, mn, tn in mismatches:
        if tn < mn:
            print(f"  {mod:16} L{lno}  masm={mn} ours={tn}  {src}")
            shown += 1
            if shown >= 25:
                break


if __name__ == "__main__":
    main()

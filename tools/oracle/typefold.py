#!/usr/bin/env python3
"""Fit MASM's COFF symbol-`type` class-upgrade state machine against gold.

The type is NOT a static function of a symbol's context (proven in typerule.py);
it is an ordered fold over the symbol's class EVENTS (definition + each data use),
mirroring Masm.exe's sym_define (FUN_0001ee53) transition table. This tool:

  1. measures the PURITY ceiling: group symbols by their ordered event signature
     and report how many fall in signatures that map to >1 gold type (the floor
     for any function of the event stream alone), and
  2. scores a candidate fold() so the transition table can be refined to 0 diffs.

Usage: typefold.py GOLD.OBJ symevents.txt
"""
import struct
import sys
from collections import Counter, defaultdict


def parse_gold(path):
    d = open(path, "rb").read()
    symptr = struct.unpack_from("<I", d, 8)[0]
    nsym = struct.unpack_from("<I", d, 12)[0]
    strtab = symptr + nsym * 18
    out = {}
    i = 0
    o = symptr
    while i < nsym:
        n8 = d[o:o + 8]
        val, scn, typ, scl, naux = struct.unpack_from("<IhHBB", d, o + 8)
        if n8[:4] == b"\x00\x00\x00\x00":
            so = strtab + struct.unpack_from("<I", n8, 4)[0]
            name = d[so:d.index(b"\x00", so)].decode("latin1")
        else:
            name = n8.split(b"\x00")[0].decode("latin1")
        if naux == 0:
            out[name] = typ
        o += 18 * (1 + naux)
        i += 1 + naux
    return out


def read_events(path):
    rows = {}
    for ln in open(path, encoding="latin1").read().splitlines()[1:]:
        f = ln.split("\t")
        name, value, kind, defctx, addr, listed = f[0], f[1], f[2], f[3], f[4], f[5]
        events = f[6].split(",") if len(f) > 6 and f[6] else []
        rows[name] = dict(kind=kind, defctx=defctx, addr=addr, listed=listed, events=events)
    return rows


# ---- the rule coff.rs implements today (first-occurrence element) -----------
ELEM = {"W": 3, "B": 2, "R": 2, "C": 0, "E": None, "-": None}


def current(r):
    """Reproduce output/coff.rs exactly: type = element of the first-occurrence
    context; an Abs with no element-bearing first-occ is 0, a Rel falls back to
    the element at its address."""
    ev = r["events"]
    ctx = ELEM[ev[0][1]] if ev else None
    if r["kind"] == "Abs":
        return ctx or 0
    if ctx is not None:
        return ctx
    return ELEM.get(r["addr"], None) or 0


def main():
    gold = parse_gold(sys.argv[1])
    rows = read_events(sys.argv[2])
    names = [n for n in gold if n in rows]
    print(f"symbols compared: {len(names)}")

    # 1) purity ceiling, for several signature definitions (does adding defctx/addr
    #    break the impurity?).
    def purity(sigfn, label):
        by_sig = defaultdict(Counter)
        for n in names:
            by_sig[sigfn(rows[n])][gold[n]] += 1
        floor = sum(sum(c.values()) - c.most_common(1)[0][1]
                    for c in by_sig.values() if len(c) > 1)
        nimp = sum(1 for c in by_sig.values() if len(c) > 1)
        print(f"  {label:28s} sigs={len(by_sig):4d}  impure={nimp:3d}  floor={floor}")
        return by_sig

    print("purity ceilings (min achievable diffs for a function of that signature):")
    purity(lambda r: (r["kind"], tuple(r["events"])), "events")
    purity(lambda r: (r["kind"], r["listed"], tuple(r["events"])), "listed+events")
    purity(lambda r: (r["kind"], r["listed"], r["addr"], tuple(r["events"])), "listed+addr+events")
    by_sig = purity(lambda r: (r["kind"], r["listed"], r["addr"], tuple(r["events"])), "(used below)")
    print()

    # 2) verify current() reproduces coff.rs (must be 482)
    cur = Counter()
    for n in names:
        if current(rows[n]) != gold[n]:
            cur[(current(rows[n]), gold[n])] += 1
    print(f"\ncurrent() [= coff.rs today] diffs total = {sum(cur.values())}")
    for k in sorted(cur, key=lambda k: -cur[k]):
        print(f"  {k[0]}->{k[1]} = {cur[k]:4d}")

    # 3) no-regression ceiling: signatures that are PURE (one gold type) but where
    #    current() is wrong for all members. Flipping these costs zero regressions.
    sig = lambda r: (r["kind"], r["listed"], r["addr"], tuple(r["events"]))
    members = defaultdict(list)
    for n in names:
        members[sig(rows[n])].append(n)
    pure_fixable = 0
    impure_cur_loss = 0
    maj_gain = 0
    for s, ms in members.items():
        types = Counter(gold[n] for n in ms)
        maj, mc = types.most_common(1)[0]
        # majority-vote (overfit) gain over current()
        cur_right = sum(1 for n in ms if current(rows[n]) == gold[n])
        maj_gain += mc - cur_right
        if len(types) == 1:                     # pure signature
            if current(rows[ms[0]]) != maj:
                pure_fixable += len(ms)
        else:                                   # impure: current's loss vs majority
            impure_cur_loss += (len(ms) - cur_right) - (len(ms) - mc)
    print(f"\nno-regression ceiling (pure signatures current() gets wrong): {pure_fixable}")
    print(f"majority-vote (overfit) total gain over current: {maj_gain}  -> {sum(cur.values()) - maj_gain} diffs")

    # 3b) net effect of principled candidate rules (fixes vs regressions vs current)
    def relnone0(r):
        """Like current(), but a Rel symbol with no element-bearing first-occ
        context (EQU/bare/marker def) is typed 0 instead of its address element."""
        ev = r["events"]
        ctx = ELEM[ev[0][1]] if ev else None
        if r["kind"] == "Abs":
            return ctx or 0
        return ctx if ctx is not None else 0

    def relequ0(r):
        """Only force 0 when the first occurrence is literally the symbol's own
        EQU/bare-label definition (role 'd', no element)."""
        ev = r["events"]
        if r["kind"] == "Rel" and ev and ev[0][0] == "d" and ELEM[ev[0][1]] is None:
            return 0
        return current(r)

    for name_, rule in [("relnone0", relnone0), ("relequ0", relequ0)]:
        fixes = sum(1 for n in names if current(rows[n]) != gold[n] and rule(rows[n]) == gold[n])
        regr = sum(1 for n in names if current(rows[n]) == gold[n] and rule(rows[n]) != gold[n])
        tot = sum(1 for n in names if rule(rows[n]) != gold[n])
        print(f"  rule {name_:10s}: fixes={fixes} regressions={regr} net={fixes-regr}  total_diffs={tot}")

    # 4) biggest pure-wrong signatures (principled, zero-regression rules)
    print("\nbiggest PURE signatures current() mispredicts (kind/listed/addr/events: cur->gold xN):")
    rowscore = []
    for s, ms in members.items():
        types = set(gold[n] for n in ms)
        if len(types) == 1:
            g = next(iter(types))
            c = current(rows[ms[0]])
            if c != g:
                rowscore.append((len(ms), s, c, g, ms[:4]))
    for cnt, s, c, g, ex in sorted(rowscore, reverse=True)[:16]:
        k, ls, ad, ev = s
        print(f"  {cnt:4d}  {k:3s} l={ls} a={ad} {','.join(ev)[:34]:34s} {c}->{g}  e.g. {' '.join(ex)}")

    # 3) show worst impure signatures (to refine event detail if needed)
    print("\nworst impure signatures (kind/listed/addr/events -> gold-type spread):")
    for sig, c in sorted(by_sig.items(), key=lambda kv: -(sum(kv[1].values()) - kv[1].most_common(1)[0][1]))[:14]:
        if len(c) > 1:
            k, ls, ad, ev = sig
            print(f"  {k:3s} l={ls} a={ad} {','.join(ev)[:44]:44s} {dict(c)}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Parse the Motorola MASM COFF-style .OBJ to map its structure.

Usage: objdump.py FILE.OBJ [--syms N] [--sec]
"""
import struct
import sys


def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    print(f"file size: {len(data)}")

    # COFF file header (20 bytes).
    (magic, nsec, tstamp, symptr, nsym, optsz, flags) = struct.unpack_from("<HHIIIHH", data, 0)
    print(f"--- file header ---")
    print(f"  magic        = {magic:#06x}")
    print(f"  num sections = {nsec}")
    print(f"  timestamp    = {tstamp:#010x}")
    print(f"  symtab ptr   = {symptr:#x} ({symptr})")
    print(f"  num symbols  = {nsym}")
    print(f"  opt hdr size = {optsz}")
    print(f"  flags        = {flags:#06x}")

    # Section table starts at 20 + optsz. Standard COFF section header = 40 bytes.
    sec_off = 20 + optsz
    print(f"--- section table @ {sec_off:#x} ({nsec} entries) ---")
    secs = []
    for i in range(nsec):
        o = sec_off + i * 40
        name = data[o:o + 8].split(b"\x00")[0].decode("latin1")
        (paddr, vaddr, size, scnptr, relptr, lnnoptr, nreloc, nlnno, sflags) = \
            struct.unpack_from("<IIIIIIHHI", data, o + 8)
        secs.append(dict(name=name, paddr=paddr, vaddr=vaddr, size=size, scnptr=scnptr,
                         relptr=relptr, nreloc=nreloc, flags=sflags))
        print(f"  [{i:2}] {name:<8} paddr={paddr:#08x} vaddr={vaddr:#08x} size={size:#08x} "
              f"scnptr={scnptr:#08x} relptr={relptr:#08x} nreloc={nreloc} flags={sflags:#010x}")

    # String table: right after the symbol table (each symbol entry = 18 bytes).
    strtab_off = symptr + nsym * 18
    if strtab_off + 4 <= len(data):
        strtab_len = struct.unpack_from("<I", data, strtab_off)[0]
        print(f"--- string table @ {strtab_off:#x}, declared len {strtab_len} "
              f"(file has {len(data) - strtab_off}) ---")

    # Sample symbols.
    n = 20
    for a in sys.argv:
        if a.startswith("--syms"):
            n = int(a.split("=")[1]) if "=" in a else 20
    print(f"--- first {n} symbols @ {symptr:#x} ---")
    i = 0
    shown = 0
    while i < nsym and shown < n:
        o = symptr + i * 18
        raw = data[o:o + 18]
        zeros, = struct.unpack_from("<I", raw, 0)
        if zeros == 0:
            stroff, = struct.unpack_from("<I", raw, 4)
            name = read_str(data, strtab_off, stroff)
        else:
            name = raw[0:8].split(b"\x00")[0].decode("latin1")
        (value, scnum, stype, sclass, naux) = struct.unpack_from("<IhHBB", raw, 8)
        print(f"  [{i:5}] {name:<28} val={value:#08x} scnum={scnum} type={stype:#06x} "
              f"class={sclass} naux={naux}")
        i += 1 + naux
        shown += 1


def read_str(data, strtab_off, stroff):
    o = strtab_off + stroff
    end = data.index(b"\x00", o)
    return data[o:end].decode("latin1")


if __name__ == "__main__":
    main()

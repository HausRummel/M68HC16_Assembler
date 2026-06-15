#!/usr/bin/env python3
"""Carve the DOS/4GW LE (Linear Executable) image out of a bound MZ+LE .exe and
emit a fully-relocated FLAT binary suitable for loading into Ghidra as x86:LE:32.

Masm.exe (and the rest of the Motorola toolchain) is a Watcom/DOS-4GW protected-
mode program. Ghidra only auto-loads the 12.5 KB real-mode MZ stub; the real code
is the LE image whose header sits at the MZ `e_lfanew` offset (~0x28B8 for Masm).

The LE image has 2 objects (code @ 0x10000, data @ 0x30000). All fixups in Masm.exe
are source-type 7 (32-bit offset) internal references, so a flat relocated image is
exact: we place each object at its virtual base and overwrite every 32-bit fixup
source with (target_object_base + target_offset).

Usage:  le_to_flat.py INPUT.EXE OUTPUT.bin
Prints the load base, object layout, and entry-point VA (load these into Ghidra).
"""
import struct
import sys


def main():
    inp, outp = sys.argv[1], sys.argv[2]
    f = open(inp, "rb").read()

    e_lfanew = struct.unpack_from("<I", f, 0x3C)[0]
    LE = e_lfanew
    assert f[LE:LE + 2] == b"LE", f"no LE header at {LE:#x}"

    def u32(o):
        return struct.unpack_from("<I", f, o)[0]

    num_pages = u32(LE + 0x14)
    eip_obj = u32(LE + 0x18)
    eip = u32(LE + 0x1C)
    page_size = u32(LE + 0x28)
    last_page_size = u32(LE + 0x2C)
    obj_tab = LE + u32(LE + 0x40)
    num_objs = u32(LE + 0x44)
    obj_pagemap = LE + u32(LE + 0x48)  # noqa: F841 (pages are sequential here)
    fixup_pagetab = LE + u32(LE + 0x68)
    fixup_rectab = LE + u32(LE + 0x6C)
    data_pages = u32(LE + 0x80)  # file offset to page data

    # Object table: 24 bytes each.
    objs = []
    for i in range(num_objs):
        o = obj_tab + i * 24
        vsize = u32(o)
        base = u32(o + 4)
        flags = u32(o + 8)
        pm_idx = u32(o + 12)
        pm_cnt = u32(o + 16)
        objs.append(dict(vsize=vsize, base=base, flags=flags, pm_idx=pm_idx, pm_cnt=pm_cnt))

    base_va = min(o["base"] for o in objs)
    end_va = max(o["base"] + o["vsize"] for o in objs)
    flat = bytearray(end_va - base_va)

    # Map each 1-based global page number -> (file offset of its data, virtual address).
    # Pages are laid out sequentially in the file from data_pages; each object owns a
    # contiguous run of page-map indices [pm_idx, pm_idx+pm_cnt).
    page_va = {}
    for ob in objs:
        for k in range(ob["pm_cnt"]):
            pageno = ob["pm_idx"] + k          # global 1-based page number
            va = ob["base"] + k * page_size
            page_va[pageno] = va
            foff = data_pages + (pageno - 1) * page_size
            this_sz = last_page_size if pageno == num_pages else page_size
            flat[va - base_va: va - base_va + this_sz] = f[foff:foff + this_sz]

    # Fixup page table: num_pages+1 u32 offsets into the fixup record table.
    pte = [u32(fixup_pagetab + i * 4) for i in range(num_pages + 1)]

    applied = 0
    for pg in range(num_pages):       # 0-based; global page number = pg+1
        pageno = pg + 1
        pva = page_va[pageno]
        o = fixup_rectab + pte[pg]
        end = fixup_rectab + pte[pg + 1]
        while o < end:
            src = f[o]
            flags = f[o + 1]
            o += 2
            st = src & 0x0F
            # Source offset list.
            offs = []
            if src & 0x20:
                cnt = f[o]
                o += 1
                for _ in range(cnt):
                    offs.append(struct.unpack_from("<h", f, o)[0])
                    o += 2
            else:
                offs.append(struct.unpack_from("<h", f, o)[0])
                o += 2
            # Internal target.
            assert (flags & 0x03) == 0, "only internal fixups supported"
            objnum = (struct.unpack_from("<H", f, o)[0], o := o + 2)[0] if (flags & 0x40) \
                else (f[o], o := o + 1)[0]
            if st != 2:
                if flags & 0x10:
                    toff = u32(o); o += 4
                else:
                    toff = struct.unpack_from("<H", f, o)[0]; o += 2
            else:
                toff = 0
            assert st == 7, f"unhandled source type {st}"
            target = objs[objnum - 1]["base"] + toff
            for so in offs:
                dst = pva + so - base_va
                if 0 <= dst <= len(flat) - 4:
                    struct.pack_into("<I", flat, dst, target & 0xFFFFFFFF)
                    applied += 1

    open(outp, "wb").write(flat)
    print(f"wrote {outp}: {len(flat)} bytes")
    print(f"LOAD BASE (image base) = {base_va:#x}")
    for i, ob in enumerate(objs):
        kind = "CODE" if (ob["flags"] & 0x4) else "DATA"
        print(f"  object[{i+1}] {kind}: va {ob['base']:#x}..{ob['base']+ob['vsize']:#x} "
              f"(vsize {ob['vsize']:#x}, flags {ob['flags']:#x})")
    print(f"ENTRY POINT VA = {objs[eip_obj-1]['base'] + eip:#x}")
    print(f"fixups applied = {applied}")


if __name__ == "__main__":
    main()

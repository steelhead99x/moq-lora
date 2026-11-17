#!/usr/bin/env python3
"""Minimal ELF -> UF2 for RP2040 (family 0xe48bff56): PT_LOAD segments by physical address, 256-byte blocks."""
import struct, sys
elf = open(sys.argv[1], 'rb').read()
assert elf[:4] == b'\x7fELF' and elf[4] == 1
phoff, = struct.unpack_from('<I', elf, 28); phentsize, phnum = struct.unpack_from('<HH', elf, 42)
mem = {}
for i in range(phnum):
    t, off, vaddr, paddr, filesz, memsz, flags, align = struct.unpack_from('<8I', elf, phoff + i * phentsize)
    if t == 1 and filesz:
        assert 0x10000000 <= paddr < 0x10200000, hex(paddr)
        for j in range(filesz): mem[paddr + j] = elf[off + j]
pages = sorted({a & ~255 for a in mem})
out = bytearray()
for n, p in enumerate(pages):
    data = bytes(mem.get(p + k, 0) for k in range(256))
    out += struct.pack('<8I', 0x0A324655, 0x9E5D5157, 0x2000, p, 256, n, len(pages), 0xe48bff56) + data + b'\0' * 220 + struct.pack('<I', 0x0AB16F30)
open(sys.argv[2], 'wb').write(out)
print(f"{len(pages)} blocks, {len(pages)*256} bytes, 0x{pages[0]:08x}..0x{pages[-1]+256:08x}")
